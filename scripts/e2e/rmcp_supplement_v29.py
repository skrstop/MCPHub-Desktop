#!/usr/bin/env python3
"""v29 suite — R29 fixes regression: tray menu handler idempotence is Rust-side (verified by cargo), here: protocol surface re-verification after rebuild + menu-action emit paths unaffected."""
import json, urllib.request, sys

BASE = "http://127.0.0.1:23333"
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

def rpc(session, method, params=None, version=None, path="/mcp"):
    body = {"jsonrpc": "2.0", "id": 1, "method": method}
    if params is not None:
        body["params"] = params
    headers = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
    if version:
        headers["MCP-Protocol-Version"] = version
    if session:
        headers["mcp-session-id"] = session
    req = urllib.request.Request(BASE + path, json.dumps(body).encode("utf-8"), headers)
    try:
        resp = urllib.request.urlopen(req, timeout=15)
        return resp.status, resp.headers.get("mcp-session-id"), resp.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as e:
        return e.code, None, e.read().decode("utf-8", "replace")

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

# 1. All 5 versions echo consistency after rebuild
for v in ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"]:
    st, sid, raw = rpc(None, "initialize", {"protocolVersion": v, "capabilities": {}, "clientInfo": {"name": "v29", "version": "1"}}, path="/mcp")
    r = parse(raw)
    check(f"echo-{v}", st == 200 and r and r.get("result", {}).get("protocolVersion") == v, f"{st}")

# 2. Real public-IP call per legacy version (one each — R29 re-verification)
import re
for v in ["2025-03-26", "2025-06-18", "2025-11-25"]:
    st, sid, raw = rpc(None, "initialize", {"protocolVersion": v, "capabilities": {}, "clientInfo": {"name": "v29ip", "version": "1"}}, path="/mcp")
    if not sid:
        check(f"ip-{v}", False, "no session")
        continue
    rpc(sid, "notifications/initialized", {}, version=v, path="/mcp")
    st2, _, raw2 = rpc(sid, "tools/call", {"name": "本机公网ip查询-getPublicIp", "arguments": {}}, version=v, path="/mcp")
    cr = parse(raw2)
    text = ""
    try:
        text = cr["result"]["content"][0]["text"]
    except Exception:
        pass
    body = text.split("Response Body:")[-1]
    m = re.search(r"\b\d{1,3}(\.\d{1,3}){3}\b", body)
    check(f"ip-{v}", st2 == 200 and not cr.get("result", {}).get("isError") and m, f"{st2} {text[-60:]}")

# 3. 2026 stateless discover + real call
body = {"jsonrpc": "2.0", "id": 1, "method": "server/discover",
        "params": {"_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28",
                              "io.modelcontextprotocol/clientInfo": {"name": "v29", "version": "1"},
                              "io.modelcontextprotocol/clientCapabilities": {}}}}
req = urllib.request.Request(BASE + "/mcp", json.dumps(body).encode("utf-8"),
                             {"Content-Type": "application/json", "Accept": "application/json, text/event-stream",
                              "MCP-Protocol-Version": "2026-07-28", "Mcp-Method": "server/discover"})
try:
    resp = urllib.request.urlopen(req, timeout=15)
    r = parse(resp.read().decode("utf-8", "replace"))
    res = r.get("result", {}) if r else {}
    check("2026-discover", resp.status == 200 and res.get("resultType") == "complete" and res.get("ttlMs") == 3600000, str(res)[:120])
except Exception as e:
    check("2026-discover", False, str(e))

# 2026 stateless real IP call
body = {"jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": "本机公网ip查询-getPublicIp", "arguments": {},
                    "_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28",
                               "io.modelcontextprotocol/clientInfo": {"name": "v29", "version": "1"},
                               "io.modelcontextprotocol/clientCapabilities": {}}}}
req = urllib.request.Request(BASE + "/mcp", json.dumps(body).encode("utf-8"),
                             {"Content-Type": "application/json", "Accept": "application/json, text/event-stream",
                              "MCP-Protocol-Version": "2026-07-28", "Mcp-Method": "tools/call",
                              "Mcp-Name": "=?base64?" + __import__("base64").b64encode("本机公网ip查询-getPublicIp".encode()).decode() + "?="})
try:
    resp = urllib.request.urlopen(req, timeout=30)
    r = parse(resp.read().decode("utf-8", "replace"))
    text = ""
    try:
        text = r["result"]["content"][0]["text"]
    except Exception:
        pass
    m = re.search(r"\b\d{1,3}(\.\d{1,3}){3}\b", text.split("Response Body:")[-1])
    check("ip-2026", resp.status == 200 and r.get("result", {}).get("resultType") == "complete" and m, f"{text[-60:]}")
except Exception as e:
    check("ip-2026", False, str(e))

# 4. health
try:
    resp = urllib.request.urlopen(BASE + "/health", timeout=10)
    check("health", resp.status == 200)
except Exception as e:
    check("health", False, str(e))

print(f"\nv29: {PASS} passed, {FAIL} failed")
if FAILURES:
    for n, d in FAILURES:
        print(f"  FAIL {n}: {d}")
sys.exit(1 if FAIL else 0)
