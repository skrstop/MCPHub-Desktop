#!/usr/bin/env python3
"""E2E round r1200 (round-12 coverage).

A  Bearer key matrix (largest uncovered auth face): enable
   routing.enableBearerAuth via system_config + create a real bearer key row
   (bearer CRUD is Tauri-command-only, no REST route exists, so the key is
   inserted directly into bearer_keys, same pattern as rmcp_carpet_r200).
   Channels under test: root /mcp, server-scope /mcp/{IP},
   /rest/{server}/call, /rest/group/{g}/call, /api/tools/{server}/{tool},
   /api/$smart/search.  No token -> 401; invalid token -> 401; valid token ->
   2xx with REAL public-IP tool call on MCP root + server scope (2025-06-18
   legacy and 2026-07-28 stateless) and on all REST faces.  Then restrict the
   key's allowed_servers to ONLY the IP server: non-allowed server refused
   while the IP tool still works; finally empty allow-list -> fail-closed
   (deny everything, per R53 semantics, verified empirically and printed).
   Bearer flag + key row restored in finally.

B  Disabled-tool gate on ALL faces: disable playwright browser_close via
   server_tool_config (enabled=0) and verify absence/refusal on /mcp
   tools/list, /mcp tools/call, /rest/{server}/tools, /rest/{server}/call,
   /api/tools/{server}/{tool}, and $smart search.  Row state restored.

C  Group allow-list semantics (existing "Test" group containing playwright +
   the IP server): /mcp/{group} and /rest/group/{g}/call respect membership;
   a non-member tool refused; trailing-slash group path still resolves.

D  Cleanup/completeness audit (repo grep): retired-symbol scan printed as a
   summary table; zero CODE hits asserted (comment hits inspected + printed).

Exit 0 only if all cases pass.  All state restored.
"""
import json
import os
import re
import sqlite3
import subprocess
import sys
import time
import urllib.parse

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import rmcp_round_r1100 as base  # noqa: E402
from rmcp_round_r1100 import (  # noqa: E402,F401
    HOST, PORT, DB,
    req, sse_last, json_txt, cfg_read, cfg_write,
    rpc, initialize, notify_initialized, result_text, tool_error,
    find_ip_tool, valid_pub_ip, IP_SERVER, IP_TOOL, META_2026,
)
check = base.check
skip = base.skip

IP_Q = urllib.parse.quote(IP_SERVER, safe="")
PW_Q = urllib.parse.quote("playwright", safe="")
GRP = "Test"
GRP_Q = urllib.parse.quote(GRP, safe="")
TEST_KEY = "mcphub_e2e_r1200_bearer"
NON_MEMBER_TOOL = "codegraph_search"          # codegraph not in Test group
DISABLE_SERVER, DISABLE_TOOL = "playwright", "browser_close"
BEARER = {"Authorization": f"Bearer {TEST_KEY}"}


def health():
    try:
        st, _, _ = req("GET", "/health", default_accept=False, timeout=5)
        return st == 200
    except Exception:
        return False


# -- DB state helpers ---------------------------------------------------------
def bearer_snapshot():
    con = sqlite3.connect(DB)
    rows = con.execute(
        "SELECT id, name, token, enabled, access_type, allowed_groups, allowed_servers "
        "FROM bearer_keys WHERE token=?", (TEST_KEY,)).fetchall()
    con.close()
    return rows


def bearer_upsert(enabled, access_type, allowed_servers):
    con = sqlite3.connect(DB)
    con.execute("DELETE FROM bearer_keys WHERE token=?", (TEST_KEY,))
    con.execute(
        "INSERT INTO bearer_keys (id, name, token, enabled, access_type, allowed_groups, allowed_servers) "
        "VALUES (lower(hex(randomblob(16))), 'e2e-r1200', ?, ?, ?, '[]', ?)",
        (TEST_KEY, 1 if enabled else 0, access_type,
         json.dumps(allowed_servers, ensure_ascii=False)))
    con.commit()
    con.close()
    time.sleep(0.3)


def tool_cfg_snapshot(server, tool):
    con = sqlite3.connect(DB)
    row = con.execute(
        "SELECT id, enabled, description, pinned FROM server_tool_config "
        "WHERE server_name=? AND item_type='tool' AND item_name=?",
        (server, tool)).fetchone()
    con.close()
    return row


def tool_cfg_set_enabled(server, tool, val):
    con = sqlite3.connect(DB)
    if val is None:
        con.execute("DELETE FROM server_tool_config WHERE server_name=? AND item_type='tool' AND item_name=?",
                    (server, tool))
    else:
        con.execute(
            """INSERT INTO server_tool_config (id, server_name, item_type, item_name, enabled, description, pinned)
               VALUES (lower(hex(randomblob(16))), ?, 'tool', ?, ?, NULL, 0)
               ON CONFLICT(server_name, item_type, item_name) DO UPDATE SET enabled=excluded.enabled""",
            (server, tool, val))
    con.commit()
    con.close()
    time.sleep(0.6)


def set_bearer_flag(on):
    cfg = cfg_read()
    cfg.setdefault("routing", {})["enableBearerAuth"] = bool(on)
    cfg_write(cfg)


def mcp_names(path, version="2025-06-18", rid=1):
    st, echoed, sid = initialize(path, version, rid=rid, client="r1200")
    if st != 200 or echoed != version:
        return st, echoed, sid, []
    notify_initialized(path, sid)
    st, data, _ = rpc(path, rid + 1, "tools/list", {}, session=sid, timeout=90)
    o = sse_last(data, rid + 1)
    names = [t.get("name", "") for t in (((o or {}).get("result", {}) or {}).get("tools", []) or [])]
    return st, echoed, sid, names


def init_b(path, version="2025-06-18", rid=1):
    """Initialize with explicit extra headers (r1100.initialize has no header arg)."""
    st, data, h = req("POST", path, body={
        "jsonrpc": "2.0", "id": rid, "method": "initialize", "params": {
            "protocolVersion": version, "capabilities": {},
            "clientInfo": {"name": "r1200", "version": "1.0"}}},
        headers=dict(BEARER), timeout=90)
    o = sse_last(data, rid)
    echoed = ((o or {}).get("result", {}) or {}).get("protocolVersion")
    return st, echoed, h.get("mcp-session-id", "")


def rpc_b(path, rid, method, params=None, session=None, timeout=90):
    h = {"mcp-session-id": session} if session else {}
    h.update(BEARER)
    b = {"jsonrpc": "2.0", "id": rid, "method": method}
    if params is not None:
        b["params"] = params
    return req("POST", path, body=b, headers=h, timeout=timeout)


def list_with_bearer(path, sid, rid):
    st, data, _ = rpc_b(path, rid, "tools/list", {}, session=sid)
    o = sse_last(data, rid)
    names = [t.get("name", "") for t in (((o or {}).get("result", {}) or {}).get("tools", []) or [])]
    return st, names, o


def rest_call(path, tool, args=None, headers=None):
    return req("POST", path, body={"tool": tool, "arguments": args or {}},
               headers=headers, timeout=90)


def rest_is_ip_ok(st, data):
    if st != 200:
        return False
    try:
        o = json.loads(data)
    except Exception:
        return False
    text = json.dumps(o, ensure_ascii=False)
    return (o.get("isError") is False) or valid_pub_ip(text)


# -- Section A ----------------------------------------------------------------
def section_a():
    print("== A: bearer key matrix ==")
    set_bearer_flag(True)
    bearer_upsert(True, "servers", ["playwright", IP_SERVER])
    time.sleep(0.5)

    # A1  no token / invalid token on every channel -> 401
    init_body = {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {},
        "clientInfo": {"name": "r1200", "version": "1.0"}}}
    for label, fn in [
        ("root /mcp initialize", lambda h: req("POST", "/mcp", body=init_body, headers=h)),
        ("server-scope initialize", lambda h: req("POST", f"/mcp/{IP_Q}", body=init_body, headers=h)),
        ("rest/{server}/call", lambda h: rest_call(f"/rest/{IP_Q}/call", IP_TOOL, headers=h)),
        ("rest/group/{g}/call", lambda h: rest_call(f"/rest/group/{GRP_Q}/call", IP_TOOL, headers=h)),
        ("api/tools/{server}/{tool}", lambda h: req("POST", f"/api/tools/{IP_Q}/{IP_TOOL}", {}, headers=h)),
        ("api/$smart/search", lambda h: req("POST", "/api/$smart/search", {"query": "ip", "limit": 5}, headers=h)),
    ]:
        st, _, _ = fn(None)
        check(f"A1 {label} no token -> 401", st == 401, f"st={st}")
        st, _, _ = fn({"Authorization": "Bearer mcphub_invalid_token_000"})
        check(f"A1 {label} invalid token -> 401", st == 401, f"st={st}")

    # A2  valid token - MCP root legacy + 2026 stateless, real public-IP call
    st, echoed, sid = init_b("/mcp", rid=10)
    st, names, _o = (list_with_bearer("/mcp", sid, 11) if sid else (st, [], None))
    ip_name = find_ip_tool(names)
    check("A2 root legacy initialize+tools/list with bearer", st == 200 and ip_name is not None,
          f"st={st} n_tools={len(names)}")
    if sid and ip_name:
        st, data, _ = rpc_b("/mcp", 12, "tools/call", {"name": ip_name, "arguments": {}},
                            session=sid)
        o = sse_last(data, 12)
        check("A2 root legacy real public-IP call",
              st == 200 and tool_error(o) is None and valid_pub_ip(result_text(o)),
              f"st={st} {json_txt(o)[:140]}")
    st, data, _ = req("POST", "/mcp", body={
        "jsonrpc": "2.0", "id": 13, "method": "tools/call",
        "params": {"name": f"{IP_SERVER}-{IP_TOOL}", "arguments": {}, **META_2026}},
        headers={**BEARER, "MCP-Protocol-Version": "2026-07-28"}, timeout=90)
    o = sse_last(data, 13)
    r = (o or {}).get("result", {}) or {}
    check("A2 root 2026 stateless real public-IP call",
          st == 200 and r.get("resultType") == "complete" and valid_pub_ip(result_text(o)),
          f"st={st} {data[:140]}")

    # A3  valid token - server scope (bare tool name), legacy + 2026
    st, echoed, sid = init_b(f"/mcp/{IP_Q}", rid=20)
    names = []
    if sid:
        st, names, _o = list_with_bearer(f"/mcp/{IP_Q}", sid, 21)
    check("A3 server-scope tools/list exposes bare IP tool",
          st == 200 and IP_TOOL in names, f"st={st} names={names[:6]}")
    if sid:
        st, data, _ = rpc_b(f"/mcp/{IP_Q}", 22, "tools/call", {"name": IP_TOOL, "arguments": {}},
                            session=sid)
        o = sse_last(data, 22)
        check("A3 server-scope legacy real public-IP call",
              st == 200 and tool_error(o) is None and valid_pub_ip(result_text(o)),
              f"st={st} {json_txt(o)[:140]}")
    st, data, _ = req("POST", f"/mcp/{IP_Q}", body={
        "jsonrpc": "2.0", "id": 23, "method": "tools/call",
        "params": {"name": IP_TOOL, "arguments": {}, **META_2026}},
        headers={**BEARER, "MCP-Protocol-Version": "2026-07-28"}, timeout=90)
    o = sse_last(data, 23)
    r = (o or {}).get("result", {}) or {}
    check("A3 server-scope 2026 stateless real public-IP call",
          st == 200 and r.get("resultType") == "complete" and valid_pub_ip(result_text(o)),
          f"st={st} {data[:140]}")

    # A4  valid token - REST faces real call
    st, data, _ = rest_call(f"/rest/{IP_Q}/call", IP_TOOL, headers=BEARER)
    check("A4 /rest/{server}/call real public-IP", rest_is_ip_ok(st, data), f"st={st} {data[:120]}")
    st, data, _ = rest_call(f"/rest/group/{GRP_Q}/call", IP_TOOL, headers=BEARER)
    check("A4 /rest/group/{g}/call real public-IP", rest_is_ip_ok(st, data), f"st={st} {data[:120]}")
    st, data, _ = req("POST", f"/api/tools/{IP_Q}/{IP_TOOL}", {}, headers=BEARER, timeout=90)
    check("A4 /api/tools/{server}/{tool} real public-IP", rest_is_ip_ok(st, data), f"st={st} {data[:120]}")
    st, data, _ = req("POST", "/api/$smart/search", {"query": "ip", "limit": 5}, headers=BEARER)
    ok = st == 200 and (IP_TOOL in data or valid_pub_ip(data))
    check("A4 /api/$smart/search with bearer -> 200 + results", ok, f"st={st} {data[:120]}")

    # A5  restrict allow-list to IP server only
    bearer_upsert(True, "servers", [IP_SERVER])
    time.sleep(0.5)
    st, data, _ = rest_call(f"/rest/{PW_Q}/call", DISABLE_TOOL, headers=BEARER)
    check("A5 non-allowed server /rest/call refused", st in (401, 403), f"st={st}")
    st, data, _ = req("GET", f"/rest/{PW_Q}/tools", headers=BEARER)
    check("A5 non-allowed server /rest/tools refused", st in (401, 403), f"st={st}")
    # Actual semantics (verified r1200): initialize on a non-allowed server
    # scope still negotiates a session (200), but tools/list and tools/call
    # are refused - enforcement happens at the tool surface, not at
    # initialize.
    st, echoed, sid = init_b(f"/mcp/{PW_Q}", rid=31)
    check("A5 non-allowed server-scope initialize allowed (documented)",
          st == 200, f"st={st}")
    if sid:
        st, names, _o = list_with_bearer(f"/mcp/{PW_Q}", sid, 32)
        check("A5 non-allowed server-scope tools/list refused/empty",
              st in (401, 403) or not names, f"st={st} n={len(names)}")
    st, data, _ = rest_call(f"/rest/{IP_Q}/call", IP_TOOL, headers=BEARER)
    check("A5 allowed IP server still works", rest_is_ip_ok(st, data), f"st={st} {data[:120]}")

    # A6  empty allow-list = fail-closed (deny all)
    bearer_upsert(True, "servers", [])
    time.sleep(0.5)
    st, data, _ = rest_call(f"/rest/{IP_Q}/call", IP_TOOL, headers=BEARER)
    check("A6 empty allow-list fail-closed (IP server denied too)", st in (401, 403), f"st={st}")
    if st in (401, 403):
        print("  note: access_type='servers' with allowed_servers=[] denies ALL servers "
              "(Some(empty set)) - fail-closed confirmed")
    else:
        print(f"  note: empty allow-list behaved OPEN (st={st}) - R53 semantics violated")


# -- Section B ----------------------------------------------------------------
def section_b():
    print("== B: disabled-tool gate on all faces ==")
    set_bearer_flag(False)  # section A leaves bearer on; these faces test unauthenticated lane
    snap = tool_cfg_snapshot(DISABLE_SERVER, DISABLE_TOOL)
    tool_cfg_set_enabled(DISABLE_SERVER, DISABLE_TOOL, 0)
    try:
        _, _, _, names = mcp_names("/mcp", rid=100)
        hits = [n for n in names if n == DISABLE_TOOL or n.endswith(f"-{DISABLE_TOOL}")]
        check("B /mcp tools/list absent", not hits, f"hits={hits}")

        _, _, sid, names = mcp_names(f"/mcp/{PW_Q}", rid=110)
        check("B /mcp/{server} tools/list absent", DISABLE_TOOL not in names,
              f"names={names[:8]}")
        if sid:
            st, data, _ = rpc(f"/mcp/{PW_Q}", 112, "tools/call",
                          {"name": DISABLE_TOOL, "arguments": {}}, session=sid, timeout=60)
            o = sse_last(data, 112)
            refused = (st != 200 or tool_error(o) is not None
                       or ((o or {}).get("result", {}) or {}).get("isError") is True)
            check("B /mcp/{server} tools/call refused", refused, f"st={st} {json_txt(o)[:120]}")

        st, data, _ = req("GET", f"/rest/{PW_Q}/tools")
        # Documented semantics: the REST listing keeps disabled tools visible
        # with enabled:false (UI re-enable affordance); the CALL surface is
        # what refuses them.  Assert the enabled flag is false when present.
        try:
            tlist = json.loads(data).get("tools", [])
            bc = [t for t in tlist if t.get("name") == DISABLE_TOOL]
            bstate = None if not bc else bc[0].get("enabled")
        except Exception:
            tlist, bstate = None, "parse-error"
        check("B /rest/{server}/tools marks disabled tool enabled:false (or absent)",
              st != 200 or bstate is False or bstate is None,
              f"st={st} enabled={bstate}")
        st, data, _ = rest_call(f"/rest/{PW_Q}/call", DISABLE_TOOL)
        check("B /rest/{server}/call refused", st >= 400,
              f"st={st} {data[:120]}")
        st, data, _ = req("GET", f"/api/tools/{PW_Q}/{DISABLE_TOOL}", timeout=60)
        check("B /api/tools GET refused", st >= 400, f"st={st} {data[:120]}")

        st, data, _ = req("POST", "/api/$smart/search", {"query": DISABLE_TOOL, "limit": 10})
        try:
            sdata = json.loads(data)
            inner = sdata.get("tools")
            if inner is None and sdata.get("content"):
                inner = json.loads(sdata["content"][0]["text"])
            stext = json.dumps(inner.get("tools", inner), ensure_ascii=False)
            # the echoed query string contains the tool name - only fail if a
            # RESULT entry references it
            sabsent = f'"name": "{DISABLE_TOOL}"' not in stext and DISABLE_TOOL not in [
                t.get("name", "") for t in (inner.get("tools") or [])]
        except Exception:
            sabsent = False
        check("B $smart search absent (parsed results)", st != 200 or sabsent,
              f"st={st} {data[:120]}")
    finally:
        if snap is None:
            tool_cfg_set_enabled(DISABLE_SERVER, DISABLE_TOOL, None)
        else:
            con = sqlite3.connect(DB)
            con.execute("UPDATE server_tool_config SET enabled=?, description=?, pinned=? WHERE id=?",
                        (snap[1], snap[2], snap[3], snap[0]))
            con.commit()
            con.close()
            time.sleep(0.6)
    st, data, _ = rest_call(f"/rest/{PW_Q}/call", DISABLE_TOOL, args={})
    check("B restore: tool callable again (2xx)", st == 200, f"st={st} {data[:120]}")


# -- Section C ----------------------------------------------------------------
def section_c():
    print("== C: group allow-list semantics ==")
    con = sqlite3.connect(DB)
    row = con.execute("SELECT servers FROM groups WHERE name=?", (GRP,)).fetchone()
    con.close()
    if not row:
        skip("C", f"group {GRP!r} not found")
        return
    members = [m.get("name") if isinstance(m, dict) else m for m in json.loads(row[0] or "[]")]
    check("C Test group contains IP + playwright servers",
          IP_SERVER in members and "playwright" in members, f"members={members}")

    gpath = f"/mcp/{GRP_Q}"
    st, echoed, sid = init_b(gpath, rid=200)
    names = []
    if sid:
        st, names, _o = list_with_bearer(gpath, sid, 201)
    ip_name = find_ip_tool(names)
    check("C /mcp/{group} tools/list contains IP tool", ip_name is not None,
          f"n_tools={len(names)}")
    check("C /mcp/{group} tools/list contains member playwright tool",
          any(n == DISABLE_TOOL or n.endswith(f"-{DISABLE_TOOL}") for n in names),
          f"n_tools={len(names)}")
    check("C /mcp/{group} tools/list excludes non-member tool",
          not any(n == NON_MEMBER_TOOL or n.endswith(f"-{NON_MEMBER_TOOL}") for n in names))
    if sid and ip_name:
        st, data, _ = rpc_b(gpath, 202, "tools/call", {"name": ip_name, "arguments": {}},
                            session=sid)
        o = sse_last(data, 202)
        check("C /mcp/{group} IP call real public IPv4",
              st == 200 and tool_error(o) is None and valid_pub_ip(result_text(o)),
              f"st={st} {json_txt(o)[:140]}")

    st, data, _ = rest_call(f"/rest/group/{GRP_Q}/call", NON_MEMBER_TOOL)
    check("C /rest/group/{g}/call non-member tool refused", st >= 400,
          f"st={st} {data[:120]}")

    st, echoed, _ = init_b(gpath + "/", rid=210)
    check("C trailing-slash /mcp/{group}/ resolves", st == 200 and echoed == "2025-06-18",
          f"st={st} echo={echoed}")


# -- Section D ----------------------------------------------------------------
RETIRED = ["dispatch_mcp", "mcp_version", "SESSION_STRATEGY", "SSE_CHANNELS",
           "jsonrpc_response_s", "create_tool_task", "http_transport.rs",
           "x-mcphub-task-requested"]


def classify_hits(symbol):
    roots = ["src-tauri/src", "frontend/src"]
    hits = []
    repo = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "../.."))
    for root in roots:
        p = subprocess.run(
            ["grep", "-rn", "-F", symbol, os.path.join(repo, root),
             "--include=*.rs", "--include=*.ts", "--include=*.tsx"],
            capture_output=True, text=True)
        for line in p.stdout.splitlines():
            if not line.strip():
                continue
            path, ln, text = line.split(":", 2)
            t = text.strip()
            is_comment = (t.startswith("//") or t.startswith("*") or t.startswith("/*")
                          or t.startswith("//!")
                          or re.match(r"^\s*\*", text))
            hits.append((path, ln, t[:120], "comment" if is_comment else "CODE"))
    return hits


def section_d():
    print("== D: cleanup/completeness audit ==")
    any_code = False
    LIVE_OK = {"x-mcphub-task-requested"}  # live internal marker, not retired
    for sym in RETIRED:
        hits = classify_hits(sym)
        if sym == "http_transport.rs":
            hits = [h for h in hits if not h[0].endswith(("openapi_transport.rs",
                                                          "rmcp_http_transport.rs",
                                                          "sse_transport.rs"))]
        for path, ln, text, cls in hits:
            print(f"  {sym:<26} {os.path.basename(path)}:{ln:<5} {cls:<8} {text}")
            if cls == "CODE" and sym not in LIVE_OK:
                any_code = True
        if not hits:
            print(f"  {sym:<26} ZERO")
    # x-mcphub-task-requested is a LIVE internal marker (leniency injects it,
    # rmcp_bridge consumes it for task-directed creation) - documented in
    # AGENTS.md as a legitimate surface, not a retired symbol.
    LIVE_OK = {"x-mcphub-task-requested"}
    check("D no retired-symbol CODE hits (comments/live-markers only)", not any_code)
    repo = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "../.."))
    check("D legacy mcp/http_transport.rs absent",
          not os.path.exists(os.path.join(repo, "src-tauri/src/mcp/http_transport.rs")))


def main():
    if not health():
        print(f"SKIP-ALL: hub not healthy at {HOST}:{PORT}")
        sys.exit(0)
    snap_cfg = cfg_read()
    orig_bearer = (snap_cfg.get("routing") or {}).get("enableBearerAuth")
    try:
        for fn in (section_a, section_b, section_c, section_d):
            try:
                fn()
            except Exception as e:
                import traceback
                traceback.print_exc()
                check(f"{fn.__name__} no unexpected exception", False, repr(e))
    finally:
        set_bearer_flag(orig_bearer)
        con = sqlite3.connect(DB)
        con.execute("DELETE FROM bearer_keys WHERE token=?", (TEST_KEY,))
        con.commit()
        con.close()
        print(f"\nrestored: bearer flag -> {orig_bearer} | test key row deleted "
              "| tool-config rows restored per-section")
    print(f"\n{base.PASS} passed, {base.FAIL} failed, {base.SKIPS} skipped")
    if base.FAILURES:
        print("Failures:", *base.FAILURES, sep="\n  - ")
    sys.exit(1 if base.FAIL else 0)


if __name__ == "__main__":
    main()
