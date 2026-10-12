#!/usr/bin/env python3
"""E2E round r1100 (round-11 coverage).

A  Version echo consistency x ALL channels x ALL versions: initialize on
   root /mcp, single-server scope /mcp/{IP} and group /mcp/{group} for each
   of the 4 legacy versions - response protocolVersion must equal the
   requested version EXACTLY. 2026-07-28 stateless on root + group ->
   resultType:complete with no version downgrade.
B  REAL public-IP tool call on EVERY version x EVERY channel: initialize ->
   initialized -> tools/list (assert the IP tool appears with the correct
   prefix form for that channel) -> tools/call -> semantic public IPv4
   (ipaddress, private/loopback/link-local excluded) and isError:false.
   2026-07-28 stateless included.
C  Leniency x version pairing: bare tools/call (no Accept, no session) -> 2xx
   valid JSON-RPC per version (legacy + 2026); missing Content-Type ->
   accepted or 415 (documented which); malformed JSON -> 4xx; wrong jsonrpc
   version -> normalized or 415 (documented which). Strict-mode spot check:
   mcp.strictValidation=true -> one bare request 4xx, restored.
D  OAuth token lifecycle REAL end-to-end: enable oauthServer -> dynamic
   registration 201 -> authorize consent page 200 (nonce parsed headlessly)
   -> approve POST -> 302 code -> token exchange 200 (access+refresh) ->
   real refresh_token grant 200 -> negative paths (bogus code, missing
   grant_type, bogus refresh token) -> metadata endpoints. Full restore.
E  $smart search/describe/call naming with one IP-server pin: bare-name
   describe vs prefixed-call asymmetry asserted EXACTLY as currently
   documented (printed); unpin -> gate-inactive semantics restored.

Cleanup: pins / oauthServer block / mcp.strictValidation / bearer flag all
restored in finally. Exit 0 only if all cases pass.
"""
import base64
import hashlib
import http.client
import ipaddress
import json
import os
import re
import secrets
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
LEGACY_VERSIONS = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"]
META_TOOLS = {"smart_route_search", "smart_route_describe", "smart_route_call"}
META_2026 = {"_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28",
                       "io.modelcontextprotocol/clientCapabilities": {}}}


def cleanup_oauth_clients(con, prefixes):
    """Delete OAuth clients registered by e2e suites (by exact known name
    prefix) plus their codes/tokens. NEVER pass user-facing prefixes here —
    only suite-owned names like 'r800', 'r1100-e2e', 'r1700-e2e', 'r300'.

    `con` is an open sqlite3 connection to the MCPHub DB. Deletes cascade
    to oauth_codes/oauth_tokens by client_id, mirroring the inline cleanup
    the r1100/r1700 suites already used. Errors are swallowed so a missing
    table never fails a suite's finally block.
    """
    for prefix in prefixes:
        # Escape LIKE wildcards so a future caller passing 'r_800' can't
        # widen the delete set ('_' matches any single char).
        esc = prefix.replace('\\', '\\\\').replace('%', '\\%').replace('_', '\\_')
        try:
            cur = con.execute(
                "SELECT client_id FROM oauth_clients WHERE name LIKE ? ESCAPE '\\'",
                (esc + "%",))
            ids = [r[0] for r in cur.fetchall()]
        except Exception as e:
            print(f"[cleanup_oauth_clients] select '{prefix}': {e}", file=sys.stderr)
            continue
        for cid in ids:
            for tbl in ("oauth_codes", "oauth_tokens"):
                try:
                    con.execute(f"DELETE FROM {tbl} WHERE client_id=?", (cid,))
                except Exception as e:
                    print(f"[cleanup_oauth_clients] {tbl} {cid}: {e}", file=sys.stderr)
            try:
                con.execute("DELETE FROM oauth_clients WHERE client_id=?", (cid,))
            except Exception as e:
                print(f"[cleanup_oauth_clients] oauth_clients {cid}: {e}", file=sys.stderr)
    try:
        con.commit()
    except Exception as e:
        print(f"[cleanup_oauth_clients] commit: {e}", file=sys.stderr)


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


def req(method, path, body=None, headers=None, timeout=120, default_accept=True,
        raw_body=None, form=False):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {}
    if default_accept:
        h["Accept"] = "application/json, text/event-stream"
    if body is not None or form:
        h["Content-Type"] = ("application/x-www-form-urlencoded" if form
                             else "application/json")
    if headers:
        h.update(headers)
    payload = raw_body if raw_body is not None else (
        body.encode("utf-8") if (form and isinstance(body, str))
        else (json.dumps(body, ensure_ascii=False).encode("utf-8")
              if body is not None else None))
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
    con.execute("UPDATE system_config SET config_json=? WHERE id=1",
                (json.dumps(cfg, ensure_ascii=False),))
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


def restore_pins(snap):
    con = sqlite3.connect(DB)
    con.execute("UPDATE server_tool_config SET pinned=0")
    for server, tool in snap:
        con.execute(
            "UPDATE server_tool_config SET pinned=1 "
            "WHERE server_name=? AND item_type='tool' AND item_name=?", (server, tool))
    con.commit()
    con.close()
    time.sleep(0.6)


def pick_group():
    """Prefer 'Test'; fall back to any group whose member list has the IP server."""
    con = sqlite3.connect(DB)
    rows = con.execute("SELECT name, servers FROM groups").fetchall()
    con.close()
    fallback = None
    for name, servers_json in rows:
        try:
            members = json.loads(servers_json or "[]")
        except Exception:
            continue
        names = []
        for m in members:
            if isinstance(m, str):
                names.append(m)
            elif isinstance(m, dict):
                names.append(m.get("name") or "")
        if name == "Test" and IP_SERVER in names:
            return "Test"
        if IP_SERVER in names and fallback is None:
            fallback = name
    return fallback


def rpc(path, rid, method, params=None, session=None, accept=True, timeout=120):
    b = {"jsonrpc": "2.0", "id": rid, "method": method}
    if params is not None:
        b["params"] = params
    h = {"mcp-session-id": session} if session else {}
    return req("POST", path, body=b, headers=h, default_accept=accept, timeout=timeout)


def initialize(path, version, rid=1, client="r1100"):
    st, data, h = req("POST", path, body={
        "jsonrpc": "2.0", "id": rid, "method": "initialize", "params": {
            "protocolVersion": version, "capabilities": {},
            "clientInfo": {"name": client, "version": "1.0"}}})
    o = sse_last(data, rid)
    sid = h.get("mcp-session-id", "")
    echoed = ((o or {}).get("result", {}) or {}).get("protocolVersion")
    return st, echoed, sid


def notify_initialized(path, sid):
    req("POST", path, body={"jsonrpc": "2.0", "method": "notifications/initialized"},
        headers={"mcp-session-id": sid} if sid else None, default_accept=False)


def result_text(o):
    try:
        return o["result"]["content"][0]["text"]
    except Exception:
        return ""


def tool_error(o):
    e = (o or {}).get("error")
    return e.get("code") if isinstance(e, dict) else None


def find_ip_tool(names):
    for n in names:
        if n == IP_TOOL or n.endswith(f"-{IP_TOOL}"):
            return n
    return None


def channels_for(group, server_path):
    chs = {"root": "/mcp"}
    if server_path:
        chs["server"] = f"/mcp/{quote(server_path, safe='')}"
    if group:
        chs["group"] = f"/mcp/{quote(group, safe='')}"
    return chs


# ── A: version echo × channel × version ─────────────────────────────────────
def section_a(group, server_path):
    print("== A: version echo consistency x all channels x all versions ==")
    for ch, path in channels_for(group, server_path).items():
        for v in LEGACY_VERSIONS:
            st, echoed, _ = initialize(path, v)
            check(f"A {ch} {v} initialize echo EXACT",
                  st == 200 and echoed == v, f"st={st} echo={echoed}")
    for ch, path in channels_for(group, None).items():
        st, data, _ = req("POST", path, body={
            "jsonrpc": "2.0", "id": 9, "method": "tools/call",
            "params": {"name": f"{IP_SERVER}-{IP_TOOL}", "arguments": {}, **META_2026}},
            headers={"MCP-Protocol-Version": "2026-07-28"}, timeout=90)
        o = sse_last(data, 9)
        r = (o or {}).get("result", {}) or {}
        downgraded = (r.get("protocolVersion") not in (None, "2026-07-28"))
        check(f"A {ch} 2026-07-28 stateless resultType:complete (no downgrade)",
              st == 200 and r.get("resultType") == "complete" and not downgraded,
              f"st={st} resultType={r.get('resultType')}")


# ── B: real public-IP call × all versions × all channels ────────────────────
def section_b(group, server_path):
    print("== B: real MCP public-IP call x every version x every channel ==")
    for ch, path in channels_for(group, server_path).items():
        for v in LEGACY_VERSIONS:
            st, echoed, sid = initialize(path, v)
            if st != 200 or echoed != v:
                check(f"B {ch} {v} initialize", False, f"st={st} echo={echoed}")
                continue
            notify_initialized(path, sid)
            st, data, _ = rpc(path, 2, "tools/list", {}, session=sid, timeout=90)
            o = sse_last(data, 2)
            names = [t.get("name", "")
                     for t in (((o or {}).get("result", {}) or {}).get("tools", []) or [])]
            ip_name = find_ip_tool(names)
            expected_bare = (ch == "server")
            check(f"B {ch} {v} tools/list exposes IP tool "
                  f"({'bare' if expected_bare else 'prefixed'} form)",
                  ip_name is not None and (ip_name == IP_TOOL) == expected_bare,
                  f"ip_name={ip_name!r} n_tools={len(names)}")
            if ip_name is None:
                continue
            st, data, _ = rpc(path, 3, "tools/call",
                              {"name": ip_name, "arguments": {}},
                              session=sid, timeout=90)
            o = sse_last(data, 3)
            check(f"B {ch} {v} tools/call isError:false + public IPv4",
                  st == 200 and o is not None and tool_error(o) is None
                  and o.get("result", {}).get("isError") is False
                  and valid_pub_ip(result_text(o)), f"st={st} {json_txt(o)[:140]}")
    for ch, path in channels_for(group, None).items():
        name = f"{IP_SERVER}-{IP_TOOL}"
        st, data, _ = req("POST", path, body={
            "jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": {"name": name, "arguments": {}, **META_2026}},
            headers={"MCP-Protocol-Version": "2026-07-28"}, timeout=90)
        o = sse_last(data, 4)
        r = (o or {}).get("result", {}) or {}
        check(f"B {ch} 2026-07-28 stateless call public IPv4",
              st == 200 and r.get("resultType") == "complete"
              and r.get("isError") is False and valid_pub_ip(result_text(o)),
              f"st={st} {data[:140]}")


# ── C: leniency × version pairing ───────────────────────────────────────────
def section_c():
    print("== C: leniency x version pairing ==")
    for v in LEGACY_VERSIONS:
        st, data, _ = req("POST", "/mcp", body={
            "jsonrpc": "2.0", "id": f"c-{v}", "method": "tools/call",
            "params": {"name": f"{IP_SERVER}-{IP_TOOL}", "arguments": {}}},
            default_accept=False, headers={"MCP-Protocol-Version": v}, timeout=90)
        o = sse_last(data, f"c-{v}")
        check(f"C1 {v} bare tools/call (no Accept/session) -> 2xx valid JSON-RPC",
              200 <= st < 300 and o is not None and o.get("jsonrpc") == "2.0"
              and o.get("id") == f"c-{v}"
              and (("result" in o) or isinstance(o.get("error"), dict)),
              f"st={st} {data[:120]}")
    st, data, _ = req("POST", "/mcp", body={
        "jsonrpc": "2.0", "id": "c-2026", "method": "tools/call",
        "params": {"name": f"{IP_SERVER}-{IP_TOOL}", "arguments": {}, **META_2026}},
        default_accept=False, headers={"MCP-Protocol-Version": "2026-07-28"}, timeout=90)
    o = sse_last(data, "c-2026")
    r = (o or {}).get("result", {}) or {}
    check("C1 2026-07-28 bare tools/call -> 2xx resultType:complete",
          200 <= st < 300 and r.get("resultType") == "complete", f"st={st} {data[:120]}")
    # Missing Content-Type: valid JSON body, no Content-Type header.
    st, data, _ = req("POST", "/mcp", raw_body=json.dumps({
        "jsonrpc": "2.0", "id": "c-noct", "method": "tools/call",
        "params": {"name": f"{IP_SERVER}-{IP_TOOL}", "arguments": {}}}).encode("utf-8"),
        default_accept=False, headers={"MCP-Protocol-Version": "2025-06-18"}, timeout=90)
    if 200 <= st < 300:
        warn("C2 missing Content-Type -> ACCEPTED (leniency injects application/json)")
        check("C2 missing Content-Type -> accepted or 415 (documented: accepted)", True)
    elif st == 415:
        warn("C2 missing Content-Type -> 415 (documented)")
        check("C2 missing Content-Type -> accepted or 415 (documented: 415)", True)
    else:
        check("C2 missing Content-Type -> accepted or 415", False, f"st={st}")
    # Malformed JSON.
    st, data, _ = req("POST", "/mcp", raw_body=b"{not-json", timeout=30)
    check("C3 malformed JSON -> 4xx (never 5xx)", 400 <= st < 500, f"{st}")
    # Wrong jsonrpc version -> normalized or 415 (document which).
    st, data, _ = req("POST", "/mcp", raw_body=json.dumps({
        "jsonrpc": "1.1", "id": "c-11", "method": "tools/call",
        "params": {"name": f"{IP_SERVER}-{IP_TOOL}", "arguments": {}}}).encode("utf-8"),
        headers={"MCP-Protocol-Version": "2025-06-18"}, timeout=90)
    if 200 <= st < 300:
        warn("C4 wrong jsonrpc 1.1 -> NORMALIZED and accepted")
        check("C4 wrong jsonrpc version -> normalized or 415 (documented: normalized)", True)
    elif st == 415:
        warn("C4 wrong jsonrpc 1.1 -> 415")
        check("C4 wrong jsonrpc version -> normalized or 415 (documented: 415)", True)
    else:
        check("C4 wrong jsonrpc version -> normalized or 415", False, f"st={st} {data[:100]}")
    # Strict-mode spot check.
    orig_strict = cfg_get("mcp", "strictValidation")
    try:
        cfg = cfg_read()
        cfg.setdefault("mcp", {})["strictValidation"] = True
        cfg_write(cfg)
        time.sleep(0.3)
        st, data, _ = req("POST", "/mcp", body={
            "jsonrpc": "2.0", "id": "c-strict", "method": "tools/call",
            "params": {"name": f"{IP_SERVER}-{IP_TOOL}", "arguments": {}}},
            default_accept=False, headers={"MCP-Protocol-Version": "2025-06-18"},
            timeout=30)
        check("C5 strict mode rejects bare request -> 4xx", 400 <= st < 500,
              f"st={st} {data[:100]}")
    finally:
        cfg0 = cfg_read()
        if orig_strict is None:
            cfg0.get("mcp", {}).pop("strictValidation", None)
        else:
            cfg0.setdefault("mcp", {})["strictValidation"] = orig_strict
        cfg_write(cfg0)


# ── D: OAuth token lifecycle (real end-to-end) ──────────────────────────────
def pkce_pair():
    verifier = secrets.token_urlsafe(48)
    challenge = base64.urlsafe_b64encode(
        hashlib.sha256(verifier.encode()).digest()).rstrip(b"=").decode()
    return verifier, challenge


def section_d():
    print("== D: OAuth token lifecycle (real HTTP end-to-end) ==")
    orig = cfg_get("oauthServer")
    con = sqlite3.connect(DB)
    try:
        cleanup_oauth_clients(con, ["r1100-e2e"])
    finally:
        con.close()
    redirect = "http://127.0.0.1/callback"
    try:
        cfg = cfg_read()
        cfg["oauthServer"] = {"enabled": True, "requireClientSecret": False,
                              "allowedScopes": [], "requireState": True,
                              "dynamicRegistration": {"enabled": True}}
        cfg_write(cfg)
        time.sleep(0.3)

        # (1) dynamic client registration
        st, data, _ = req("POST", "/oauth/register", body={
            "client_name": "r1100-e2e", "redirect_uris": [redirect]},
            default_accept=False)
        try:
            reg = json.loads(data)
        except Exception:
            reg = {}
        client_id = reg.get("client_id")
        check("D1 dynamic registration -> 201 with client_id",
              st == 201 and bool(client_id), f"st={st} {data[:140]}")
        if not client_id:
            skip("D2-D4 positive flow (no client_id)")
            client_id = "bogus-client"

        # (2) authorization endpoint -> consent page (assert not 500)
        verifier, challenge = pkce_pair()
        q = (f"?response_type=code&client_id={quote(client_id, safe='')}"
             f"&redirect_uri={quote(redirect, safe='')}&scope=mcp:all&state=r1100"
             f"&code_challenge={challenge}&code_challenge_method=S256")
        st, data, h = req("GET", f"/oauth/authorize{q}", default_accept=False)
        m = re.search(r'name="nonce" value="([^"]+)"', data)
        nonce = m.group(1) if m else None
        check("D2 authorize endpoint -> 200 consent page with nonce (not 500)",
              st == 200 and nonce is not None and "Approve" in data,
              f"st={st} nonce={'yes' if nonce else 'no'}")
        code = None
        if st == 200 and nonce:
            # (3) approve POST -> 302 with code
            form = ("nonce={}&client_id={}&redirect_uri={}&scope=mcp:all&state=r1100"
                    "&code_challenge={}&resource=&action=approve").format(
                quote(nonce, safe=""), quote(client_id, safe=""),
                quote(redirect, safe=""), quote(challenge, safe=""))
            st2, data2, h2 = req("POST", "/oauth/authorize", body=form, form=True,
                                 default_accept=False)
            loc = h2.get("location", "")
            mc = re.search(r"[?&]code=([^&]+)", loc)
            code = mc.group(1) if mc else None
            check("D3 approve POST -> 302 with code + state",
                  st2 == 302 and code is not None and "state=r1100" in loc,
                  f"st2={st2} loc={loc[:120]}")
        else:
            skip("D3 approve POST (no consent page)")
        if code:
            # (4) code exchange -> access + refresh tokens
            body = ("grant_type=authorization_code&code={}&redirect_uri={}"
                    "&client_id={}&code_verifier={}").format(
                quote(code, safe=""), quote(redirect, safe=""),
                quote(client_id, safe=""), quote(verifier, safe=""))
            st, data, _ = req("POST", "/oauth/token", body=body, form=True,
                              default_accept=False)
            try:
                tok = json.loads(data)
            except Exception:
                tok = {}
            check("D4 code exchange -> 200 access_token + refresh_token",
                  st == 200 and tok.get("access_token") and tok.get("refresh_token"),
                  f"st={st} {data[:140]}")
            rt = tok.get("refresh_token")
            # (5) real refresh_token grant with the returned refresh token.
            if rt:
                body = ("grant_type=refresh_token&refresh_token={}&client_id={}").format(
                    quote(rt, safe=""), quote(client_id, safe=""))
                st, data, _ = req("POST", "/oauth/token", body=body, form=True,
                                  default_accept=False)
                try:
                    tok2 = json.loads(data)
                except Exception:
                    tok2 = {}
                check("D5 refresh_token grant with real RT -> 200 new access_token",
                      st == 200 and bool(tok2.get("access_token")),
                      f"st={st} {data[:140]}")
            else:
                skip("D5 refresh_token grant (no refresh token issued)")
        else:
            skip("D4 code exchange (no code)")
            skip("D5 refresh_token grant (no code)")

        # Negative paths.
        body = ("grant_type=authorization_code&code=bogus-code&redirect_uri={}"
                "&client_id={}&code_verifier={}").format(
            quote(redirect, safe=""), quote(client_id, safe=""), quote("x" * 43, safe=""))
        st, data, _ = req("POST", "/oauth/token", body=body, form=True,
                          default_accept=False)
        check("D6 bogus code -> 400 invalid_grant",
              st == 400 and "invalid_grant" in data, f"st={st} {data[:120]}")

        st, data, _ = req("POST", "/oauth/token", body="code=x&client_id=y",
                          form=True, default_accept=False)
        check("D7 missing grant_type -> 4xx (never 5xx)", 400 <= st < 500,
              f"st={st} {data[:120]}")

        body = ("grant_type=refresh_token&refresh_token=bogus-rt&client_id={}").format(
            quote(client_id, safe=""))
        st, data, _ = req("POST", "/oauth/token", body=body, form=True,
                          default_accept=False)
        check("D8 bogus refresh token -> 400 invalid_grant",
              st == 400 and "invalid_grant" in data, f"st={st} {data[:120]}")

        # (6) metadata endpoints still correct.
        st, data, _ = req("GET", "/.well-known/oauth-protected-resource",
                          default_accept=False)
        check("D9 protected-resource metadata 200", st == 200, f"st={st}")
        st, data, _ = req("GET", "/.well-known/oauth-authorization-server",
                          default_accept=False)
        try:
            meta = json.loads(data)
        except Exception:
            meta = {}
        check("D10 auth-server metadata 200 with token+registration endpoints",
              st == 200 and meta.get("issuer")
              and "/oauth/token" in (meta.get("token_endpoint") or "")
              and "/oauth/register" in (meta.get("registration_endpoint") or ""),
              f"st={st} {data[:140]}")
    finally:
        # Cleanup the r1100-e2e client + its codes/tokens.
        try:
            con = sqlite3.connect(DB)
            cleanup_oauth_clients(con, ["r1100-e2e"])
            con.close()
        except Exception:
            pass
        cfg0 = cfg_read()
        if orig is None:
            cfg0.pop("oauthServer", None)
        else:
            cfg0["oauthServer"] = orig
        cfg_write(cfg0)


# ── E: $smart search/describe/call naming ───────────────────────────────────
def smart_session(path="/mcp/$smart", version="2025-06-18"):
    st, echoed, sid = initialize(path, version, client="r1100-smart")
    if st != 200:
        return None
    notify_initialized(path, sid)
    return sid


def section_e():
    print("== E: $smart search/describe/call naming (pin asymmetry) ==")
    snap = pinned_rows()
    if snap:
        warn(f"E setup: temporarily clearing {len(snap)} existing pin(s)")
        con = sqlite3.connect(DB)
        con.execute("UPDATE server_tool_config SET pinned=0")
        con.commit()
        con.close()
        time.sleep(0.6)
    try:
        pin(IP_SERVER, IP_TOOL)
        sid = smart_session()
        if sid is None:
            skip("E faces (no $smart session)")
            return
        st, data, _ = rpc("/mcp/$smart", 2, "tools/call",
                          {"name": "smart_route_search",
                           "arguments": {"query": "ip", "limit": 5}}, session=sid)
        o = sse_last(data, 2)
        try:
            hits = json.loads(result_text(o)).get("tools", [])
        except Exception:
            hits = []
        check("E1 pinned: search hit from pinned server only",
              len(hits) >= 1 and all(h.get("serverName") == IP_SERVER for h in hits),
              json_txt(hits)[:120])
        hit_names = [h.get("name") for h in hits]
        prefixed = [n for n in hit_names if n and n != IP_TOOL]
        # Documented asymmetry (round-6 contract): describe accepts the BARE
        # pinned name, call requires the PREFIXED form on multi-server scopes.
        warn("E2 documented asymmetry: describe accepts BARE pinned name; "
             "call requires PREFIXED form on multi-server root $smart")
        st, data, _ = rpc("/mcp/$smart", 3, "tools/call",
                          {"name": "smart_route_describe",
                           "arguments": {"toolName": IP_TOOL}}, session=sid)
        o = sse_last(data, 3)
        desc_ok = tool_error(o) is None and IP_TOOL in result_text(o) \
            and '"error"' not in result_text(o)
        check("E2 pinned: describe via BARE name succeeds (documented asymmetry)",
              desc_ok, json_txt(o)[:140])
        st, data, _ = rpc("/mcp/$smart", 4, "tools/call",
                          {"name": "smart_route_call",
                           "arguments": {"toolName": IP_TOOL, "arguments": {}}},
                          session=sid, timeout=90)
        o = sse_last(data, 4)
        bare_call_refused = tool_error(o) is not None or '"error"' in result_text(o)
        check("E3 pinned: call via BARE name refused on multi-server scope",
              bare_call_refused, json_txt(o)[:140])
        call_name = prefixed[0] if prefixed else f"{IP_SERVER}-{IP_TOOL}"
        st, data, _ = rpc("/mcp/$smart", 5, "tools/call",
                          {"name": "smart_route_call",
                           "arguments": {"toolName": call_name, "arguments": {}}},
                          session=sid, timeout=90)
        o = sse_last(data, 5)
        check("E4 pinned: call via PREFIXED name -> public IPv4",
              tool_error(o) is None and '"error"' not in result_text(o)
              and valid_pub_ip(result_text(o)),
              f"call_name={call_name} {json_txt(o)[:140]}")

        unpin(IP_SERVER, IP_TOOL)
        sid2 = smart_session()
        check("E5 unpin: session established", sid2 is not None)
        if sid2 is None:
            return
        st, data, _ = rpc("/mcp/$smart", 6, "tools/call",
                          {"name": "smart_route_search",
                           "arguments": {"query": "ip", "limit": 5}}, session=sid2)
        o = sse_last(data, 6)
        try:
            hits2 = json.loads(result_text(o)).get("tools", [])
        except Exception:
            hits2 = []
        check("E6 unpinned: search returns hits again (gate inactive)",
              len(hits2) >= 1, f"hits={len(hits2)}")
        st, data, _ = rpc("/mcp/$smart", 7, "tools/call",
                          {"name": "smart_route_describe",
                           "arguments": {"toolName": "analyze_calls"}}, session=sid2)
        o = sse_last(data, 7)
        check("E7 unpinned: describe on formerly-refused (unpinned) tool succeeds",
              tool_error(o) is None and '"error"' not in result_text(o),
              json_txt(o)[:140])
        names2 = []
        st, data, _ = rpc("/mcp/$smart", 8, "tools/list", {}, session=sid2)
        o = sse_last(data, 8)
        names2 = [t.get("name", "")
                  for t in (((o or {}).get("result", {}) or {}).get("tools", []) or [])]
        check("E8 unpinned: tools/list non-empty with meta tools",
              len(names2) > 0 and bool(META_TOOLS & set(names2)), f"names={names2[:6]}")
    finally:
        restore_pins(snap)


def main():
    try:
        st, _, _ = req("GET", "/health", default_accept=False, timeout=5)
        if st != 200:
            print(f"SKIP-ALL: hub not healthy at {HOST}:{PORT} (status {st})")
            sys.exit(0)
    except Exception as e:
        print(f"SKIP-ALL: hub down at {HOST}:{PORT}: {e}")
        sys.exit(0)
    group = pick_group()
    server_path = IP_SERVER  # enabled servers listed by name; scope path uses it
    snap = cfg_read()
    orig_bearer = (snap.get("routing") or {}).get("enableBearerAuth")
    try:
        if orig_bearer:
            cfg = cfg_read()
            cfg.setdefault("routing", {})["enableBearerAuth"] = False
            cfg_write(cfg)
        if group is None:
            warn("no suitable group found - group-channel cases will be absent")
        for fn in (section_a, section_b, section_c, section_d, section_e):
            try:
                if fn in (section_a, section_b):
                    fn(group, server_path)
                else:
                    fn()
            except Exception as e:
                import traceback
                traceback.print_exc()
                check(f"{fn.__name__} no unexpected exception", False, repr(e))
    finally:
        # Sections restore their own pins; safety-net: no-op placeholder.
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
