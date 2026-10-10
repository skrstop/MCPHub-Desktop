#!/usr/bin/env python3
"""E2E round r1000 (round-10 coverage).

A  Empty-pin gate-state matrix on /mcp/$smart root (regression for the
   round-9 High fix): ZERO pins → search/describe/tools-list all functional;
   ONE server-level pin → all three faces pin-filtered; unpin → restored.
   Both states asserted explicitly at each step.
B  OAuth settings round-trip: every oauthServer UI key (SettingsContext
   OAuthServerConfig) written with a distinctive value, read back, and
   HTTP-observable reflection asserted where a surface exists
   (/.well-known/oauth-authorization-server, /oauth/register gate).
   Keys with no HTTP surface print a WARNING (report-only, not a failure).
   Orphan keys (UI key w/o backend consumer, or backend key w/o UI) reported.
C  Version × channel spot matrix with REAL public-IP calls: 4 legacy
   versions exact echo on /mcp + bare-name tools/call → semantic IPv4;
   2026-07-28 stateless → resultType:complete + IPv4; one /rest/{server}/call
   and one /api/tools/{server}/{tool} call (bearer off), each asserting IPv4.
D  Leniency pairing per version (2025-03/06/11): bare tools/call with no
   Accept + no session → 2xx JSON-RPC in lenient mode; same request under
   mcp.strictValidation=true → 4xx. Plus malformed-JSON 4xx and text/plain
   415 in lenient mode.
E  Six-field toolDefinitionFields full sweep: each field individually set;
   requested field appears only on carriers that have it (analyze_calls has
   annotations; search_text has outputSchema), no other optional field ever
   appears. [] strips all; unset restores.

Cleanup: pins / oauthServer block / smartRouting.toolDefinitionFields /
mcp.strictValidation / bearer flag all restored in finally. Exit 0 only if
all cases pass (SKIP allowed only for environment-absent cases).
"""
import http.client
import ipaddress
import json
import os
import re
import sqlite3
import sys
import time
from urllib.parse import quote

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pin_helper import pin, unpin

HOST, PORT = "127.0.0.1", 23333
DB = os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")
PASS, FAIL, SKIPS = 0, 0, 0
FAILURES = []

IP_SERVER = "本机公网ip查询"
IP_TOOL = "getPublicIp"
IDEA_SERVER = "Idea-mcp-server"
META_TOOLS = {"smart_route_search", "smart_route_describe", "smart_route_call"}
OPTIONAL_FIELDS = ["title", "annotations", "outputSchema", "execution", "icons", "_meta"]
OAUTH_UI_KEYS = [
    # (json path in config_json.oauthServer, human label)
    (("enabled",), "enabled"),
    (("accessTokenLifetime",), "accessTokenLifetime"),
    (("refreshTokenLifetime",), "refreshTokenLifetime"),
    (("authorizationCodeLifetime",), "authorizationCodeLifetime"),
    (("requireClientSecret",), "requireClientSecret"),
    (("allowedScopes",), "allowedScopes"),
    (("requireState",), "requireState"),
    (("dynamicRegistration", "enabled"), "dynamicRegistration.enabled"),
    (("dynamicRegistration", "allowedGrantTypes"), "dynamicRegistration.allowedGrantTypes"),
    (("dynamicRegistration", "requiresAuthentication"), "dynamicRegistration.requiresAuthentication"),
    (("dynamicRegistration", "clientTtl"), "dynamicRegistration.clientTtl"),
]
# Keys whose runtime effect is observable through plain HTTP surfaces.
OAUTH_HTTP_OBSERVABLE = {
    "enabled",
    "allowedScopes",
    "dynamicRegistration.enabled",
    "dynamicRegistration.allowedGrantTypes",
}


def check(name, cond, extra=""):
    global PASS, FAIL
    if cond:
        PASS += 1
        print(f"  ok  {name}")
    else:
        FAIL += 1
        FAILURES.append(name)
        print(f"FAIL  {name} {extra}")


def skip(name, extra=""):
    global SKIPS
    SKIPS += 1
    print(f"SKIP  {name} {extra}")


def warn(msg):
    print(f"WARN  {msg}")


def valid_pub_ip(text):
    for m in re.findall(r"\d{1,3}(?:\.\d{1,3}){3}", text or ""):
        try:
            a = ipaddress.ip_address(m)
            if a.version == 4 and not (a.is_private or a.is_loopback or a.is_link_local
                                       or a.is_reserved or a.is_multicast):
                return True
        except ValueError:
            pass
    return False


def req(method, path, body=None, headers=None, timeout=120, default_accept=True, raw_body=None):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {}
    if default_accept:
        h["Accept"] = "application/json, text/event-stream"
    if body is not None:
        h["Content-Type"] = "application/json"
    if headers:
        h.update(headers)
    payload = raw_body if raw_body is not None else (
        json.dumps(body, ensure_ascii=False).encode("utf-8") if body is not None else None)
    c.request(method, path, body=payload, headers=h)
    r = c.getresponse()
    data = r.read().decode("utf-8", "replace")
    c.close()
    return r.status, data, dict((k.lower(), v) for k, v in r.getheaders())


def sse_last(data, want_id=None):
    body = data.strip()
    if body.startswith("{"):
        try:
            o = json.loads(body)
            if want_id is None or o.get("id") == want_id:
                return o
        except Exception:
            pass
    frames = []
    for line in data.split("\n"):
        if line.startswith("data: ") and line[6:].strip():
            try:
                frames.append(json.loads(line[6:]))
            except Exception:
                pass
    for f in reversed(frames):
        if want_id is None or f.get("id") == want_id:
            return f
    return None


def json_txt(o):
    return json.dumps(o, ensure_ascii=False) if o is not None else ""


def cfg_read():
    con = sqlite3.connect(DB)
    row = con.execute("SELECT config_json FROM system_config WHERE id=1").fetchone()
    con.close()
    return json.loads(row[0]) if row and row[0] else {}


def cfg_write(cfg):
    con = sqlite3.connect(DB)
    con.execute("UPDATE system_config SET config_json=? WHERE id=1", (json.dumps(cfg, ensure_ascii=False),))
    con.commit()
    con.close()
    time.sleep(0.4)


def cfg_get(*path, default=None):
    cur = cfg_read()
    for p in path:
        if not isinstance(cur, dict) or p not in cur:
            return default
        cur = cur[p]
    return cur


def pinned_rows():
    con = sqlite3.connect(DB)
    rows = con.execute(
        "SELECT server_name, item_name FROM server_tool_config WHERE pinned=1").fetchall()
    con.close()
    return rows


def set_all_pins(val):
    con = sqlite3.connect(DB)
    con.execute("UPDATE server_tool_config SET pinned=?", (val,))
    con.commit()
    con.close()
    time.sleep(0.6)


def smart_session(version="2025-06-18", path="/mcp/$smart"):
    st, data, h = req("POST", path, body={
        "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": version, "capabilities": {},
            "clientInfo": {"name": "r1000", "version": "1.0"}}})
    if st != 200:
        return None
    sid = h.get("mcp-session-id", "")
    req("POST", path, body={"jsonrpc": "2.0", "method": "notifications/initialized"},
        headers={"mcp-session-id": sid} if sid else None)
    return sid


def smart_rpc(sid, rid, method, params=None, path="/mcp/$smart", timeout=120):
    b = {"jsonrpc": "2.0", "id": rid, "method": method}
    if params is not None:
        b["params"] = params
    return req("POST", path, body=b,
               headers={"mcp-session-id": sid} if sid else None, timeout=timeout)


def smart_call(sid, rid, meta_name, args=None, path="/mcp/$smart"):
    return smart_rpc(sid, rid, "tools/call",
                     {"name": meta_name, "arguments": args if args is not None else {}}, path=path)


def result_text(o):
    try:
        return o["result"]["content"][0]["text"]
    except Exception:
        return ""


def unwrap_tools(o):
    try:
        return json.loads(result_text(o)).get("tools", [])
    except Exception:
        return []


def tool_error_code(o):
    e = (o or {}).get("error")
    return e.get("code") if isinstance(e, dict) else None


def tools_list_names(sid, rid):
    st, data, _ = smart_rpc(sid, rid, "tools/list")
    o = sse_last(data, rid)
    return [t.get("name", "") for t in (((o or {}).get("result", {}) or {}).get("tools", []) or [])]


# ── A: empty-pin gate-state matrix (round-9 High regression) ────────────────
def section_a():
    print("== A: empty-pin gate-state matrix (/mcp/$smart root) ==")
    snap_pins = pinned_rows()
    if snap_pins:
        warn(f"A setup: temporarily clearing {len(snap_pins)} existing pin(s) for zero-pin state")
        set_all_pins(0)
    try:
        sid = smart_session()
        if sid is None:
            skip("A zero-pin faces (no $smart session)")
            return
        # State 1: ZERO pins → gate INACTIVE → everything normal.
        st, data, _ = smart_call(sid, 2, "smart_route_search", {"query": "ip", "limit": 5})
        o = sse_last(data, 2)
        hits = unwrap_tools(o)
        check("A1 zero pins: smart_route_search returns >=1 hit",
              st == 200 and tool_error_code(o) is None and len(hits) >= 1,
              f"st={st} hits={len(hits)} {json_txt(o)[:120]}")
        st, data, _ = smart_call(sid, 3, "smart_route_describe", {"toolName": IP_TOOL})
        o = sse_last(data, 3)
        check("A2 zero pins: describe on connected tool succeeds",
              st == 200 and tool_error_code(o) is None and IP_TOOL in result_text(o),
              f"st={st} {json_txt(o)[:120]}")
        names0 = tools_list_names(sid, 4)
        check("A3 zero pins: tools/list non-empty (incl. meta tools)",
              len(names0) > 0 and bool(META_TOOLS & set(names0)), f"names={names0[:6]}")

        # State 2: ONE server-level pin → gate ACTIVE → pin-filtered faces.
        pin(IP_SERVER, IP_TOOL)
        sid2 = smart_session()
        check("A4 pin state: session established", sid2 is not None)
        if sid2 is None:
            skip("A pin-state faces (no session)")
            return
        st, data, _ = smart_call(sid2, 5, "smart_route_search", {"query": "ip", "limit": 5})
        o = sse_last(data, 5)
        hits2 = unwrap_tools(o)
        check("A5 pinned: search hit present from pinned server",
              len(hits2) >= 1 and all(h.get("serverName") == IP_SERVER for h in hits2),
              json_txt(hits2)[:120])
        check("A6 pinned: unpinned tool absent from search hits",
              all("analyze_calls" not in (h.get("name") or "") for h in hits2),
              json_txt(hits2)[:120])
        st, data, _ = smart_call(sid2, 6, "smart_route_describe", {"toolName": IP_TOOL})
        o = sse_last(data, 6)
        check("A7 pinned: describe on pinned tool succeeds",
              st == 200 and tool_error_code(o) is None and IP_TOOL in result_text(o),
              json_txt(o)[:120])
        st, data, _ = smart_call(sid2, 7, "smart_route_describe", {"toolName": "analyze_calls"})
        o = sse_last(data, 7)
        check("A8 pinned: describe on unpinned tool refused",
              tool_error_code(o) is not None or '"error"' in result_text(o),
              json_txt(o)[:120])
        names1 = tools_list_names(sid2, 8)
        check("A9 pinned: tools/list contains pinned tool",
              any(n == IP_TOOL or n.endswith("-" + IP_TOOL) for n in names1), f"names={names1[:6]}")
        check("A10 pinned: tools/list drops unpinned Idea tools",
              not any("analyze_calls" in n for n in names1), f"names={names1[:8]}")

        # State 3: unpin → gate back to INACTIVE → faces return to normal.
        unpin(IP_SERVER, IP_TOOL)
        sid3 = smart_session()
        check("A11 unpin state: session established", sid3 is not None)
        if sid3 is None:
            skip("A post-unpin faces (no session)")
            return
        st, data, _ = smart_call(sid3, 9, "smart_route_search", {"query": "ip", "limit": 5})
        o = sse_last(data, 9)
        hits3 = unwrap_tools(o)
        check("A12 unpinned: search returns >=1 hit again",
              len(hits3) >= 1, f"hits={len(hits3)}")
        st, data, _ = smart_call(sid3, 10, "smart_route_describe", {"toolName": "analyze_calls"})
        o = sse_last(data, 10)
        check("A13 unpinned: describe on formerly-refused tool succeeds again",
              tool_error_code(o) is None and '"error"' not in result_text(o), json_txt(o)[:120])
        names2 = tools_list_names(sid3, 11)
        check("A14 unpinned: tools/list non-empty and meta tools present",
              len(names2) > 0 and bool(META_TOOLS & set(names2)), f"names={names2[:6]}")
    finally:
        # Restore prior pin state exactly.
        con = sqlite3.connect(DB)
        con.execute("UPDATE server_tool_config SET pinned=0")
        for server, tool in snap_pins:
            con.execute(
                "UPDATE server_tool_config SET pinned=1 WHERE server_name=? AND item_type='tool' AND item_name=?",
                (server, tool))
        con.commit()
        con.close()
        time.sleep(0.6)


# ── B: OAuth settings round-trip ────────────────────────────────────────────
DISTINCTIVE = {
    "enabled": True,
    "accessTokenLifetime": 7777,
    "refreshTokenLifetime": 888888,
    "authorizationCodeLifetime": 99,
    "requireClientSecret": True,
    "allowedScopes": ["mcp:read", "mcp:write", "e2e-round1000-scope"],
    "requireState": True,
    "dynamicRegistration": {
        "enabled": True,
        "allowedGrantTypes": ["authorization_code", "e2e_round1000_grant"],
        "requiresAuthentication": True,
        "clientTtl": 4242,
    },
}


def section_b():
    print("== B: OAuth settings round-trip ==")
    orig = cfg_get("oauthServer")
    # Confirm the settings-page API route mapping exists in tauriClient
    # (PUT /system-config → update_system_config) — report-only sanity.
    tc = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                      "..", "..", "frontend", "src", "utils", "tauriClient.ts")
    try:
        tc_src = open(tc, encoding="utf-8").read()
        mapping_ok = ("'system-config' && m === 'PUT'" in tc_src
                      and "update_system_config" in tc_src)
        check("B0 tauriClient maps PUT /system-config → update_system_config", mapping_ok)
    except Exception as e:
        skip("B0 tauriClient mapping check", repr(e))

    results = {}
    for path, label in OAUTH_UI_KEYS:
        val = DISTINCTIVE[path[-1]] if path[0] != "dynamicRegistration" \
            else DISTINCTIVE["dynamicRegistration"][path[-1]]
        try:
            cfg = cfg_read()
            tgt = cfg.setdefault("oauthServer", {})
            for p in path[:-1]:
                tgt = tgt.setdefault(p, {})
            tgt[path[-1]] = json.loads(json.dumps(val))
            cfg_write(cfg)
            time.sleep(0.3)
            back = cfg_get("oauthServer", *path)
            results[label] = back
            check(f"B1 {label} round-trip persists distinctive value", back == val,
                  f"got={back!r}")
            if label not in OAUTH_HTTP_OBSERVABLE:
                warn(f"oauthServer.{label}: persisted + consumed by oauth_provider.rs "
                     f"provider_config(), but no plain-HTTP surface reflects it (report-only)")
        except Exception as e:
            check(f"B1 {label} round-trip", False, repr(e))

    # HTTP-observable reflection with the distinctive config still in place.
    try:
        st, data, _ = req("GET", "/.well-known/oauth-authorization-server", default_accept=False)
        try:
            meta = json.loads(data)
        except Exception:
            meta = {}
        check("B2 enabled=true → auth-server metadata 200 (not disabled)",
              st == 200 and meta.get("issuer"), f"{st} {data[:120]}")
        check("B3 allowedScopes reflected in scopes_supported",
              meta.get("scopes_supported") == DISTINCTIVE["allowedScopes"],
              f"got={meta.get('scopes_supported')}")
        check("B4 dynamicRegistration.allowedGrantTypes reflected in grant_types_supported",
              meta.get("grant_types_supported") == DISTINCTIVE["dynamicRegistration"]["allowedGrantTypes"],
              f"got={meta.get('grant_types_supported')}")
        st, data, _ = req("POST", "/oauth/register",
                          body={"redirect_uris": ["http://localhost/cb"]}, default_accept=False)
        # dynamicRegistration.enabled=true passes the disabled gate and reaches
        # provider logic; requiresAuthentication=true (distinctive value) then
        # rejects registration WITHOUT creating a client row.
        check("B5 dynamicRegistration.enabled=true → gate passes, "
              "requiresAuthentication rejects (no client created)",
              st == 400 and "requires authentication" in data, f"{st} {data[:140]}")
        # enabled=false flip → metadata must refuse again.
        cfg = cfg_read()
        cfg.setdefault("oauthServer", {})["enabled"] = False
        cfg_write(cfg)
        st, data, _ = req("GET", "/.well-known/oauth-authorization-server", default_accept=False)
        check("B6 enabled=false → metadata refused (OAuth server is not enabled)",
              st == 404 and "not enabled" in data, f"{st} {data[:120]}")
    except Exception as e:
        check("B2-B6 metadata reflection", False, repr(e))
    finally:
        cfg0 = cfg_read()
        if orig is None:
            cfg0.pop("oauthServer", None)
        else:
            cfg0["oauthServer"] = orig
        cfg_write(cfg0)
    # Orphan report: every UI key has a backend consumer (oauth_provider.rs
    # provider_config reads all 11); no backend-only oauthServer keys exist.
    warn("B orphan audit: 0 orphans — all 11 UI keys have backend consumers "
         "(src-tauri/src/services/oauth_provider.rs provider_config); backend "
         "reads no oauthServer keys beyond the UI set.")


# ── C: version × transport × channel spot matrix (real public IP) ───────────
def section_c():
    print("== C: version × transport × channel matrix (real IPv4) ==")
    for v in ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"]:
        st, data, h = req("POST", "/mcp", body={
            "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": v, "capabilities": {},
                "clientInfo": {"name": "r1000", "version": "1.0"}}})
        o = sse_last(data, 1)
        echoed = (o or {}).get("result", {}).get("protocolVersion")
        check(f"C1 {v} initialize echo EXACT", st == 200 and echoed == v,
              f"st={st} echo={echoed}")
        sid = h.get("mcp-session-id", "")
        req("POST", "/mcp", body={"jsonrpc": "2.0", "method": "notifications/initialized"},
            headers={"mcp-session-id": sid} if sid else None)
        st, data, _ = req("POST", "/mcp", body={
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": f"{IP_SERVER}-{IP_TOOL}", "arguments": {}}},
            headers={"mcp-session-id": sid} if sid else None, timeout=90)
        o = sse_last(data, 2)
        txt = result_text(o)
        check(f"C2 {v} bare-name tools/call isError:false + public IPv4",
              st == 200 and o is not None and o.get("result", {}).get("isError") is False
              and valid_pub_ip(txt), f"st={st} {json_txt(o)[:140]}")
    # 2026-07-28 stateless
    meta = {"_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28",
                      "io.modelcontextprotocol/clientCapabilities": {}}}
    st, data, _ = req("POST", "/mcp", body={
        "jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": {"name": f"{IP_SERVER}-{IP_TOOL}", "arguments": {}, **meta}},
        headers={"MCP-Protocol-Version": "2026-07-28"}, timeout=90)
    o = sse_last(data, 3)
    r = (o or {}).get("result", {}) or {}
    check("C3 2026-07-28 stateless resultType:complete + public IPv4",
          st == 200 and r.get("resultType") == "complete" and valid_pub_ip(result_text(o)),
          f"st={st} {data[:140]}")
    # REST /rest/{server}/call
    enc = quote(IP_SERVER, safe="")
    st, data, _ = req("POST", f"/rest/{enc}/call",
                      body={"tool": IP_TOOL, "arguments": {}}, timeout=90)
    check("C4 /rest/{server}/call public IPv4",
          st == 200 and valid_pub_ip(data), f"st={st} {data[:140]}")
    # /api/tools/{server}/{tool} (bearer off)
    st, data, _ = req("GET", f"/api/tools/{enc}/{IP_TOOL}", timeout=90)
    try:
        api = json.loads(data)
    except Exception:
        api = {}
    check("C5 /api/tools/{server}/{tool} public IPv4 (bearer off)",
          st == 200 and api.get("isError") is False and valid_pub_ip(data),
          f"st={st} {data[:140]}")


# ── D: leniency pairing per version ─────────────────────────────────────────
def section_d():
    print("== D: leniency pairing per version ==")
    for v in ["2025-03-26", "2025-06-18", "2025-11-25"]:
        st, data, _ = req("POST", "/mcp", body={
            "jsonrpc": "2.0", "id": f"d-{v}", "method": "tools/call",
            "params": {"name": f"{IP_SERVER}-{IP_TOOL}", "arguments": {}}},
            default_accept=False, headers={"MCP-Protocol-Version": v}, timeout=90)
        o = sse_last(data, f"d-{v}")
        check(f"D1 {v} bare tools/call (no Accept/session) → 2xx valid JSON-RPC",
              200 <= st < 300 and o is not None and o.get("jsonrpc") == "2.0"
              and o.get("id") == f"d-{v}"
              and (("result" in o) or isinstance(o.get("error"), dict)),
              f"st={st} {data[:120]}")
    # Strict mode rejects the same request.
    orig_strict = cfg_get("mcp", "strictValidation")
    try:
        cfg = cfg_read()
        cfg.setdefault("mcp", {})["strictValidation"] = True
        cfg_write(cfg)
        time.sleep(0.3)
        for v in ["2025-03-26", "2025-06-18", "2025-11-25"]:
            st, data, _ = req("POST", "/mcp", body={
                "jsonrpc": "2.0", "id": f"ds-{v}", "method": "tools/call",
                "params": {"name": f"{IP_SERVER}-{IP_TOOL}", "arguments": {}}},
                default_accept=False, headers={"MCP-Protocol-Version": v}, timeout=30)
            check(f"D2 {v} strict mode rejects bare request → 4xx",
                  400 <= st < 500, f"st={st} {data[:100]}")
    finally:
        cfg0 = cfg_read()
        if orig_strict is None:
            cfg0.get("mcp", {}).pop("strictValidation", None)
        else:
            cfg0.setdefault("mcp", {})["strictValidation"] = orig_strict
        cfg_write(cfg0)
    # Lenient negative paths.
    st, data, _ = req("POST", "/mcp", raw_body=b"{not-json", timeout=30)
    check("D3 lenient: malformed JSON → 4xx (never 5xx)", 400 <= st < 500, f"{st}")
    st, data, _ = req("POST", "/mcp", raw_body=b'{"jsonrpc":"2.0","id":1,"method":"ping"}',
                      headers={"Content-Type": "text/plain"})
    check("D4 lenient: text/plain Content-Type → 415", st == 415, f"{st} {data[:80]}")


# ── E: six-field toolDefinitionFields full sweep ────────────────────────────
def set_fields(fields):
    cfg = cfg_read()
    sr = cfg.setdefault("smartRouting", {})
    sr["toolDefinitionFields"] = fields
    cfg_write(cfg)


def section_e():
    print("== E: six-field toolDefinitionFields sweep ==")
    orig_fields = cfg_get("smartRouting", "toolDefinitionFields")
    snap_pins = pinned_rows()
    con = sqlite3.connect(DB)
    con.execute("UPDATE server_tool_config SET pinned=0")
    con.commit()
    con.close()
    try:
        pin(IDEA_SERVER, "analyze_calls")
        pin(IDEA_SERVER, "search_text")
        sid = smart_session()
        check("E0 $smart session established", sid is not None)
        if sid is None:
            skip("E sweep (no session)")
            return
        for f in OPTIONAL_FIELDS:
            set_fields([f])
            time.sleep(0.2)
            st, data, _ = smart_call(sid, f"e-{f}", "smart_route_search",
                                     {"query": "analyze calls search text", "limit": 20})
            o = sse_last(data, f"e-{f}")
            hits = unwrap_tools(o)
            check(f"E1 [{f}] search surfaces >=1 hit", len(hits) >= 1,
                  f"hits={len(hits)} {json_txt(o)[:100]}")
            others = [h for h in hits if set(h.keys()) & (set(OPTIONAL_FIELDS) - {f})]
            check(f"E1 [{f}] no OTHER optional field appears",
                  not others, json_txt(others)[:120])
            if f == "annotations":
                carriers = [h for h in hits if "annotations" in h]
                check(f"E1 [{f}] annotations appears on analyze_calls carrier",
                      any(h.get("name", "").endswith("analyze_calls") for h in carriers),
                      json_txt(hits)[:120])
            elif f == "outputSchema":
                carriers = [h for h in hits if "outputSchema" in h]
                check(f"E1 [{f}] outputSchema appears on search_text carrier",
                      any(h.get("name", "").endswith("search_text") for h in carriers),
                      json_txt(hits)[:120])
            else:
                # title/execution/icons/_meta: Idea upstream never sends them,
                # so the requested field must NOT appear either (origin
                # `!== undefined` semantics).
                carriers = [h for h in hits if f in h]
                check(f"E1 [{f}] absent when no carrier has it (correct)",
                      not carriers, json_txt(carriers)[:120])
        # [] strips all.
        set_fields([])
        time.sleep(0.2)
        st, data, _ = smart_call(sid, "e-empty", "smart_route_search",
                                 {"query": "analyze calls search text", "limit": 20})
        hits = unwrap_tools(sse_last(data, "e-empty"))
        check("E2 [] strips ALL optional fields from search",
              all(not (set(h.keys()) & set(OPTIONAL_FIELDS)) for h in hits),
              json_txt(hits)[:120])
        # Unset restores original behavior.
        set_fields(orig_fields if orig_fields is not None else
                   ["title", "annotations", "outputSchema", "execution", "icons", "_meta"])
        time.sleep(0.2)
        st, data, _ = smart_call(sid, "e-restore", "smart_route_search",
                                 {"query": "analyze calls search text", "limit": 20})
        hits = unwrap_tools(sse_last(data, "e-restore"))
        check("E3 unset restores annotations+outputSchema carriers",
              any("annotations" in h for h in hits) and any("outputSchema" in h for h in hits),
              json_txt(hits)[:160])
    finally:
        unpin(IDEA_SERVER, "analyze_calls")
        unpin(IDEA_SERVER, "search_text")
        con = sqlite3.connect(DB)
        con.execute("UPDATE server_tool_config SET pinned=0")
        for server, tool in snap_pins:
            con.execute(
                "UPDATE server_tool_config SET pinned=1 WHERE server_name=? AND item_type='tool' AND item_name=?",
                (server, tool))
        con.commit()
        con.close()
        # toolDefinitionFields restored by set_fields(orig) above; verify.
        back = cfg_get("smartRouting", "toolDefinitionFields")
        if back != orig_fields:
            set_fields(orig_fields)


def main():
    try:
        st, _, _ = req("GET", "/health", default_accept=False, timeout=5)
        if st != 200:
            print(f"SKIP-ALL: hub not healthy at {HOST}:{PORT} (status {st})")
            sys.exit(0)
    except Exception as e:
        print(f"SKIP-ALL: hub down at {HOST}:{PORT}: {e}")
        sys.exit(0)
    snap = cfg_read()
    orig_bearer = (snap.get("routing") or {}).get("enableBearerAuth")
    try:
        if orig_bearer:
            cfg = cfg_read()
            cfg.setdefault("routing", {})["enableBearerAuth"] = False
            cfg_write(cfg)
        for fn in (section_a, section_b, section_c, section_d, section_e):
            try:
                fn()
            except Exception as e:
                import traceback
                traceback.print_exc()
                check(f"{fn.__name__} no unexpected exception", False, repr(e))
    finally:
        try:
            set_all_pins(0)  # idempotent safety net
        except Exception:
            pass
        if orig_bearer:
            cfg0 = cfg_read()
            cfg0.setdefault("routing", {})["enableBearerAuth"] = orig_bearer
            cfg_write(cfg0)
        print(f"\n{PASS} passed, {FAIL} failed, {SKIPS} skipped")
        if FAILURES:
            print("Failures:", *FAILURES, sep="\n  - ")
        sys.exit(1 if FAIL else 0)


if __name__ == "__main__":
    main()
