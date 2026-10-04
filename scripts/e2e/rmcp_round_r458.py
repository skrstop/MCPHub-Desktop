#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""第 8 轮复核（R458+）缺口补全套件。覆盖 A10 审计的 P1/P2 真空格子：

G1  /mcp/{group} × 2026-07-28 × 真实公网 IP 工具调用（组通道 2026 无状态首覆盖）
G2  /mcp/$smart × 2026-07-28 × search_tools + call_tool 真实公网 IP
G3  meta 工具自递归回归：smart_route_call 目标为 meta 工具 → 快速报错（不挂死）
G4  tasks/update 双代际：legacy 会话 -32601 门控；2026 未知 id -32602
G5  json_response=true 显式形态：modern 请求 Content-Type=application/json（无 SSE 帧）
G6  bearer × server/discover：无 key 401 / 有效 key 200（测毕还原）
G7  bearer allowed_servers=[] fail-closed：空列表拒绝一切（测毕还原）
G8  空 mcp-session-id 头：宽松剥离升格放行（严格模式回归由既有套件守护）

依赖：服务器运行于 :23333，「本机公网ip查询」(openapi) 已连接，分组 Test 存在。
"""
import http.client, json, sqlite3, os, re, sys, time, urllib.parse, socket

HOST, PORT = "localhost", 23333
DB = os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")
IP_SERVER = "本机公网ip查询"
SCOPE = urllib.parse.quote(IP_SERVER)
GROUP_PATH = "/mcp/Test"
PV = "io.modelcontextprotocol/protocolVersion"
CLIENT_META = {"io.modelcontextprotocol/clientInfo": {"name": "r8", "version": "1"},
               "io.modelcontextprotocol/clientCapabilities": {}}
IP_RE = re.compile(r"(?<![\d.])(?:\d{1,3}\.){3}\d{1,3}(?![\d.])")

PASS, FAIL, FAILED, SKIPPED = 0, 0, [], 0
def check(name, cond, detail=""):
    global PASS, FAIL
    if cond:
        PASS += 1; print(f"PASS | {name}" + (f" ({detail})" if detail else ""))
    else:
        FAIL += 1; FAILED.append(name); print(f"FAIL | {name} :: {str(detail)[:200]}")
def skip(name, detail=""):
    global SKIPPED
    SKIPPED += 1; print(f"SKIP | {name} :: {detail}")

def valid_ip(t):
    m = IP_RE.search(t or "")
    if not m: return None
    try:
        import ipaddress; ipaddress.ip_address(m.group(0)); return m.group(0)
    except ValueError: return None

def req(method, path, payload=None, headers=None, timeout=90):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
    if headers: h.update(headers)
    body = json.dumps(payload, ensure_ascii=False).encode("utf-8") if payload is not None else None
    try:
        c.request(method, path, body=body, headers=h)
        r = c.getresponse(); data = r.read().decode("utf-8", "replace")
        st = r.status; sid = r.getheader("mcp-session-id"); ct = r.getheader("content-type") or ""
    finally:
        c.close()
    obj = None
    if data.strip().startswith("{"):
        try: obj = json.loads(data)
        except Exception: pass
    if obj is None:
        frames = []
        for l in data.split("\n"):
            if l.startswith("data: ") and l[6:].strip():
                try: frames.append(json.loads(l[6:]))
                except Exception: pass
        obj = frames[-1] if frames else None
    return st, sid, obj, ct, data

def modern(body_id, name, arguments):
    return {"jsonrpc": "2.0", "id": body_id, "method": "tools/call",
            "params": {"name": name, "arguments": arguments,
                       "_meta": {PV: "2026-07-28", **CLIENT_META}}}

# ── G1: group × 2026 × real IP ────────────────────────────────────────────────
def g1():
    st, sid, obj, ct, raw = req("POST", GROUP_PATH, modern(1, "placeholder", {}))
    # tools/list first to find the prefixed IP tool in the group channel
    st0, sid0, obj0, _, _ = req("POST", GROUP_PATH, {"jsonrpc": "2.0", "id": 1, "method": "tools/list",
        "params": {"_meta": {PV: "2026-07-28", **CLIENT_META}}})
    tools = (obj0 or {}).get("result", {}).get("tools", [])
    tname = next((t["name"] for t in tools if t["name"].endswith("getPublicIp")), None)
    check("G1 group tools/list (2026, stateless)", st0 == 200 and obj0 and "result" in obj0, f"st={st0} n={len(tools)}")
    check("G1 group 2026 resultType complete", (obj0 or {}).get("result", {}).get("resultType") == "complete",
          str((obj0 or {}).get("result", {}).get("resultType")))
    if not tname:
        skip("G1 group IP 真实调用", "group Test 无 IP 工具"); return
    st1, _, obj1, _, _ = req("POST", GROUP_PATH, modern(2, tname, {}))
    txt = "".join(str(c.get("text", "")) for c in (obj1 or {}).get("result", {}).get("content", []))
    ip = valid_ip(txt)
    check("G1 group × 2026 公网IP真实调用", st1 == 200 and ip and (obj1.get("result", {}).get("isError") is False),
          f"st={st1} ip={ip} txt={txt[:50]}")

# ── G2: $smart × 2026 × real IP ──────────────────────────────────────────────
def g2():
    base = "/mcp/%24smart"
    st, _, obj, _, _ = req("POST", base, {"jsonrpc": "2.0", "id": 1, "method": "tools/list",
        "params": {"_meta": {PV: "2026-07-28", **CLIENT_META}}})
    tools = (obj or {}).get("result", {}).get("tools", [])
    names = [t["name"] for t in tools]
    # progressiveDisclosure=false exposes exactly 2 tools (search + call);
    # progressive mode would expose 3 (incl. smart_route_describe).
    check("G2 $smart tools/list (2026 meta 工具)", st == 200 and len(names) in (2, 3)
          and "smart_route_search" in names and "smart_route_call" in names, f"st={st} {names}")
    # search_tools for the IP tool
    st2, _, obj2, _, _ = req("POST", base, {"jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": "smart_route_search", "arguments": {"query": "公网 ip"},
                   "_meta": {PV: "2026-07-28", **CLIENT_META}}})
    body = (obj2 or {}).get("result", {})
    txt = json.dumps(body, ensure_ascii=False)
    check("G2 $smart smart_route_search (2026)", st2 == 200 and "getPublicIp" in txt, f"st={st2} {txt[:120]}")
    # resolve the prefixed tool name from search results, then call
    m = re.search(r'"name"\s*:\s*"([^"]*getPublicIp)"', txt)
    target = m.group(1) if m else None
    if not target:
        skip("G2 $smart smart_route_call", "search 未命中 IP 工具"); return
    st3, _, obj3, _, _ = req("POST", base, {"jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": {"name": "smart_route_call", "arguments": {"toolName": target, "arguments": {}},
                   "_meta": {PV: "2026-07-28", **CLIENT_META}}})
    rtxt = json.dumps((obj3 or {}).get("result", {}), ensure_ascii=False)
    ip = valid_ip(rtxt)
    check("G2 $smart × 2026 call 公网IP真实调用", st3 == 200 and ip, f"st={st3} ip={ip} {rtxt[:100]}")

# ── G3: meta self-recursion regression ───────────────────────────────────────
def g3():
    # direct meta name as call target via $smart call_tool
    base = "/mcp/%24smart"
    t0 = time.time()
    st, _, obj, _, _ = req("POST", base, {"jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "call_tool", "arguments": {"toolName": "mcphub-desktop-smart_route_call", "arguments": {"toolName": "x"}},
                   "_meta": {PV: "2026-07-28", **CLIENT_META}}})
    dt = time.time() - t0
    ok = st < 500 and dt < 10
    check("G3 meta 自递归回归（$smart call_tool→meta 名 快速报错不挂死）", ok, f"st={st} dt={dt:.1f}s {str(obj)[:100]}")
    # bare meta name via REST smart path (if present)
    t1 = time.time()
    st2, obj2, raw = None, None, None
    try:
        st2, obj2, raw2, _, raw = req("POST", "/rest/smart/call",
            {"toolName": "smart_route_call", "arguments": {"toolName": "smart_route_search", "arguments": {}}})
    except Exception as e:
        raw = str(e)
    dt2 = time.time() - t1
    check("G3 REST /rest/smart/call meta 目标不挂死", dt2 < 10 and (st2 is None or st2 < 500), f"st={st2} dt={dt2:.1f}s {str(raw)[:100]}")

# ── G4: tasks/update dual-generation ────────────────────────────────────────
def g4():
    # legacy session: tasks/update must be -32601 (ext method redesigned in 2026)
    st, sid, obj, _, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "r8", "version": "1"}}})
    if not sid:
        skip("G4 legacy tasks/update", "init failed"); return
    req("POST", "/mcp", {"jsonrpc": "2.0", "method": "notifications/initialized"}, {"Mcp-Session-Id": sid})
    hdr = {"Mcp-Session-Id": sid, "MCP-Protocol-Version": "2025-11-25"}
    st2, _, obj2, _, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 2, "method": "tasks/update",
        "params": {"taskId": "nope", "inputResponses": []}}, hdr)
    code = (obj2 or {}).get("error", {}).get("code")
    check("G4 legacy tasks/update -32601 门控", st2 == 200 and code == -32601, f"st={st2} code={code}")
    # 2026 stateless: unknown id → -32602. NOTE: per spec the client MUST
    # declare the tasks capability — the SDK rejects undeclared with
    # missing-required-capability, so declare it here (rmcp 3.4.1
    # validate_tasks_capability).
    body = {"jsonrpc": "2.0", "id": 3, "method": "tasks/update",
            "params": {"taskId": "nope", "inputResponses": {},
                       "_meta": {PV: "2026-07-28",
                                 **CLIENT_META,
                                 "io.modelcontextprotocol/clientCapabilities": {
                                     "extensions": {"io.modelcontextprotocol/tasks": {}}}}}}
    st3, _, obj3, _, _ = req("POST", "/mcp", body)
    code3 = (obj3 or {}).get("error", {}).get("code")
    # rmcp maps JSON-RPC -32602 to HTTP 400 — both surfaces are valid.
    check("G4 2026 tasks/update 未知 id -32602", code3 == -32602 and st3 in (200, 400), f"st={st3} code={code3} {str(obj3)[:120]}")

# ── G5: json_response=true explicit form ────────────────────────────────────
def g5():
    st, sid, obj, ct, raw = req("POST", "/mcp", modern(1, "本机公网ip查询-getPublicIp", {}))
    ip = valid_ip(raw)
    check("G5 modern stateless CT=application/json", st == 200 and "application/json" in ct and "text/event-stream" not in ct, f"st={st} ct={ct}")
    check("G5 modern stateless 无 SSE 帧污染", raw.strip().startswith("{"), raw[:40])
    check("G5 公网IP真实调用（json 响应形态）", ip is not None, f"ip={ip}")

# ── bearer helpers ───────────────────────────────────────────────────────────
def set_bearer(on):
    con = sqlite3.connect(DB)
    cfg = json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
    cfg.setdefault("routing", {})["enableBearerAuth"] = bool(on)
    con.execute("UPDATE system_config SET config_json=?, updated_at=datetime('now') WHERE rowid=1",
                (json.dumps(cfg, ensure_ascii=False),))
    con.commit(); con.close()

def get_key():
    con = sqlite3.connect(DB)
    row = con.execute("SELECT token FROM bearer_keys WHERE enabled=1 LIMIT 1").fetchone()
    con.close(); return row[0] if row else None

def set_key_allowed(token, allowed_json):
    con = sqlite3.connect(DB)
    con.execute("UPDATE bearer_keys SET allowed_servers=? WHERE token=?", (allowed_json, token))
    con.commit(); con.close()

def g6():
    orig = json.loads(sqlite3.connect(DB).execute(
        "SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}").get("routing", {}).get("enableBearerAuth", False)
    if orig:
        skip("G6 bearer × discover", "bearer 已开启（还原风险）"); return
    try:
        set_bearer(True)
        time.sleep(0.3)
        st, _, obj, _, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 1, "method": "server/discover",
            "params": {"_meta": {PV: "2026-07-28", **CLIENT_META}}})
        check("G6 bearer on discover 无 key 401", st == 401, f"st={st}")
        token = get_key()
        if not token:
            skip("G6 bearer × discover 带key", "无 bearer key"); return
        st2, _, obj2, _, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 2, "method": "server/discover",
            "params": {"_meta": {PV: "2026-07-28", **CLIENT_META}}}, {"Authorization": f"Bearer {token}"})
        sv = (((obj2 or {}).get("result", {}) or {}).get("_meta", {}) or {}).get("io.modelcontextprotocol/serverInfo")
        check("G6 bearer on discover 带key 200 + serverInfo", st2 == 200 and sv, f"st={st2} {str(obj2)[:120]}")
    finally:
        set_bearer(False)
        time.sleep(0.3)

def g7():
    token = get_key()
    if not token:
        skip("G7 空 allowed_servers", "无 bearer key"); return
    con = sqlite3.connect(DB)
    orig_at, orig = con.execute(
        "SELECT access_type, allowed_servers FROM bearer_keys WHERE token=?", (token,)).fetchone()
    con.close()
    con0 = sqlite3.connect(DB)
    cfg0 = con0.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0]
    con0.close()
    try:
        # access_type must be 'servers' for allowed_servers to matter at all
        # ('all' ignores the list by design) — and bearer auth must be ON,
        # otherwise the key is never consulted.
        con = sqlite3.connect(DB)
        cfg = json.loads(cfg0 or "{}")
        cfg.setdefault("routing", {})["enableBearerAuth"] = True
        con.execute("UPDATE bearer_keys SET access_type='servers', allowed_servers=? WHERE token=?", ("[]", token))
        con.execute("UPDATE system_config SET config_json=?, updated_at=datetime('now') WHERE rowid=1",
                    (json.dumps(cfg, ensure_ascii=False),))
        con.commit(); con.close()
        time.sleep(0.5)
        st, sid, obj, _, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "r8", "version": "1"}}},
            {"Authorization": f"Bearer {token}"})
        if st != 200:
            check("G7 空 allow-list initialize", False, f"st={st}"); return
        req("POST", "/mcp", {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {"Mcp-Session-Id": sid, "Authorization": f"Bearer {token}"})
        hdr = {"Mcp-Session-Id": sid, "MCP-Protocol-Version": "2025-11-25", "Authorization": f"Bearer {token}"}
        st2, _, obj2, _, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": "本机公网ip查询-getPublicIp", "arguments": {}}}, hdr)
        err = (obj2 or {}).get("error", {}) or {}
        res = (obj2 or {}).get("result", {}) or {}
        # fail-closed: empty allow-list must NOT permit the call (403/404 error or isError)
        blocked = err.get("code") in (403, -32000, -32602) or res.get("isError") is True
        check("G7 allowed_servers=[] fail-closed 拒绝调用", blocked, f"st={st2} {str(obj2)[:140]}")
    finally:
        con = sqlite3.connect(DB)
        con.execute("UPDATE bearer_keys SET access_type=?, allowed_servers=? WHERE token=?", (orig_at, orig, token))
        con.execute("UPDATE system_config SET config_json=?, updated_at=datetime('now') WHERE rowid=1", (cfg0,))
        con.commit(); con.close()

def g8():
    # blank session header on a legacy bare request: lenient strips + upgrades
    st, sid, obj, ct, raw = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "本机公网ip查询-getPublicIp", "arguments": {}}},
        {"mcp-session-id": "", "MCP-Protocol-Version": "2025-11-25"})
    ip = valid_ip(raw)
    check("G8 空 session 头宽松放行 + 公网IP真实调用", st == 200 and ip is not None, f"st={st} ip={ip} {raw[:80]}")

def main():
    for f in [g5, g1, g2, g3, g4, g8, g6, g7]:
        try:
            f()
        except Exception as e:
            check(f.__name__ + " (exception)", False, repr(e))
    print(f"\n== {PASS}/{PASS+FAIL} passed, {SKIPPED} skipped ==")
    if FAILED:
        print("FAILED:", ", ".join(FAILED)); sys.exit(1)

if __name__ == "__main__":
    main()
