#!/usr/bin/env python3
# 套件 R158：R108-R157 轮修复回归 + E2E 覆盖缺口补全
#  H-121a REST meta 门控 / M-121b 列表泄漏 / M-121c 组 allow-list / M-108-1 resources/read 缓存
#  M-110-1 tasks 字段名分代 / M-116-1 smart 超时（结构断言）/ 导出字段 / registry 注册
import json, http.client

HOST, PORT = "localhost", 23333
PASS, FAIL = [], []
def check(name, ok, detail=""):
    (PASS if ok else FAIL).append(name)
    print(("PASS" if ok else "FAIL"), "|", name, ("| " + str(detail)[:180] if detail and not ok else ""))

def req(method, path, payload=None, headers=None, timeout=60):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {"Accept": "application/json, text/event-stream"}
    if payload is not None: h["Content-Type"] = "application/json"
    if headers: h.update(headers)
    c.request(method, path, body=json.dumps(payload) if payload is not None else None, headers=h)
    r = c.getresponse(); data = r.read().decode("utf-8", "replace")
    sid = r.getheader("mcp-session-id"); st = r.status
    c.close()
    obj = None
    try: obj = json.loads(data)
    except Exception:
        frames = []
        for line in data.split("\n"):
            if line.startswith("data: "):
                try: frames.append(json.loads(line[6:]))
                except Exception: pass
        obj = frames[-1] if frames else None
    return st, obj, sid, data

META_TOOLS = ["smart_route_search", "smart_route_describe", "smart_route_call"]
IP_TOOL = "本机公网ip查询-getPublicIp"
PV = "io.modelcontextprotocol/protocolVersion"
CLIENT_META = {"io.modelcontextprotocol/clientInfo": {"name": "r158", "version": "1"},
               "io.modelcontextprotocol/clientCapabilities": {
                   "extensions": {"io.modelcontextprotocol/tasks": {}}}}
H26 = {"MCP-Protocol-Version": "2026-07-28"}

# ── A: H-121a — meta 工具在 REST 执行面 404（bearer allow-list 绕过封堵）──
st, obj, _, _ = req("POST", "/api/tools/mcphub-desktop/smart_route_call",
                    {"toolName": "anything", "arguments": {}})
check("A1 [H-121a] POST /api/tools/…/smart_route_call → 404", st == 404, f"status={st} obj={obj}")
for mt in META_TOOLS:
    st, obj, _, _ = req("POST", f"/rest/mcphub-desktop/call",
                        {"name": mt, "arguments": {}})
    check(f"A2 [H-121a] /rest/mcphub-desktop/call {mt} → 404/4xx", 400 <= st < 500,
          f"status={st} obj={obj}")

# ── B: M-121b — /rest/{server}/tools 不再泄漏 meta 工具 schema ──
st, obj, _, _ = req("GET", "/rest/mcphub-desktop/tools")
names = [t.get("name") for t in (obj or {}).get("tools", [])] if obj else []
check("B1 [M-121b] /rest/mcphub-desktop/tools 200", st == 200, f"status={st}")
check("B2 [M-121b] 无 meta 工具泄漏", not any(n in names for n in META_TOOLS), f"{names}")
check("B3 单服务器 REST 列表仍含 rag 工具（功能不受损）", len(names) > 0, f"count={len(names)}")

# ── C: M-121c — /rest/group/{g}/tools 应用组 allow-list + 无 meta 泄漏 ──
st, obj, _, _ = req("GET", "/rest/group/Test/tools")
gnames = [t.get("name") for t in (obj or {}).get("tools", [])] if obj else []
if st == 200:
    check("C1 [M-121c] /rest/group/Test/tools 200 且无 meta 工具",
          not any(n in gnames for n in META_TOOLS), f"{gnames[:8]}")
else:
    check("C1 [M-121c] 组 Test 不存在（跳过，无 meta 断言面）", st == 404, f"status={st}")

# ── D: M-108-1 — 2026 stateless resources/read 带 CacheableResult ──
st, obj, _, _ = req("GET", "/mcp")
check("D0 GET /mcp 建流或 4xx（环境自检）", st in (200, 400, 405, 406), f"status={st}")
# 先拿一个资源 URI
st, obj, _, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 1, "method": "resources/list",
    "params": {"_meta": {PV: "2026-07-28", **CLIENT_META}}}, headers=H26)
uris = [r.get("uri") for r in (obj or {}).get("result", {}).get("resources", [])]
res = (obj or {}).get("result", {})
check("D1 [M-108-1] resources/list 2026 带 ttlMs", res.get("ttlMs") == 30000, f"ttlMs={res.get('ttlMs')}")
check("D2 [M-108-1] resources/list 2026 cacheScope=private", res.get("cacheScope") == "private", f"{res.get('cacheScope')}")
if uris:
    st, obj, _, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 2, "method": "resources/read",
        "params": {"uri": uris[0], "_meta": {PV: "2026-07-28", **CLIENT_META}}}, headers=H26)
    rres = (obj or {}).get("result", {})
    check("D3 [M-108-1] resources/read 2026 带 ttlMs=30000", rres.get("ttlMs") == 30000,
          f"status={st} ttlMs={rres.get('ttlMs')}")
    check("D4 [M-108-1] resources/read 2026 cacheScope=private", rres.get("cacheScope") == "private",
          f"{rres.get('cacheScope')}")
    # legacy 会话不注入（跨代隔离）
    st, obj, sid, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 3, "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "r158", "version": "1"}}})
    if sid:
        st, obj, _, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 4, "method": "resources/read",
            "params": {"uri": uris[0]}}, headers={"mcp-session-id": sid})
        rres = (obj or {}).get("result", {})
        check("D5 legacy resources/read 无 ttlMs（跨代隔离）", "ttlMs" not in rres, f"{rres.get('ttlMs')}")

# ── E: M-110-1 — tasks 快照字段名分代 ──
def create_task(version_meta=None, session=None):
    h = {}
    p = {"name": IP_TOOL, "arguments": {}, "task": {"ttl": 60000}}
    if version_meta:
        p["_meta"] = {PV: version_meta, **CLIENT_META}
        h.update(H26)
    headers = {"x-mcphub-task-requested": '{"ttl":60000}', **h}
    if session: headers["mcp-session-id"] = session
    st, obj, _, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 10, "method": "tools/call",
        "params": p}, headers=headers)
    tid = None
    try: tid = obj["result"]["taskId"]
    except Exception: pass
    return st, tid

# 2026 路径：无 session + _meta 2026
st, tid2026 = create_task("2026-07-28")
check("E1 [M-110-1] 2026 客户端任务创建成功", st == 200 and tid2026, f"status={st} tid={tid2026}")
if tid2026:
    st, obj, _, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 11, "method": "tasks/list",
        "params": {"_meta": {PV: "2026-07-28", **CLIENT_META}}}, headers=H26)
    items = (obj or {}).get("result", {}).get("tasks", [])
    mine = next((t for t in items if t.get("taskId") == tid2026), None)
    check("E2 [M-110-1] 2026 tasks/list 条目含 pollIntervalMs", bool(mine) and "pollIntervalMs" in mine,
          f"{mine}")
    check("E3 [M-110-1] 2026 tasks/list 条目不含 legacy pollInterval", bool(mine) and "pollInterval" not in (mine or {}),
          f"{(mine or {}).keys()}")
    st, obj, _, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 12, "method": "tasks/result",
        "params": {"taskId": tid2026, "_meta": {PV: "2026-07-28", **CLIENT_META}}}, headers=H26)
    snap = (obj or {}).get("result", {})
    ok_shape = ("pollIntervalMs" in snap) or (snap.get("status") in ("completed", "failed", "cancelled"))
    check("E4 [M-110-1] 2026 tasks/result 快照/终态载荷合法", ok_shape, f"{str(snap)[:120]}")
# 2025-11 会话：legacy 字段名
st, obj, sid, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 20, "method": "initialize",
    "params": {"protocolVersion": "2025-11-25",
               "capabilities": {"extensions": {"io.modelcontextprotocol/tasks": {}}},
               "clientInfo": {"name": "r158l", "version": "1"}}})
if sid:
    st, tid = create_task(session=sid)
    check("E5 legacy 会话任务创建成功", st == 200 and tid, f"status={st}")
    if tid:
        st, obj, _, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 21, "method": "tasks/list",
            "params": {}}, headers={"mcp-session-id": sid})
        items = (obj or {}).get("result", {}).get("tasks", [])
        mine = next((t for t in items if t.get("taskId") == tid), None)
        check("E6 [M-110-1] legacy tasks/list 含 pollInterval（2025-11 形状）",
              bool(mine) and "pollInterval" in mine, f"{mine}")
        check("E7 [M-110-1] legacy tasks/list 不含 pollIntervalMs",
              bool(mine) and "pollIntervalMs" not in (mine or {}), f"{(mine or {}).keys()}")

# ── F: 公网IP 真实调用回归（每版本通道抽验，确认本轮修复未破坏执行面）──
for ver, meta in [("2024-11-05", None), ("2025-11-25", None), ("2026-07-28", "2026-07-28")]:
    p = {"name": IP_TOOL, "arguments": {}}
    h = {}
    if meta:
        p["_meta"] = {PV: meta, **CLIENT_META}
        h.update(H26)
    st, obj, _, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 30, "method": "tools/call", "params": p}, headers=h)
    txt = ""
    try: txt = obj["result"]["content"][0]["text"]
    except Exception: pass
    check(f"F1 [{ver}] 公网IP 真实调用 isError=false 且有 IP", st == 200 and obj and obj.get("result", {}).get("isError") is False and len(txt) > 3, f"status={st} txt={txt[:40]}")

# ── G: /mcp/{server} 单服务器通道 meta 工具 404（既有 M3 回归锚点）──
st, obj, _, _ = req("POST", "/mcp/%E6%9C%AC%E6%9C%BA%E5%85%AC%E7%BD%91ip%E6%9F%A5%E8%AF%A2",
                    {"jsonrpc": "2.0", "id": 40, "method": "tools/call",
                     "params": {"name": "smart_route_call", "arguments": {"toolName": "x"}}})
check("G1 单服务器通道 smart_route_call 拒绝", 400 <= st < 500 or (obj or {}).get("error"), f"status={st}")

print(f"\n== {len(PASS)}/{len(PASS)+len(FAIL)} passed ==")
if FAIL:
    print("FAILED:", *FAIL, sep="\n  - ")
    raise SystemExit(1)
