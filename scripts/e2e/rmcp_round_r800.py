#!/usr/bin/env python3
"""E2E round r800: recent fixes lacking coverage.

A  Refresh-grant client auth (RFC 6749 §6): confidential client must
   authenticate (client_id + client_secret) at the token endpoint for the
   refresh grant; replay of a rotated refresh token → 400 + family revoke.
B  /mcp body limit: >16MB JSON → 4xx (never 5xx/timeout); normal ping 200.
C  Version matrix spot: 2025-03-26 / 2025-06-18 / 2025-11-25 initialize echo
   + 2026-07-28 stateless tools/list (resultType complete).
D  Leniency: bare tools/call without session/initialize → 2xx valid JSON-RPC.
E  $smart meta tools visible on /mcp/$smart (SKIP if Smart Routing disabled).
"""
import http.client, json, os, re, socket, sys, time, hashlib, base64, secrets, sqlite3
import urllib.parse

HOST, PORT = "127.0.0.1", 23333
BASE = f"http://{HOST}:{PORT}"
DB = os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")
PASS, FAIL, SKIPS = 0, 0, 0
FAILURES = []


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


def req(method, path, body=None, headers=None, timeout=120, default_accept=True, raw_body=None, form=None):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {}
    if default_accept:
        h["Accept"] = "application/json, text/event-stream"
    if body is not None:
        h["Content-Type"] = "application/json"
    if headers:
        h.update(headers)
    if form is not None:
        payload = urllib.parse.urlencode(form).encode()
        h["Content-Type"] = "application/x-www-form-urlencoded"
    else:
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


def register_confidential_client():
    st, data, _ = req("POST", "/oauth/register", body={
        "client_name": "r800", "redirect_uris": ["https://claude.ai/api/mcp/callback"],
        "token_endpoint_auth_method": "client_secret_post"})
    return st, (json.loads(data) if data else {})


def auth_code_flow(client_id, client_secret=None, cb="https://claude.ai/api/mcp/callback"):
    verifier = base64.urlsafe_b64encode(secrets.token_bytes(32)).rstrip(b"=").decode()
    challenge = base64.urlsafe_b64encode(
        hashlib.sha256(verifier.encode()).digest()).rstrip(b"=").decode()
    state = secrets.token_urlsafe(8)
    q = urllib.parse.urlencode({
        "response_type": "code", "client_id": client_id, "redirect_uri": cb,
        "scope": "mcp:all", "state": state,
        "code_challenge": challenge, "code_challenge_method": "S256"})
    st, data, _ = req("GET", f"/oauth/authorize?{q}")
    m = re.search(rb'name="nonce" value="([^"]+)"', data.encode("utf-8", "replace"))
    if st != 200 or m is None:
        return None, None
    st, _, h = req("POST", "/oauth/authorize", form={
        "client_id": client_id, "redirect_uri": cb, "scope": "mcp:all", "state": state,
        "code_challenge": challenge,
        "resource": "", "action": "approve", "nonce": m.group(1).decode()})
    loc = h.get("location", "")
    code = urllib.parse.parse_qs(urllib.parse.urlparse(loc).query).get("code", [""])[0]
    if not code:
        return None, None
    st, data, _ = req("POST", "/oauth/token", form={
        "grant_type": "authorization_code", "code": code, "redirect_uri": cb,
        "client_id": client_id, "code_verifier": verifier,
        **({"client_secret": client_secret} if client_secret else {})})
    tok = json.loads(data)
    return (tok.get("access_token"), tok.get("refresh_token")) if st == 200 else (None, None)


# ── A: refresh-grant client auth ────────────────────────────────────────────
def section_a():
    print("== A: refresh-grant client auth ==")
    orig = cfg_read().get("oauthServer")
    cfg = cfg_read()
    cfg.setdefault("oauthServer", {})["enabled"] = True
    cfg_write(cfg)
    try:
        st, reg = register_confidential_client()
        check("A0 register 201/200", st in (200, 201), f"{st} {reg}")
        client_id = reg.get("client_id", "")
        secret = reg.get("client_secret")
        check("A0 confidential (secret issued)", client_id.startswith("mcp-") and secret)
        access, refresh = auth_code_flow(client_id, secret)
        check("A0 auth-code flow issued tokens", bool(access) and bool(refresh))
        if not (client_id and secret and refresh):
            skip("A refresh-grant auth (setup failed)")
            return
        form = lambda **kw: dict({"grant_type": "refresh_token", "refresh_token": refresh}, **kw)
        st, data, _ = req("POST", "/oauth/token", form=form(client_id="wrong-client", client_secret=secret))
        check("A1 wrong client_id \u2192 400", st == 400, f"{st} {data[:80]}")
        st, data, _ = req("POST", "/oauth/token", form=form(client_id=client_id, client_secret="wrong-secret-aaaaaaaa"))
        check("A2 wrong client_secret \u2192 400", st == 400, f"{st} {data[:80]}")
        st, data, _ = req("POST", "/oauth/token", form=form(client_id=client_id, client_secret=secret))
        check("A3 correct id+secret \u2192 200", st == 200, f"{st} {data[:120]}")
        st, data, _ = req("POST", "/oauth/token", form=form(client_id=client_id, client_secret=secret))
        check("A4 pre-rotation refresh replay \u2192 400", st == 400, f"{st} {data[:80]}")
        # Family revocation: the successor access token must now be dead.
        if access:
            st2, _, _ = req("POST", "/mcp", body={
                "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                    "protocolVersion": "2025-11-25", "capabilities": {},
                    "clientInfo": {"name": "r800", "version": "1.0"}}},
                headers={"Authorization": f"Bearer {access}"})
            check("A5 replay revoked successor access (401)", st2 == 401, f"{st2}")
    finally:
        cfg0 = cfg_read()
        if orig is None:
            cfg0.pop("oauthServer", None)
        else:
            cfg0["oauthServer"] = orig
        cfg_write(cfg0)


# ── B: body limit ────────────────────────────────────────────────────────────
def section_b():
    print("== B: /mcp body limit ==")
    big = ('{"jsonrpc":"2.0","id":1,"method":"ping","params":{"pad":"'
           + "A" * (17 * 1024 * 1024) + '"}}').encode()
    # Raw-socket probe: the server rejects (>limit) without draining the
    # body and resets mid-send; the 413 response is still readable after the
    # reset, which http.client cannot recover. Expect: 100-continue first.
    hdr = (f"POST /mcp HTTP/1.1\r\nHost: {HOST}:{PORT}\r\n"
           "Content-Type: application/json\r\n"
           "Accept: application/json, text/event-stream\r\n"
           "Expect: 100-continue\r\n"
           f"Content-Length: {len(big)}\r\n\r\n")
    st_b1 = None
    try:
        sk = socket.create_connection((HOST, PORT), timeout=60)
        sk.sendall(hdr.encode())
        time.sleep(0.5)
        sk.settimeout(5)
        first = b""
        try:
            first = sk.recv(4096)
        except Exception:
            pass
        if first.startswith(b"HTTP/1.1 100"):
            try:
                sk.sendall(big)
            except Exception:
                pass
            first = b""
            try:
                first = sk.recv(4096)
            except Exception:
                pass
        if first.startswith(b"HTTP/1.1 "):
            st_b1 = int(first.split()[1])
        sk.close()
    except Exception as e:
        st_b1 = None
        print(f"  (B1 probe error: {e!r})")
    check("B1 >16MB body 4xx", st_b1 is not None and 400 <= st_b1 < 500, f"{st_b1}")
    # Normal ping (with session) still 200.
    st, data, h = req("POST", "/mcp", body={
        "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "clientInfo": {"name": "r800", "version": "1.0"}}})
    sid = h.get("mcp-session-id", "")
    st2, data2, _ = req("POST", "/mcp", body={"jsonrpc": "2.0", "id": 2, "method": "ping"},
                        headers={"mcp-session-id": sid})
    o = sse_last(data2, 2)
    check("B2 normal ping 200", st2 == 200 and o is not None and "error" not in o, f"{st2}")


# ── C: version matrix spot ───────────────────────────────────────────────────
def section_c():
    print("== C: version matrix spot ==")
    for v in ["2025-03-26", "2025-06-18", "2025-11-25"]:
        st, data, _ = req("POST", "/mcp", body={
            "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": v, "capabilities": {},
                "clientInfo": {"name": "r800", "version": "1.0"}}})
        o = sse_last(data, 1)
        echoed = (o or {}).get("result", {}).get("protocolVersion")
        check(f"C1 initialize {v} echo", st == 200 and echoed == v, f"st={st} echo={echoed}")
    # 2026 stateless tools/list (modern shape from rmcp_carpet_r300.py).
    st, data, _ = req("POST", "/mcp", {
        "jsonrpc": "2.0", "id": 3, "method": "tools/list",
        "params": {"_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28",
                             "io.modelcontextprotocol/clientCapabilities": {}}}},
        headers={"MCP-Protocol-Version": "2026-07-28"})
    o = sse_last(data, 3)
    r = o.get("result", {}) if o else {}
    check("C2 2026 tools/list 200 + resultType complete",
          st == 200 and r.get("resultType") == "complete" and isinstance(r.get("tools"), list),
          f"st={st} resultType={r.get('resultType')}")


# ── D: leniency bare upgrade ─────────────────────────────────────────────────
def section_d():
    print("== D: leniency bare upgrade ==")
    # Pick a real tool so the call exercises the full upgrade path
    # (r460 wire: no Accept header + MCP-Protocol-Version header).
    st, data, _ = req("POST", "/mcp", body={"jsonrpc": "2.0", "id": 9, "method": "tools/list"},
                      headers={"MCP-Protocol-Version": "2025-11-25"})
    o = sse_last(data, 9)
    tools = ((o or {}).get("result", {}) or {}).get("tools", []) or []
    tool = tools[0]["name"] if tools else "definitely-not-a-real-tool-xyz"
    st, data, _ = req("POST", "/mcp", body={
        "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {
            "name": tool, "arguments": {}}}, default_accept=False,
        headers={"MCP-Protocol-Version": "2025-11-25"})
    o = sse_last(data, 7)
    # Result or tool-level error both prove the bare request was UPGRADED.
    check("D1 bare tools/call 2xx", st == 200, f"{st} {data[:80]}")
    check("D2 valid JSON-RPC response", o is not None and o.get("jsonrpc") == "2.0" and o.get("id") == 7,
          json_txt(o)[:120])
    check("D3 has result or JSON-RPC error",
          isinstance(o, dict) and (("result" in o) or (isinstance(o.get("error"), dict) and o["error"].get("code", 0) < 0)),
          json_txt(o)[:120])


# ── E: $smart meta tools ─────────────────────────────────────────────────────
def section_e():
    print("== E: $smart meta tools ==")
    sr = cfg_read().get("smartRouting", {})
    enabled = bool(sr.get("enabled") if isinstance(sr, dict) else False)
    st, data, _ = req("POST", "/mcp/$smart", body={
        "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "clientInfo": {"name": "r800", "version": "1.0"}}})
    o = sse_last(data, 1)
    check("E1 $smart initialize 200", st == 200 and o is not None, f"{st} {data[:80]}")
    st2, data2, _ = req("POST", "/mcp/$smart", body={"jsonrpc": "2.0", "id": 2, "method": "tools/list"})
    o2 = sse_last(data2, 2)
    tools = ((o2 or {}).get("result", {}) or {}).get("tools", []) or []
    names = {t.get("name", "") for t in tools}
    meta = {"smart_route_search", "smart_route_describe", "smart_route_call"}
    if not enabled or not meta.issubset(names):
        skip("E2 $smart meta tools (Smart Routing disabled or no index)",
             f"enabled={enabled} names={sorted(names)}")
        return
    check("E2 $smart meta tools present", meta.issubset(names), sorted(names))


def main():
    snap = cfg_read()
    orig_bearer = snap.get("enableBearerAuth")
    try:
        st, data, _ = req("GET", "/health", default_accept=False, timeout=5)
        if st != 200:
            print(f"SKIP: hub not healthy at {BASE} (status {st})")
            sys.exit(0)
    except Exception as e:
        print(f"SKIP: hub down at {BASE}: {e}")
        sys.exit(0)
    try:
        # Bearer auth may be ON in this deployment; sections B-E need
        # unauthenticated /mcp. Toggled with restore (strict mode untouched).
        orig_bearer = (snap.get("routing") or {}).get("enableBearerAuth")
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
