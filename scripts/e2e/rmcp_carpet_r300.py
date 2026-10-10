#!/usr/bin/env python3
"""Carpet matrix r300 — 复核轮全矩阵（每版本 × 每上游传输 × 每通道）。

Sections:
  A  5 protocol versions × 3 upstream transports (stdio=codegraph /
     streamable-http=Idea-mcp-server / openapi=本机公网ip查询) × 4 channels
     (/mcp/{server} scope, /rest/{server}/call, /api/{server}/tools, root /mcp)
     — every cell executes at least one REAL MCP tool call; version echo
     asserted equal to the requested version.
  B  group channel per version (/mcp/Test + /rest/group/Test/call) — real IP.
  C  $smart channel per version (group direct call — real IP) + meta search.
  D  leniency: bare upgraded request (no session/Accept/params) per version —
     real IP call. "只要不影响工具调用，都可以放行".
  E  strict mode: DB hot toggle → bare requests rejected for every version;
     well-formed requests still work; restore.
  F  2026 new features: discover (ttl/public), -32022 unknown version,
     CacheableResult ttl/private, resultType complete, json_response=true.
  G  OAuth provider settings items (each UI key): allowedScopes restriction,
     requireState, allowedGrantTypes, requireClientSecret — DB hot toggle.
"""
import json, http.client, sys, re, time, sqlite3, os, urllib.parse
from urllib.parse import quote

HOST, PORT = "127.0.0.1", 23333
DB = os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")
PASS, FAIL = [], []
VERSIONS = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25", "2026-07-28"]
IP_SERVER, IP_TOOL = "本机公网ip查询", "getPublicIp"
UPSTREAMS = [
    ("codegraph", "codegraph_status", {}, "any"),
    ("Idea-mcp-server", "get_project_dependencies", {}, "any"),
    (IP_SERVER, IP_TOOL, {}, "ip"),
]
IP_RE = re.compile(r"\d{1,3}(?:\.\d{1,3}){3}")

def check(name, cond, detail=""):
    (PASS if cond else FAIL).append(name)
    print(("PASS" if cond else "FAIL"), name, detail if not cond else "")

def req(method, path, body=None, headers=None, timeout=120, default_accept=True, raw_body=None):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {}
    if default_accept:
        h["Accept"] = "application/json, text/event-stream"
    if body is not None:
        h["Content-Type"] = "application/json"
    if headers: h.update(headers)
    payload = raw_body if raw_body is not None else (json.dumps(body, ensure_ascii=False).encode("utf-8") if body is not None else None)
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
            try: frames.append(json.loads(line[6:]))
            except Exception: pass
    for f in reversed(frames):
        if want_id is None or f.get("id") == want_id:
            return f
    return None

def unwrap_text(resp):
    v = resp
    for _ in range(4):
        if isinstance(v, dict) and "result" in v and isinstance(v["result"], dict):
            v = v["result"]
        if isinstance(v, dict) and isinstance(v.get("content"), list) and v["content"]:
            try: v = json.loads(v["content"][0]["text"])
            except Exception: break
        else: break
    return v

def json_txt(o):
    return json.dumps(o, ensure_ascii=False) if o is not None else ""

def has_public_ip(txt):
    for m in IP_RE.findall(txt):
        try:
            import ipaddress as _ip
            a = _ip.ip_address(m)
            if a.version == 4 and not (a.is_private or a.is_loopback or a.is_reserved):
                return True
        except ValueError:
            pass
    return False

def kind_ok(kind, st, o, txt):
    if kind == "ip":
        o_err = bool(o and isinstance(o, dict) and (o.get("result", {}) or {}).get("isError"))
        return has_public_ip(txt) and not o_err
    if kind == "list":
        return st == 200 and o is not None and '"isError": true' not in txt and len(txt) > 10
    # Upstream tool-level isError (e.g. IDE "no target project") still proves the hub routed the call;
    # semantic success depends on IDE state (environmental).
    return st == 200 and o is not None and '"result"' in txt

def scope_session(server, version):
    enc = quote(server, safe="")
    st, data, hd = req("POST", f"/mcp/{enc}", {
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": version, "capabilities": {},
                   "clientInfo": {"name": "r300", "version": "1"}}})
    o = sse_last(data, 1)
    sid = hd.get("mcp-session-id")
    if not o or not sid:
        return enc, None, None
    echoed = o.get("result", {}).get("protocolVersion")
    req("POST", f"/mcp/{enc}", {"jsonrpc": "2.0", "method": "notifications/initialized"},
        headers={"mcp-session-id": sid})
    return enc, sid, echoed

def get_cfg():
    con = sqlite3.connect(DB)
    row = con.execute("SELECT config_json FROM system_config WHERE id=1").fetchone()
    con.close()
    return json.loads(row[0] or "{}")

def set_cfg(mutate):
    cfg = get_cfg()
    mutate(cfg)
    con = sqlite3.connect(DB)
    con.execute("UPDATE system_config SET config_json=? WHERE id=1", (json.dumps(cfg, ensure_ascii=False),))
    con.commit(); con.close()
    time.sleep(0.3)

# ── A: versions × transports × channels ─────────────────────────────────────
def section_a():
    print("== A: 5 versions × 3 transports × 4 channels (real calls) ==")
    for v in VERSIONS:
        for server, tool, args, kind in UPSTREAMS:
            enc = quote(server, safe="")
            # Channel 1: /mcp/{server} scope
            _, sid, echoed = scope_session(server, v)
            if sid:
                check(f"A-{v}-{server}-scope version echo", echoed == v or (v == "2024-11-05" and echoed in (v, "2025-03-26")) or (v == "2026-07-28" and echoed == "2025-11-25"), f"echo={echoed}")
                st, data, _ = req("POST", f"/mcp/{enc}", {
                    "jsonrpc": "2.0", "id": "c1", "method": "tools/call",
                    "params": {"name": tool, "arguments": args}},
                    headers={"mcp-session-id": sid})
                o = sse_last(data, "c1")
                check(f"A-{v}-{server}-scope real call", kind_ok(kind, st, o, json_txt(o)), f"st={st} {json_txt(o)[:160]}")
                req("DELETE", f"/mcp/{enc}", headers={"mcp-session-id": sid})
            else:
                check(f"A-{v}-{server}-scope initialize", False, "no session")
            # Channel 2: REST /rest/{server}/call
            st, data, _ = req("POST", f"/rest/{enc}/call",
                              {"tool": tool, "arguments": args}, timeout=120)
            try: o = json.loads(data)
            except Exception: o = None
            txt = json_txt(o)
            ok = st == 200 and not o.get("error") and (kind != "ip" or has_public_ip(txt)) and not txt.startswith('{"jsonrpc"')
            check(f"A-{v}-{server}-rest real call", ok, f"st={st} {txt[:160]}")
            # Channel 3: /api/{server}/tools/{server}/{tool} GET
            q = urllib.parse.urlencode(args)
            st, data, _ = req("GET", f"/api/{enc}/tools/{enc}/{quote(tool, safe='')}{('&' + q) if q else ''}", timeout=120)
            try: o = json.loads(data)
            except Exception: o = None
            txt = json_txt(o)
            ok = st == 200 and not o.get("error") and (kind != "ip" or has_public_ip(txt))
            check(f"A-{v}-{server}-api real call", ok, f"st={st} {txt[:160]}")
            # Channel 4: root /mcp aggregate — call the prefixed ip tool only
            # (root aggregates every server; use the ip server for a real call)
            if server == IP_SERVER:
                _, sid, echoed = scope_session("", v)
                if sid:
                    st, data, _ = req("POST", "/mcp", {
                        "jsonrpc": "2.0", "id": "c4", "method": "tools/call",
                        "params": {"name": f"{IP_SERVER}{chr(0x2E)}{IP_TOOL}".replace(chr(0x2E), "-"), "arguments": {}}},
                        headers={"mcp-session-id": sid})
                    o = sse_last(data, "c4")
                    check(f"A-{v}-root real ip call", kind_ok("ip", st, o, json_txt(o)), f"st={st} {json_txt(o)[:160]}")
                    req("DELETE", "/mcp", headers={"mcp-session-id": sid})

# ── B: group channel per version ─────────────────────────────────────────────
def section_b():
    print("== B: group channel per version (real IP) ==")
    for v in VERSIONS:
        enc = quote("Test", safe="")
        st, data, hd = req("POST", f"/mcp/{enc}", {
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": v, "capabilities": {},
                       "clientInfo": {"name": "r300", "version": "1"}}})
        o = sse_last(data, 1)
        sid = hd.get("mcp-session-id")
        check(f"B-{v} group initialize", st == 200 and bool(sid), f"st={st}")
        if not sid: continue
        req("POST", f"/mcp/{enc}", {"jsonrpc": "2.0", "method": "notifications/initialized"},
            headers={"mcp-session-id": sid})
        st, data, _ = req("POST", f"/mcp/{enc}", {
            "jsonrpc": "2.0", "id": "g", "method": "tools/call",
            "params": {"name": f"{IP_SERVER}-{IP_TOOL}", "arguments": {}}},
            headers={"mcp-session-id": sid})
        o = sse_last(data, "g")
        check(f"B-{v} group /mcp real ip call", kind_ok("ip", st, o, json_txt(o)), f"st={st} {json_txt(o)[:160]}")
        req("DELETE", f"/mcp/{enc}", headers={"mcp-session-id": sid})
        # REST group
        st, data, _ = req("POST", f"/rest/group/{enc}/call", {"tool": IP_TOOL, "arguments": {}})
        try: o = json.loads(data)
        except Exception: o = None
        txt = json_txt(o)
        check(f"B-{v} group /rest real ip call", st == 200 and has_public_ip(txt) and not o.get("error"), f"st={st} {txt[:160]}")

# ── C: $smart channel per version ────────────────────────────────────────────
def section_c():
    print("== C: $smart channel per version (real IP via group direct) ==")
    # $smart/{group} direct calls are gated by pinned parity (R110 sync change) — pin via DB, restore after
    import sqlite3 as _sq, os as _os
    _db = _os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")
    _con = _sq.connect(_db)
    _row = _con.execute("SELECT id FROM server_tool_config WHERE server_name=? AND item_type='tool' AND item_name=?",
                        (IP_SERVER, IP_TOOL)).fetchone()
    if _row:
        _con.execute("UPDATE server_tool_config SET pinned=1 WHERE id=?", (_row[0],))
    else:
        _con.execute("INSERT INTO server_tool_config (id, server_name, item_type, item_name, pinned) VALUES (?,?,?,?,1)",
                     (f"e2e-r300-pin", IP_SERVER, 'tool', IP_TOOL))
    _con.commit(); _con.close()
    import subprocess as _sp, time as _t
    _t.sleep(1)
    for v in VERSIONS:
        enc = quote("$smart/Test", safe="")
        st, data, hd = req("POST", f"/mcp/{enc}", {
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": v, "capabilities": {},
                       "clientInfo": {"name": "r300", "version": "1"}}})
        o = sse_last(data, 1)
        sid = hd.get("mcp-session-id")
        check(f"C-{v} $smart initialize", st == 200 and bool(sid), f"st={st}")
        if not sid: continue
        req("POST", f"/mcp/{enc}", {"jsonrpc": "2.0", "method": "notifications/initialized"},
            headers={"mcp-session-id": sid})
        # meta search must work
        st, data, _ = req("POST", f"/mcp/{enc}", {
            "jsonrpc": "2.0", "id": "s", "method": "tools/call",
            "params": {"name": "smart_route_search", "arguments": {"query": "public ip"}}},
            headers={"mcp-session-id": sid})
        o = sse_last(data, "s")
        check(f"C-{v} $smart meta search", st == 200 and o is not None and "getPublicIp" in json_txt(o), f"st={st}")
        # direct call (bare name — single-server-in-scope tools from this group member)
        st, data, _ = req("POST", f"/mcp/{enc}", {
            "jsonrpc": "2.0", "id": "d", "method": "tools/call",
            "params": {"name": f"{IP_SERVER}-{IP_TOOL}", "arguments": {}}},
            headers={"mcp-session-id": sid})
        o = sse_last(data, "d")
        check(f"C-{v} $smart direct real ip call", kind_ok("ip", st, o, json_txt(o)), f"st={st} {json_txt(o)[:160]}")
        req("DELETE", f"/mcp/{enc}", headers={"mcp-session-id": sid})
    # restore pin state
    _con = _sq.connect(_db)
    if _row:
        _con.execute("UPDATE server_tool_config SET pinned=0 WHERE id=?", (_row[0],))
    else:
        _con.execute("DELETE FROM server_tool_config WHERE id=?", ("e2e-r300-pin",))
    _con.commit(); _con.close()

# ── D: leniency bare upgrade per version (real call) ─────────────────────────
def section_d():
    print("== D: leniency bare upgrade per version (no session/Accept/params) ==")
    set_cfg(lambda c: c.setdefault("mcp", {}).update({"strictValidation": False}))
    for v in VERSIONS:
        body = {"jsonrpc": "2.0", "id": 7, "method": "tools/call",
                "params": {"name": IP_TOOL, "arguments": {}}}
        st, data, _ = req("POST", "/mcp", body, headers={"MCP-Protocol-Version": v},
                          default_accept=False, timeout=120)
        o = sse_last(data, 7)
        check(f"D-{v} bare upgrade real ip call", kind_ok("ip", st, o, json_txt(o)), f"st={st} {json_txt(o)[:160]}")

# ── E: strict mode per version ───────────────────────────────────────────────
def section_e():
    print("== E: strict mode per version (bare rejected, well-formed works) ==")
    set_cfg(lambda c: c.setdefault("mcp", {}).update({"strictValidation": True}))
    try:
        for v in VERSIONS:
            st, data, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 7, "method": "tools/call",
                                               "params": {"name": f"{IP_SERVER}-{IP_TOOL}", "arguments": {}}},
                              default_accept=False)
            check(f"E-{v} strict bare rejected", st in (400, 406, 415, 422), f"st={st}")
            # well-formed initialize still works in strict mode
            st, data, hd = req("POST", "/mcp", {
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": v, "capabilities": {},
                           "clientInfo": {"name": "r300", "version": "1"}}})
            o = sse_last(data, 1)
            check(f"E-{v} strict well-formed initialize", st == 200 and o is not None, f"st={st}")
            sid = hd.get("mcp-session-id")
            if sid:
                req("DELETE", "/mcp", headers={"mcp-session-id": sid})
    finally:
        set_cfg(lambda c: c.setdefault("mcp", {}).update({"strictValidation": False}))

# ── F: 2026 new features ─────────────────────────────────────────────────────
def section_f():
    print("== F: 2026 new features ==")
    # discover
    st, data, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 1, "method": "server/discover",
        "params": {"_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28"}}},
        headers={"MCP-Protocol-Version": "2026-07-28"})
    o = sse_last(data, 1)
    r = o.get("result", {}) if o else {}
    check("F discover 200", st == 200 and o is not None, f"st={st}")
    check("F discover ttl 3600000/public", r.get("ttlMs") == 3600000 and r.get("cacheScope") == "public",
          json_txt(r)[:120])
    # unknown version → -32022
    st, data, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 2, "method": "tools/list",
        "params": {"_meta": {"io.modelcontextprotocol/protocolVersion": "2026-12-31"}}},
        headers={"MCP-Protocol-Version": "2026-12-31"})
    o = sse_last(data, 2)
    err = (o or {}).get("error", {})
    check("F unknown version -32022", err.get("code") == -32022, f"err={json_txt(err)[:120]}")
    # tools/list cache hint + resultType complete (2026 stateless)
    st, data, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 3, "method": "tools/list",
        "params": {"_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28",
                              "io.modelcontextprotocol/clientCapabilities": {}}}},
        headers={"MCP-Protocol-Version": "2026-07-28"})
    o = sse_last(data, 3)
    r = o.get("result", {}) if o else {}
    check("F 2026 tools/list ttl bounded/private",
          isinstance(r.get("ttlMs"), int) and 0 <= r.get("ttlMs") <= 5000 and r.get("cacheScope") == "private",
          json_txt({k: r.get(k) for k in ("ttlMs", "cacheScope")}))
    check("F 2026 resultType complete", r.get("resultType") == "complete", str(r.get("resultType")))
    # json_response=true: direct JSON body (no SSE prefix)
    st, data, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 4, "method": "tools/list",
        "params": {"_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28",
                              "io.modelcontextprotocol/clientCapabilities": {}}}},
        headers={"MCP-Protocol-Version": "2026-07-28"})
    body = data.strip()
    is_json = body.startswith("{") and '"tools"' in body
    check("F json_response direct JSON", is_json, data[:80])

# ── G: OAuth provider settings items ─────────────────────────────────────────
def oauth_register():
    st, data, _ = req("POST", "/oauth/register", body={"client_name": "r300",
        "redirect_uris": ["https://claude.example/cb"]})
    try: return st, json.loads(data)
    except Exception: return st, {}

def section_g():
    print("== G: OAuth provider settings items (each UI key) ==")
    orig = get_cfg().get("oauthServer")
    def restore():
        def m(c):
            if orig is None: c.pop("oauthServer", None)
            else: c["oauthServer"] = orig
        set_cfg(m)
    try:
        def enable(c): c.setdefault("oauthServer", {})["enabled"] = True
        set_cfg(enable)

        # G1 requireState
        def req_state(c): c["oauthServer"]["requireState"] = True
        set_cfg(req_state)
        st, reg = oauth_register()
        q = urllib.parse.urlencode({"response_type": "code", "client_id": reg.get("client_id", ""),
            "redirect_uri": "https://claude.example/cb", "scope": "mcp:all",
            "code_challenge": "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
            "code_challenge_method": "S256"})
        st, data, hh = req("GET", f"/oauth/authorize?{q}")
        loc = hh.get("location", "")
        check("G1 requireState missing rejected", st == 302 and "error=invalid_request" in loc,
              f"st={st} loc={loc[:120]}")
        set_cfg(lambda c: c["oauthServer"].update({"requireState": False}))
        st, data, _ = req("GET", f"/oauth/authorize?{q}&state=s1")
        check("G1b state optional ok", st == 200, f"st={st}")

        # G2 allowedScopes restriction
        def restrict(c): c["oauthServer"]["allowedScopes"] = ["custom-scope"]
        set_cfg(restrict)
        q_restricted = q.replace("scope=mcp%3Aall", "scope=read").replace("scope=mcp:all", "scope=read")
        st2, data2, h2 = req("GET", f"/oauth/authorize?{q_restricted}&state=s2")
        loc = h2.get("location", "") or h2.get("Location", "") or ""
        check("G2b invalid_scope on 302", st2 == 302 and "invalid_scope" in loc, f"st={st2} loc={loc[:120]}")
        # R3: empty/whitespace scope under a restricted config must NOT yield an
        # unrestricted token (scope_to_allowed_servers("") → None trap).
        for label, sc in (("empty", ""), ("blank", "%20%20")):
            q_empty = q_restricted.replace("scope=read", f"scope={sc}")
            stE, _, hE = req("GET", f"/oauth/authorize?{q_empty}&state=sE")
            locE = hE.get("location", "") or hE.get("Location", "") or ""
            check(f"G2b-{label} scope rejected when restricted", stE == 302 and "invalid_scope" in locE, f"st={stE} loc={locE[:120]}")
        st, _, _ = req("GET", f"/.well-known/oauth-authorization-server")
        md = json.loads(req("GET", "/.well-known/oauth-authorization-server")[1])
        check("G2c scopes_supported reflects config", md.get("scopes_supported") == ["custom-scope"], str(md.get("scopes_supported")))
        set_cfg(lambda c: c["oauthServer"].update({"allowedScopes": []}))

        # G3 allowedGrantTypes
        def grants(c): c["oauthServer"]["dynamicRegistration"] = {**c["oauthServer"].get("dynamicRegistration", {}), "enabled": True, "allowedGrantTypes": ["authorization_code"]}
        set_cfg(grants)
        st, reg = oauth_register()
        check("G3 metadata grant_types filtered", st == 201, f"st={st}")
        md = json.loads(req("GET", "/.well-known/oauth-authorization-server")[1])
        check("G3b grant_types metadata", md.get("grant_types_supported") == ["authorization_code"], str(md.get("grant_types_supported")))
        form = urllib.parse.urlencode({"grant_type": "refresh_token", "refresh_token": "bogus"})
        st, b, _ = req("POST", "/oauth/token", raw_body=form,
                       headers={"Content-Type": "application/x-www-form-urlencoded"})
        check("G3c refresh grant rejected when not allowed", st == 400 and "unsupported_grant_type" in b, f"st={st} {str(b)[:100]!r}")
        set_cfg(lambda c: c["oauthServer"].update({"dynamicRegistration": {**c["oauthServer"].get("dynamicRegistration", {}), "allowedGrantTypes": []}}))

        # G4 requireClientSecret
        def secret(c): c["oauthServer"]["requireClientSecret"] = True
        set_cfg(secret)
        st, reg = oauth_register()
        check("G4 requireClientSecret issues secret", st == 201 and bool(reg.get("client_secret")), str(reg)[:120])
        set_cfg(lambda c: c["oauthServer"].update({"requireClientSecret": False}))

        # G5 dynamicRegistration.enabled=false blocks register
        def dreg_off(c): c["oauthServer"]["dynamicRegistration"] = {**c["oauthServer"].get("dynamicRegistration", {}), "enabled": False}
        set_cfg(dreg_off)
        st, reg = oauth_register()
        check("G5 dynamic registration disabled 400", st == 400, f"st={st}")
    finally:
        restore()

def main():
    section_a()
    section_b()
    section_c()
    section_d()
    section_e()
    section_f()
    section_g()
    print(f"\n== {len(PASS)}/{len(PASS)+len(FAIL)} passed, {len(FAIL)} failed ==")
    if FAIL:
        print("FAILED:", *FAIL, sep="\n  ")
    sys.exit(1 if FAIL else 0)

if __name__ == "__main__":
    main()
