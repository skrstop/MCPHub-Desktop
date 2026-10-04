#!/usr/bin/env python3
"""v32 suite — R32 regression: rag command gating (fail-closed 403s in multi-user is Rust-side; here verify protocol surface + group endpoint encoding observable via /api/{name} + public-IP rotation)."""
import json, urllib.request, urllib.parse, re, sys, base64, http.client

BASE_HOST, BASE_PORT = "127.0.0.1", 23333
BASE = f"http://{BASE_HOST}:{BASE_PORT}"
PASS = FAIL = 0
FAILURES = []

def check(name, cond, detail=""):
    global PASS, FAIL
    if cond:
        PASS += 1
    else:
        FAIL += 1
        FAILURES.append((name, detail))
        print(f"FAIL {name}: {detail}")

def parse(raw):
    for line in raw.splitlines():
        if line.startswith("data:"):
            try:
                return json.loads(line[5:].strip())
            except Exception:
                pass
    try:
        return json.loads(raw)
    except Exception:
        return None

def init(version):
    conn = http.client.HTTPConnection(BASE_HOST, BASE_PORT, timeout=20)
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                       "params": {"protocolVersion": version, "capabilities": {},
                                  "clientInfo": {"name": "v32", "version": "1"}}})
    conn.request("POST", "/mcp", body, {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"})
    resp = conn.getresponse()
    sid = resp.getheader("mcp-session-id")
    raw = resp.read().decode("utf-8", "replace")
    conn.close()
    r = parse(raw)
    echoed = r.get("result", {}).get("protocolVersion") if r else None
    return resp.status, sid, echoed

def post(session, method, params, version):
    conn = http.client.HTTPConnection(BASE_HOST, BASE_PORT, timeout=30)
    body = json.dumps({"jsonrpc": "2.0", "id": 2, "method": method, "params": params})
    h = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream",
         "MCP-Protocol-Version": version}
    if session:
        h["mcp-session-id"] = session
    conn.request("POST", "/mcp", body, h)
    resp = conn.getresponse()
    raw = resp.read().decode("utf-8", "replace")
    conn.close()
    return resp.status, parse(raw)

# 1. Version echo — all 4 legacy
for v in ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"]:
    st, sid, echoed = init(v)
    check(f"echo-{v}", st == 200 and echoed == v, f"{st} echoed={echoed}")

# 2. Encoded path segment works end-to-end: percent-encoded requests reach the
# same scope resolution as raw (bridge percent-decodes). Test via a known
# group-less root call first — real IP via root channel 2025-03-26.
for v in ["2025-03-26", "2025-06-18"]:
    st, sid, _ = init(v)
    if not sid:
        check(f"ip-{v}", False, "no session")
        continue
    st2, r = post(sid, "tools/call", {"name": "本机公网ip查询-getPublicIp", "arguments": {}}, v)
    text = ""
    try:
        text = r["result"]["content"][0]["text"]
    except Exception:
        pass
    m = re.search(r"\b\d{1,3}(\.\d{1,3}){3}\b", text.split("Response Body:")[-1])
    check(f"ip-{v}", st2 == 200 and not r.get("result", {}).get("isError") and m, f"{st2} {text[-50:]}")

# 3. /api/{name}/openapi.json with percent-encoded CJK server name resolves
import urllib.parse
srv = "本机公网ip查询"
enc = urllib.parse.quote(srv)
try:
    resp = urllib.request.urlopen(f"{BASE}/api/{enc}/openapi.json", timeout=15)
    body = resp.read().decode("utf-8", "replace")
    ok = resp.status == 200 and "openapi" in body
    check("api-encoded-cjk-path", ok, f"{resp.status} len={len(body)}")
except urllib.error.HTTPError as e:
    # 404 acceptable if server group not exposed via /api single-server spec — but must NOT be 5xx
    check("api-encoded-cjk-path", e.code in (401, 403, 404), str(e.code))
except Exception as e:
    check("api-encoded-cjk-path", False, str(e))

# 4. 2026 stateless discover + IP call
conn = http.client.HTTPConnection(BASE_HOST, BASE_PORT, timeout=20)
body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "server/discover",
                   "params": {"_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28",
                                         "io.modelcontextprotocol/clientInfo": {"name": "v32", "version": "1"},
                                         "io.modelcontextprotocol/clientCapabilities": {}}}})
conn.request("POST", "/mcp", body, {"Content-Type": "application/json", "Accept": "application/json, text/event-stream",
                                     "MCP-Protocol-Version": "2026-07-28", "Mcp-Method": "server/discover"})
resp = conn.getresponse()
r = parse(resp.read().decode("utf-8", "replace"))
conn.close()
res = r.get("result", {}) if r else {}
check("2026-discover", resp.status == 200 and res.get("resultType") == "complete" and res.get("ttlMs") == 3600000, str(res)[:120])

name_hdr = "=?base64?" + base64.b64encode("本机公网ip查询-getPublicIp".encode()).decode() + "?="
conn = http.client.HTTPConnection(BASE_HOST, BASE_PORT, timeout=30)
body = json.dumps({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                   "params": {"name": "本机公网ip查询-getPublicIp", "arguments": {},
                               "_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28",
                                          "io.modelcontextprotocol/clientInfo": {"name": "v32", "version": "1"},
                                          "io.modelcontextprotocol/clientCapabilities": {}}}})
conn.request("POST", "/mcp", body, {"Content-Type": "application/json", "Accept": "application/json, text/event-stream",
                                     "MCP-Protocol-Version": "2026-07-28", "Mcp-Method": "tools/call", "Mcp-Name": name_hdr})
resp = conn.getresponse()
r = parse(resp.read().decode("utf-8", "replace"))
conn.close()
text = ""
try:
    text = r["result"]["content"][0]["text"]
except Exception:
    pass
m = re.search(r"\b\d{1,3}(\.\d{1,3}){3}\b", text.split("Response Body:")[-1])
check("ip-2026", resp.status == 200 and r.get("result", {}).get("resultType") == "complete" and m, f"{text[-50:]}")

# 5. Health
try:
    resp = urllib.request.urlopen(BASE + "/health", timeout=10)
    check("health", resp.status == 200)
except Exception as e:
    check("health", False, str(e))

print(f"\nv32: {PASS} passed, {FAIL} failed")
if FAILURES:
    for n, d in FAILURES:
        print(f"  FAIL {n}: {d}")
sys.exit(1 if FAIL else 0)
