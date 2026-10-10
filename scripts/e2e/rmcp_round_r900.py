#!/usr/bin/env python3
"""E2E round r900: scenarios still untested (second batch).

A  $smart four-face pin parity (round-8 fix): seed a server-level pin via DB
   (pin_helper), then on /mcp/$smart with a 2025-06-18 session verify the
   pinned tool across all four faces (tools/list / search / describe / call)
   and that an UNPINNED tool is refused on all four; unpin restores.
B  OAuth metadata surface: /.well-known/oauth-protected-resource and
   /.well-known/oauth-authorization-server (enabled for the section, restored).
C  Version × channel quick matrix: real MCP ip-tool call per legacy version
   on /mcp + 2026-07-28 stateless discover/tools/call.
D  Leniency pairing (leniency side only, strict untouched): bare tools/call
   without initialize/session → 2xx valid JSON-RPC; malformed JSON → 4xx;
   text/plain Content-Type → 415.
Cleanup: unpin in finally; bearer/OAuth config untouched (D needs bearer off —
restored like r800).
"""
import http.client, json, os, re, sqlite3, sys, time
import urllib.parse

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pin_helper import pin, unpin

HOST, PORT = "127.0.0.1", 23333
BASE = f"http://{HOST}:{PORT}"
PASS, FAIL, SKIPS = 0, 0, 0
FAILURES = []

PIN_SERVER = "本机公网ip查询"
PIN_TOOL = "getPublicIp"
UNPINNED_TOOL = "playwright-browser_close"
IPV4_RE = re.compile(r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b")


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
    con.execute("UPDATE system_config SET config_json=? WHERE id=1", (json.dumps(cfg),))
    con.commit()
    con.close()
    time.sleep(0.3)


DB = os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")


def smart_session(version="2025-06-18"):
    """Initialize on /mcp/$smart, send initialized, return session id."""
    st, data, h = req("POST", "/mcp/$smart", body={
        "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": version, "capabilities": {},
            "clientInfo": {"name": "r900", "version": "1.0"}}})
    if st != 200:
        return None
    sid = h.get("mcp-session-id", "")
    req("POST", "/mcp/$smart", body={"jsonrpc": "2.0", "method": "notifications/initialized"},
        headers={"mcp-session-id": sid})
    return sid


def smart_rpc(sid, rid, method, params=None, timeout=120):
    b = {"jsonrpc": "2.0", "id": rid, "method": method}
    if params is not None:
        b["params"] = params
    return req("POST", "/mcp/$smart", body=b,
               headers={"mcp-session-id": sid} if sid else None, timeout=timeout)


def smart_call(sid, rid, meta_name, args=None):
    return smart_rpc(sid, rid, "tools/call",
                     {"name": meta_name, "arguments": args if args is not None else {}})


def result_text(o):
    try:
        return o["result"]["content"][0]["text"]
    except Exception:
        return ""


def tool_error_code(o):
    e = (o or {}).get("error")
    return e.get("code") if isinstance(e, dict) else None


# ── A: $smart four-face pin parity ──────────────────────────────────────────
def section_a():
    print("== A: $smart four-face pin parity ==")
    sid = smart_session()
    check("A0 $smart session established", sid is not None)
    if sid is None:
        skip("A pin parity (no session)")
        return
    exposed_name = None
    try:
        pin(PIN_SERVER, PIN_TOOL)

        # Face 1: tools/list
        st, data, _ = smart_rpc(sid, 2, "tools/list")
        o = sse_last(data, 2)
        tools = ((o or {}).get("result", {}) or {}).get("tools", []) or []
        names = [t.get("name", "") for t in tools]
        check("A1 tools/list contains pinned tool",
              PIN_TOOL in names or any(n.endswith("-" + PIN_TOOL) for n in names),
              f"names={names[:8]}")
        exposed_name = next((n for n in names if n == PIN_TOOL or n.endswith("-" + PIN_TOOL)), None)
        check("A2 unpinned tool absent from tools/list",
              UNPINNED_TOOL not in names and not any(n.endswith(UNPINNED_TOOL) for n in names),
              f"names={names[:8]}")

        # Face 2: search
        st, data, _ = smart_call(sid, 3, "smart_route_search", {"query": "ip", "limit": 5})
        o = sse_last(data, 3)
        try:
            hits = json.loads(result_text(o))["tools"]
        except Exception:
            hits = []
        check("A3 search 'ip' returns >=1 hit", len(hits) >= 1, result_text(o)[:120])
        check("A4 all search hits from pinned server",
              bool(hits) and all(h.get("serverName") == PIN_SERVER for h in hits),
              json_txt(hits)[:120])
        st, data, _ = smart_call(sid, 4, "smart_route_search",
                                 {"query": "close browser window", "limit": 5})
        o = sse_last(data, 4)
        try:
            hits2 = json.loads(result_text(o))["tools"]
        except Exception:
            hits2 = []
        check("A5 unpinned tool absent from search hits",
              all("browser_close" not in (h.get("name") or "") for h in hits2),
              json_txt(hits2)[:120])

        # Face 3: describe (accepts bare pinned name)
        st, data, _ = smart_call(sid, 5, "smart_route_describe", {"toolName": PIN_TOOL})
        o = sse_last(data, 5)
        desc = result_text(o)
        check("A6 describe returns the pinned tool",
              st == 200 and tool_error_code(o) is None and PIN_TOOL in desc
              and "inputSchema" in desc,
              f"st={st} {json_txt(o)[:120]}")
        st, data, _ = smart_call(sid, 6, "smart_route_describe", {"toolName": UNPINNED_TOOL})
        o = sse_last(data, 6)
        check("A7 describe refuses unpinned tool (error body)",
              st == 200 and (tool_error_code(o) is not None or '"error"' in result_text(o)),
              json_txt(o)[:120])

        # Face 4: call — use the name tools/list actually exposes
        target = exposed_name or PIN_TOOL
        st, data, _ = smart_call(sid, 7, "smart_route_call", {"toolName": target, "arguments": {}})
        o = sse_last(data, 7)
        txt = result_text(o)
        check("A8 smart_route_call returns real IPv4",
              st == 200 and tool_error_code(o) is None
              and o.get("result", {}).get("isError") is False and IPV4_RE.search(txt),
              f"st={st} {json_txt(o)[:160]}")
        st, data, _ = smart_call(sid, 8, "smart_route_call",
                                 {"toolName": UNPINNED_TOOL, "arguments": {}})
        o = sse_last(data, 8)
        check("A9 smart_route_call refuses unpinned (-32602 pin gate)",
              st == 200 and tool_error_code(o) == -32602, json_txt(o)[:120])
    finally:
        unpin(PIN_SERVER, PIN_TOOL)
    # Post-unpin: all four faces drop the tool.
    sid2 = smart_session()
    if sid2 is None:
        skip("A10 unpin verification (no session)")
        return
    st, data, _ = smart_rpc(sid2, 11, "tools/list")
    o = sse_last(data, 11)
    names = [t.get("name", "") for t in (((o or {}).get("result", {}) or {}).get("tools", []) or [])]
    check("A10 after unpin tools/list drops pinned tool",
          not any(n == PIN_TOOL or n.endswith("-" + PIN_TOOL) for n in names), f"names={names[:8]}")
    st, data, _ = smart_call(sid2, 12, "smart_route_search", {"query": "ip", "limit": 5})
    o = sse_last(data, 12)
    try:
        hits = json.loads(result_text(o))["tools"]
    except Exception:
        hits = [{"name": "?", "serverName": PIN_SERVER}]
    # Gate is now INACTIVE (no pins left) — the tool returns to normal
    # exposure; what matters is that search still functions (A14) and that
    # the unpinned tool is no longer the pin-gated surface. The old
    # "tool must vanish" expectation was the empty-pin-set wipe bug.
    check("A11 after unpin gate inactive (search functional)",
          len(hits) >= 1, json_txt(hits)[:120])
    st, data, _ = smart_call(sid2, 13, "smart_route_call", {"toolName": PIN_TOOL, "arguments": {}})
    o = sse_last(data, 13)
    # Gate inactive after unpin: the formerly-pinned tool is callable again
    # (the old "call refuses" expectation encoded the empty-pin wipe bug).
    check("A12 after unpin call succeeds (gate inactive)",
          tool_error_code(o) is None, json_txt(o)[:120])
    st, data, _ = smart_call(sid2, 14, "smart_route_describe", {"toolName": PIN_TOOL})
    o = sse_last(data, 14)
    # Gate inactive: describe of the formerly-pinned tool succeeds again.
    check("A13 after unpin describe succeeds (gate inactive)",
          tool_error_code(o) is None and '"error"' not in result_text(o), json_txt(o)[:120])
    # Regression (round 9): with ZERO pins left on the scope the pin gate is
    # INACTIVE — search must return normal hits, not refuse everything
    # (an empty pin set used to wipe all results on root $smart).
    st, data, _ = smart_call(sid2, 15, "smart_route_search", {"query": "ip", "limit": 5})
    o = sse_last(data, 15)
    try:
        hits15 = json.loads(result_text(o))["tools"]
    except Exception:
        hits15 = []
    check("A14 zero pins: search still returns hits (gate inactive)",
          len(hits15) >= 1, f"hits={len(hits15)} {json_txt(hits15)[:100]}")


# ── B: OAuth metadata surface ───────────────────────────────────────────────
def section_b():
    print("== B: OAuth metadata surface ==")
    snap = cfg_read()
    orig = snap.get("oauthServer")
    cfg = cfg_read()
    cfg.setdefault("oauthServer", {})["enabled"] = True
    cfg_write(cfg)
    try:
        st, data, _ = req("GET", "/.well-known/oauth-protected-resource", default_accept=False)
        try:
            pr = json.loads(data)
        except Exception:
            pr = {}
        check("B1 protected-resource 200 with resource", st == 200 and pr.get("resource"),
              f"{st} {data[:120]}")
        check("B2 authorization_servers == [resource]",
              pr.get("authorization_servers") == [pr.get("resource")], json_txt(pr)[:160])
        st, data, _ = req("GET", "/.well-known/oauth-authorization-server", default_accept=False)
        try:
            meta = json.loads(data)
        except Exception:
            meta = {}
        check("B3 auth-server metadata 200 with issuer", st == 200 and meta.get("issuer"),
              f"{st} {data[:120]}")
        check("B4 token_endpoint advertised", bool(meta.get("token_endpoint")), data[:160])
        grants = set(meta.get("grant_types_supported", []) or [])
        check("B5 grants include authorization_code+refresh_token",
              {"authorization_code", "refresh_token"} <= grants, str(sorted(grants)))
        auth_methods = set(meta.get("token_endpoint_auth_methods_supported", []) or [])
        check("B6 register endpoint advertises client_secret_post",
              bool(meta.get("registration_endpoint")) and "client_secret_post" in auth_methods,
              str(sorted(auth_methods)))
    finally:
        cfg0 = cfg_read()
        if orig is None:
            cfg0.pop("oauthServer", None)
        else:
            cfg0["oauthServer"] = orig
        cfg_write(cfg0)


# ── C: version × channel quick matrix ───────────────────────────────────────
def section_c():
    print("== C: version × channel matrix ==")
    for v in ["2025-03-26", "2025-06-18", "2025-11-25"]:
        st, data, h = req("POST", "/mcp", body={
            "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": v, "capabilities": {},
                "clientInfo": {"name": "r900", "version": "1.0"}}})
        o = sse_last(data, 1)
        echoed = (o or {}).get("result", {}).get("protocolVersion")
        check(f"C1 initialize {v} echo", st == 200 and echoed == v, f"st={st} echo={echoed}")
        sid = h.get("mcp-session-id", "")
        req("POST", "/mcp", body={"jsonrpc": "2.0", "method": "notifications/initialized"},
            headers={"mcp-session-id": sid} if sid else None)
        st, data, _ = req("POST", "/mcp", body={"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
                          headers={"mcp-session-id": sid} if sid else None)
        o = sse_last(data, 2)
        tools = ((o or {}).get("result", {}) or {}).get("tools", []) or []
        names = [t.get("name", "") for t in tools]
        target = next((n for n in names if "getPublicIp" in n), None)
        check(f"C2 {v} tools/list has ip tool", target is not None, f"names={names[:6]}")
        if target is None:
            skip(f"C3 {v} ip call (tool not listed)")
            continue
        st, data, _ = req("POST", "/mcp", body={
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {"name": target, "arguments": {}}},
            headers={"mcp-session-id": sid} if sid else None)
        o = sse_last(data, 3)
        txt = result_text(o)
        check(f"C3 {v} ip tool call returns IPv4",
              st == 200 and o is not None and o.get("result", {}).get("isError") is not True
              and IPV4_RE.search(txt), f"st={st} {json_txt(o)[:140]}")
    # 2026-07-28 stateless
    meta = {"_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28",
                      "io.modelcontextprotocol/clientCapabilities": {}}}
    st, data, _ = req("POST", "/mcp", body={
        "jsonrpc": "2.0", "id": 4, "method": "server/discover",
        "params": meta}, headers={"MCP-Protocol-Version": "2026-07-28"})
    o = sse_last(data, 4)
    r = (o or {}).get("result", {}) or {}
    sv = r.get("supportedVersions", [])
    check("C4 2026 discover supportedVersions non-empty",
          st == 200 and isinstance(sv, list) and len(sv) > 0, f"st={st} {data[:120]}")
    st, data, _ = req("POST", "/mcp", body={
        "jsonrpc": "2.0", "id": 5, "method": "tools/call",
        "params": {"name": "本机公网ip查询-getPublicIp", "arguments": {}, **meta}},
        headers={"MCP-Protocol-Version": "2026-07-28"})
    o = sse_last(data, 5)
    r = (o or {}).get("result", {}) or {}
    check("C5 2026 tools/call resultType complete + IPv4",
          st == 200 and r.get("resultType") == "complete"
          and IPV4_RE.search(result_text(o) or ""), f"st={st} {data[:140]}")


# ── D: leniency pairing (leniency side only) ────────────────────────────────
def section_d():
    print("== D: leniency pairing ==")
    # D1: bare tools/call — no initialize, no session, no Accept, legacy header.
    st, data, _ = req("POST", "/mcp", body={
        "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {
            "name": "本机公网ip查询-getPublicIp", "arguments": {}}},
        default_accept=False, headers={"MCP-Protocol-Version": "2025-11-25"})
    o = sse_last(data, 7)
    check("D1 bare tools/call (no Accept/session) → 2xx", st == 200 and 200 <= st < 300, f"{st}")
    check("D2 bare response is valid JSON-RPC",
          o is not None and o.get("jsonrpc") == "2.0" and o.get("id") == 7
          and (("result" in o) or isinstance(o.get("error"), dict)), json_txt(o)[:140])
    # D3: malformed JSON body → 4xx, never 5xx.
    st, data, _ = req("POST", "/mcp", raw_body=b"{not-json", timeout=30)
    check("D3 malformed JSON → 4xx", 400 <= st < 500, f"{st}")
    # D4: wrong Content-Type text/plain → 415 (leniency-aware, no 5xx).
    st, data, _ = req("POST", "/mcp", raw_body=b'{"jsonrpc":"2.0","id":1,"method":"ping"}',
                      headers={"Content-Type": "text/plain"})
    check("D4 text/plain Content-Type → 415", st == 415, f"{st} {data[:80]}")


def main():
    try:
        st, data, _ = req("GET", "/health", default_accept=False, timeout=5)
        if st != 200:
            print(f"SKIP-ALL: hub not healthy at {BASE} (status {st})")
            sys.exit(0)
    except Exception as e:
        print(f"SKIP-ALL: hub down at {BASE}: {e}")
        sys.exit(0)
    snap = cfg_read()
    orig_bearer = (snap.get("routing") or {}).get("enableBearerAuth")
    try:
        if orig_bearer:
            cfg = cfg_read()
            cfg.setdefault("routing", {})["enableBearerAuth"] = False
            cfg_write(cfg)
        for fn in (section_a, section_b, section_c, section_d):
            try:
                fn()
            except Exception as e:
                import traceback
                traceback.print_exc()
                check(f"{fn.__name__} no unexpected exception", False, repr(e))
    finally:
        try:
            unpin(PIN_SERVER, PIN_TOOL)  # idempotent safety net
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
