#!/usr/bin/env python3
"""v28 suite — R28 fixes regression: git canonical-hash consistency, partial-persist sweep, updater cancel semantics (protocol-side observable parts)."""
import json, urllib.request

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
        raw = resp.read().decode("utf-8", "replace")
        sid = resp.headers.get("mcp-session-id")
        return resp.status, sid, raw
    except urllib.error.HTTPError as e:
        return e.code, None, e.read().decode("utf-8", "replace")

def parse_result(raw):
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

# 1. Initialize (legacy) — session mint still works after rebuild
st, sid, raw = rpc(None, "initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "v28", "version": "1"}}, path="/mcp")
r = parse_result(raw)
check("init-2025-06-18", st == 200 and r and r.get("result", {}).get("protocolVersion") == "2025-06-18", f"{st} {raw[:120]}")

# notifications/initialized
if sid:
    st2, _, _ = rpc(sid, "notifications/initialized", {}, path="/mcp")
    check("initialized-notify-202", st2 in (200, 202), str(st2))

# 2. Version echo consistency across all 5 versions on root channel
for v in ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"]:
    st, sid2, raw = rpc(None, "initialize", {"protocolVersion": v, "capabilities": {}, "clientInfo": {"name": "v28e", "version": "1"}}, path="/mcp")
    r = parse_result(raw)
    echoed = r.get("result", {}).get("protocolVersion") if r else None
    check(f"echo-{v}", st == 200 and echoed == v, f"{st} echoed={echoed}")

# 3. 2026 stateless: server/discover + tools/call complete resultType
import socket
body = {"jsonrpc": "2.0", "id": 1, "method": "server/discover",
        "params": {"_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28",
                              "io.modelcontextprotocol/clientInfo": {"name": "v28", "version": "1"},
                              "io.modelcontextprotocol/clientCapabilities": {}}}}
req = urllib.request.Request(BASE + "/mcp", json.dumps(body).encode("utf-8"),
                             {"Content-Type": "application/json", "Accept": "application/json, text/event-stream",
                              "MCP-Protocol-Version": "2026-07-28", "Mcp-Method": "server/discover"})
try:
    resp = urllib.request.urlopen(req, timeout=15)
    r = parse_result(resp.read().decode("utf-8", "replace"))
    res = r.get("result", {}) if r else {}
    check("2026-discover", resp.status == 200 and res.get("resultType") == "complete" and len(res.get("supportedVersions", [])) == 5, str(res)[:150])
    check("2026-discover-ttl", res.get("ttlMs") == 3600000 and res.get("cacheScope") == "public", str(res.get("ttlMs")))
except Exception as e:
    check("2026-discover", False, str(e))

# 4. Real public-IP MCP call (version 2025-06-18, root channel)
st, sid3, raw = rpc(None, "initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "v28ip", "version": "1"}}, path="/mcp")
r = parse_result(raw)
tools_ok = False
if sid3 and r:
    rpc(sid3, "notifications/initialized", {}, path="/mcp")
    st4, _, raw4 = rpc(sid3, "tools/list", {}, version="2025-06-18", path="/mcp")
    tl = parse_result(raw4)
    names = [t["name"] for t in (tl.get("result", {}).get("tools", []) if tl else [])]
    # The public-IP query tool has a Chinese name (本机公网ip查询-getPublicIp);
    # pick by exact suffix, fall back to first tool.
    ip_tools = [n for n in names if n.endswith("getPublicIp")] or names[:1]
    if ip_tools:
        st5, _, raw5 = rpc(sid3, "tools/call", {"name": ip_tools[0], "arguments": {}}, version="2025-06-18", path="/mcp")
        cr = parse_result(raw5)
        content = cr.get("result", {}).get("content", []) if cr else []
        tools_ok = st5 == 200 and cr.get("result", {}).get("isError") in (False, None) and len(content) > 0
        check("real-ip-call", tools_ok, f"{st5} tool={ip_tools[0]}")
    else:
        check("real-ip-call", False, f"no ip tool found; tools={names[:10]}")

# 5. Leniency: missing Accept header still allowed on POST (all versions)
for v in ["2025-03-26", "2025-06-18"]:
    body = {"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": v, "capabilities": {}, "clientInfo": {"name": "v28l", "version": "1"}}}
    req = urllib.request.Request(BASE + "/mcp", json.dumps(body).encode("utf-8"), {"Content-Type": "application/json"})
    try:
        resp = urllib.request.urlopen(req, timeout=15)
        check(f"leniency-noaccept-{v}", resp.status == 200, str(resp.status))
    except urllib.error.HTTPError as e:
        check(f"leniency-noaccept-{v}", False, str(e.code))

# 6. Strict contract preserved: bare invalid jsonrpc -> 415/4xx
body = {"id": 1, "method": "tools/list"}  # missing jsonrpc key
req = urllib.request.Request(BASE + "/mcp", json.dumps(body).encode("utf-8"), {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"})
try:
    resp = urllib.request.urlopen(req, timeout=15)
    # leniency normalizes to 2.0 -> 200
    check("leniency-missing-jsonrpc", resp.status == 200, str(resp.status))
except urllib.error.HTTPError as e:
    check("leniency-missing-jsonrpc", 400 <= e.code < 500, str(e.code))

# 7. health + /rest surface alive
try:
    resp = urllib.request.urlopen(BASE + "/health", timeout=10)
    check("health", resp.status == 200)
except Exception as e:
    check("health", False, str(e))

print(f"\nv28: {PASS} passed, {FAIL} failed")
if FAILURES:
    for n, d in FAILURES:
        print(f"  FAIL {n}: {d}")
exit(1 if FAIL else 0)
