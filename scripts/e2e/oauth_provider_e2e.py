#!/usr/bin/env python3
"""OAuth Provider E2E (v1.1.1004 §B5): simulates a Claude-style client against
the hub's /oauth/* endpoints + dual-track auth on /mcp and /mcp/$smart.

Covers: well-known metadata → RFC 7591 dynamic registration → consent page
(approve via HTTP POST, no browser) → authorization code → PKCE token exchange
→ /mcp initialize + tools/call with the OAuth token → $smart channel →
refresh rotation → revoke → 401.
Negative paths: disabled 404, wrong PKCE, redirect mismatch, expired/replayed
code, unauthenticated /mcp 401 shape.
"""
import json, urllib.request, urllib.parse, sqlite3, os, sys, hashlib, base64, secrets, time

BASE = os.environ.get("MCPHUB_BASE", "http://127.0.0.1:23333")
DB = os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")
PASS, FAIL = 0, 0
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

def req(method, path, body=None, headers=None, form=None, follow=False):
    url = BASE + path
    data = None
    hdrs = dict(headers or {})
    if form is not None:
        data = urllib.parse.urlencode(form).encode()
        hdrs.setdefault("Content-Type", "application/x-www-form-urlencoded")
    elif body is not None:
        data = json.dumps(body).encode()
        hdrs.setdefault("Content-Type", "application/json")
    r = urllib.request.Request(url, data=data, headers=hdrs, method=method)

    class NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *a, **k):
            return None
    opener = urllib.request.build_opener(NoRedirect) if not follow else urllib.request.build_opener()
    try:
        resp = opener.open(r, timeout=15)
        return resp.status, dict(resp.headers), resp.read()
    except urllib.error.HTTPError as e:
        return e.code, dict(e.headers), e.read()

def set_provider(on, original=None):
    con = sqlite3.connect(DB)
    row = con.execute("SELECT config_json FROM system_config WHERE id=1").fetchone()
    cfg = json.loads(row[0]) if row and row[0] else {}
    if original is None:
        original = cfg.get("oauthServer")
    if on:
        cfg.setdefault("oauthServer", {})["enabled"] = True
    else:
        cfg.pop("oauthServer", None)
    con.execute("UPDATE system_config SET config_json=? WHERE id=1", (json.dumps(cfg),))
    con.commit(); con.close()
    time.sleep(0.3)
    return original

def restore_provider(original):
    con = sqlite3.connect(DB)
    row = con.execute("SELECT config_json FROM system_config WHERE id=1").fetchone()
    cfg = json.loads(row[0]) if row and row[0] else {}
    if original is None:
        cfg.pop("oauthServer", None)
    else:
        cfg["oauthServer"] = original
    con.execute("UPDATE system_config SET config_json=? WHERE id=1", (json.dumps(cfg),))
    con.commit(); con.close()

def main():
    con = sqlite3.connect(DB)
    row = con.execute("SELECT config_json FROM system_config WHERE id=1").fetchone()
    con.close()
    cfg0 = json.loads(row[0]) if row and row[0] else {}
    original = cfg0.get("oauthServer")
    try:
        run(original)
    finally:
        restore_provider(original)
    print(f"\n{PASS} passed, {FAIL} failed")
    if FAILURES:
        print("Failures:", *FAILURES, sep="\n  - ")
    sys.exit(1 if FAIL else 0)

def run(original):
    # ── N1. Provider disabled → 404 on all provider surfaces ──
    st, _, b = req("GET", "/.well-known/oauth-authorization-server")
    check("N1 disabled metadata 404", st == 404, f"got {st}")
    st, _, _ = req("POST", "/oauth/register", body={"redirect_uris": ["https://x/cb"]})
    check("N1b disabled register 404", st == 404, f"got {st}")
    st, _, _ = req("POST", "/oauth/token", form={"grant_type": "authorization_code"})
    check("N1c disabled token 404", st == 404, f"got {st}")

    set_provider(True)

    # ── P1. Well-known metadata ──
    st, _, b = req("GET", "/.well-known/oauth-authorization-server")
    md = json.loads(b)
    check("P1 metadata 200", st == 200, f"{st} {b[:100]}")
    check("P1 issuer", md.get("issuer", "").startswith("http://"))
    check("P1 S256 only", md.get("code_challenge_methods_supported") == ["S256"])
    check("P1 register endpoint", "/oauth/register" in md.get("registration_endpoint", ""))

    # Protected resource points at ourselves when enabled.
    st, _, b = req("GET", "/.well-known/oauth-protected-resource")
    pr = json.loads(b)
    check("P1b protected-resource authorization_servers == issuer", pr.get("authorization_servers") == [pr.get("resource")], str(pr)[:120])

    # ── P2. Dynamic registration ──
    cb = "https://claude.ai/api/mcp/callback"
    st, _, b = req("POST", "/oauth/register", body={"client_name": "E2E Claude", "redirect_uris": [cb]})
    reg = json.loads(b)
    check("P2 register 201", st == 201, f"{st} {b[:120]}")
    client_id = reg.get("client_id", "")
    secret = reg.get("client_secret")
    check("P2 client_id issued", client_id.startswith("mcp-"))
    check("P2 grant types", "authorization_code" in reg.get("grant_types", []))

    # Bad redirect scheme rejected.
    st, _, _ = req("POST", "/oauth/register", body={"redirect_uris": ["javascript:alert(1)"]})
    check("N2 bad redirect 400", st == 400, f"got {st}")

    # ── P3. Consent page + approve ──
    verifier = base64.urlsafe_b64encode(secrets.token_bytes(32)).rstrip(b"=").decode()
    challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).rstrip(b"=").decode()
    state = secrets.token_urlsafe(16)
    q = urllib.parse.urlencode({
        "response_type": "code", "client_id": client_id, "redirect_uri": cb,
        "scope": "mcp:all", "state": state, "code_challenge": challenge,
        "code_challenge_method": "S256",
    })
    st, _, b = req("GET", f"/oauth/authorize?{q}")
    check("P3 consent page 200 html", st == 200 and b"E2E Claude" in b, f"{st}")
    check("P3 scope echoed", b"mcp:all" in b)
    import re as _re
    m = _re.search(rb'name="nonce" value="([^"]+)"', b)
    check("P3 nonce rendered", m is not None, b[:200])
    nonce = m.group(1).decode() if m else ""

    # CSRF: posting WITHOUT the nonce is rejected.
    st, _, _ = req("POST", "/oauth/authorize", form={
        "client_id": client_id, "redirect_uri": cb, "scope": "mcp:all", "state": state,
        "code_challenge": challenge, "resource": "", "action": "approve", "nonce": "",
    })
    check("N3c missing nonce rejected", st == 400, f"got {st}")

    st, h, _ = req("POST", "/oauth/authorize", form={
        "client_id": client_id, "redirect_uri": cb, "scope": "mcp:all", "state": state,
        "code_challenge": challenge, "resource": "", "action": "approve", "nonce": nonce,
    })
    loc = h.get("Location", "") or h.get("location", "")
    check("P3 approve 302", st == 302, f"{st}")
    parsed = urllib.parse.urlparse(loc)
    params = urllib.parse.parse_qs(parsed.query)
    code = params.get("code", [""])[0]
    check("P3 state round-trip", params.get("state", [""])[0] == state, loc)
    check("P3 code issued", len(code) > 20)

    # Deny path.
    st, _, b2 = req("GET", f"/oauth/authorize?{q}")
    import re as _re2
    nonce2 = (_re2.search(rb'name="nonce" value="([^"]+)"', b2) or [None, b""])[1].decode()
    st, h2, _ = req("POST", "/oauth/authorize", form={
        "client_id": client_id, "redirect_uri": cb, "scope": "mcp:all", "state": state,
        "code_challenge": challenge, "resource": "", "action": "deny", "nonce": nonce2,
    })
    loc2 = h2.get("Location", "") or h2.get("location", "")
    check("N3 deny → access_denied", "error=access_denied" in loc2, loc2)

    # Unregistered redirect_uri rejected on page render.
    q_bad = urllib.parse.urlencode({**dict(urllib.parse.parse_qsl(q)), "redirect_uri": "https://evil.example/cb"})
    st, _, _ = req("GET", f"/oauth/authorize?{q_bad}")
    check("N3b unregistered redirect rejected", st == 400, f"got {st}")

    # ── P4. Token exchange (PKCE) ──
    st, _, b = req("POST", "/oauth/token", form={
        "grant_type": "authorization_code", "code": code, "redirect_uri": cb,
        "client_id": client_id, "code_verifier": verifier,
    })
    tok = json.loads(b)
    check("P4 token 200", st == 200, f"{st} {b[:120]}")
    access = tok.get("access_token", "")
    refresh = tok.get("refresh_token", "")
    check("P4 bearer token_type", tok.get("token_type") == "Bearer")
    check("P4 refresh issued", len(refresh) > 20)

    # Wrong verifier fails AND burns the code (single use).
    st, _, b = req("POST", "/oauth/token", form={
        "grant_type": "authorization_code", "code": code, "redirect_uri": cb,
        "client_id": client_id, "code_verifier": "wrong-verifier-0000000000000000000000000aa",
    })
    check("N4 replay/wrong-verifier rejected", st == 400, f"{st} {b[:80]}")

    # ── P5. /mcp with the OAuth token ──
    auth = {"Authorization": f"Bearer {access}"}
    init_body = {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {},
        "clientInfo": {"name": "e2e-claude", "version": "1.0"}}}
    st, _, b = req("POST", "/mcp", body=init_body, headers={**auth, "Accept": "application/json, text/event-stream"})
    check("P5 initialize 200", st == 200, f"{st} {b[:120]}")

    # Without a token (nothing enabled previously) → 401 with resource metadata.
    st, h, b = req("POST", "/mcp", body=init_body, headers={"Accept": "application/json, text/event-stream"})
    # Note: bearer auth may be off and this request ran before provider enabled;
    # now provider is ON so unauthenticated /mcp must 401.
    check("P5b unauthenticated /mcp 401", st == 401, f"got {st}")
    www = h.get("www-authenticate", "") or h.get("WWW-Authenticate", "")
    check("P5c WWW-Authenticate has resource_metadata", "resource_metadata" in www, www[:120])

    # ── P6. $smart channel reachable with the token ──
    st, _, b = req("GET", "/mcp/$smart", headers={**auth, "Accept": "text/event-stream"})
    # rmcp GET without a session -> 400 "Session ID is required"; the point
    # of this check is that the OAuth auth gate PASSED (401/403 would mean
    # the token was rejected). 200 (stateless SSE) or 400 (session-required)
    # both prove the gate passed; anything else fails.
    check("P6 $smart GET auth gate passed", st in (200, 400), f"got {st}")

    # ── P7. Refresh rotation ──
    st, _, b = req("POST", "/oauth/token", form={"grant_type": "refresh_token", "refresh_token": refresh})
    tok2 = json.loads(b)
    check("P7 refresh 200", st == 200, f"{st} {b[:120]}")
    access2 = tok2.get("access_token", "")
    check("P7 rotation new access", access2 and access2 != access)
    # New token still valid on /mcp (must run BEFORE the replay test —
    # replay triggers family revocation per RFC 6749 §10.4).
    st, _, _ = req("POST", "/mcp", body=init_body, headers={**{"Authorization": f"Bearer {access2}"}, "Accept": "application/json, text/event-stream"})
    check("P7b new access works", st == 200, f"got {st}")

    # RFC 6749 §6: refresh grant authenticates the client. A wrong
    # client_id for a valid refresh token must be rejected.
    st, _, _ = req("POST", "/oauth/token", form={"grant_type": "refresh_token", "refresh_token": tok2.get("refresh_token", ""), "client_id": "wrong-client"})
    check("N7c refresh wrong client_id rejected", st == 400, f"got {st}")

    # Replayed (rotated) refresh = theft signal → family revoked + error.
    st, _, _ = req("POST", "/oauth/token", form={"grant_type": "refresh_token", "refresh_token": refresh})
    check("N7 refresh replay rejected", st == 400, f"got {st}")
    # Family revocation: successor access is dead too.
    st, _, _ = req("POST", "/mcp", body=init_body, headers={**{"Authorization": f"Bearer {access2}"}, "Accept": "application/json, text/event-stream"})
    check("N7b replay revoked successor access", st == 401, f"got {st}")

    # ── P8. Revoke → everything 401s ──
    con = sqlite3.connect(DB)
    con.execute("UPDATE oauth_clients SET revoked=1 WHERE client_id=?", (client_id,))
    con.commit(); con.close()
    time.sleep(0.3)
    st, _, _ = req("POST", "/mcp", body=init_body, headers={**{"Authorization": f"Bearer {access2}"}, "Accept": "application/json, text/event-stream"})
    check("P8 revoked client 401", st == 401, f"got {st}")

if __name__ == "__main__":
    main()
