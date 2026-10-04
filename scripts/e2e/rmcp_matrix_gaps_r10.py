#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""第 10 轮 E2E（C9）：B10 审计 P1 矩阵空格补齐。

A  /api/tools/{server}/{tool} GET+POST 真实调用 × 5 版本语义探测
   （实测确认 /api 无版本头语义 → 转为「每版本 × /mcp JSON-RPC + /api 通道各一次」矩阵）
B  /mcp/{group}（Test）× 5 版本真实公网 IP 调用（REST 无版本语义，以 group MCP 通道补齐矩阵）
C  2026-07-28 × /mcp/{group} 无状态通道真实 IP 调用（r458 G1 遗留空格，含无 session 断言）
D  tasks 生命周期 × 中文 scope 通道 /mcp/{server}（URL 编码）全链路

依赖：服务器运行于 :23333，「本机公网ip查询」(openapi) 已连接，分组 Test 存在。
"""
import http.client, json, re, time, urllib.parse, ipaddress

HOST, PORT = "localhost", 23333
IP_SERVER = "本机公网ip查询"
SCOPE = urllib.parse.quote(IP_SERVER)
GROUP_PATH = "/mcp/Test"
SCOPE_PATH = f"/mcp/{SCOPE}"
PV = "io.modelcontextprotocol/protocolVersion"
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
        ipaddress.ip_address(m.group(0)); return m.group(0)
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

def modern(body_id, name, arguments, cap_ext=None):
    caps = {}
    if cap_ext: caps = {"extensions": cap_ext}
    return {"jsonrpc": "2.0", "id": body_id, "method": "tools/call",
            "params": {"name": name, "arguments": arguments,
                       "_meta": {PV: "2026-07-28",
                                 "io.modelcontextprotocol/clientInfo": {"name": "r10", "version": "1"},
                                 "io.modelcontextprotocol/clientCapabilities": caps}}}

VERSIONS = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25", "2026-07-28"]

# ── A: /api/tools/{server}/{tool} GET+POST × 版本语义 ─────────────────────────
def a_part():
    api = f"/api/tools/{SCOPE}/getPublicIp"
    st, _, obj, _, _ = req("GET", api)
    txt = "".join(str(c.get("text", "")) for c in (obj or {}).get("content", []))
    check("A1 /api GET tools/{server}/{tool} 200 + isError=false",
          st == 200 and (obj or {}).get("isError") is False, f"st={st} isError={(obj or {}).get('isError')}")
    check("A2 /api GET 返回真实公网 IPv4", bool(valid_ip(txt)), f"txt={txt[:60]}")
    st2, _, obj2, _, _ = req("POST", api, {})
    txt2 = "".join(str(c.get("text", "")) for c in (obj2 or {}).get("content", []))
    check("A3 /api POST tools/{server}/{tool} 200 + isError=false",
          st2 == 200 and (obj2 or {}).get("isError") is False, f"st={st2}")
    check("A4 /api POST 返回真实公网 IPv4", bool(valid_ip(txt2)), f"txt={txt2[:60]}")
    # 版本头语义探测：/api 是否区分 MCP-Protocol-Version
    st3, _, _, _, _ = req("GET", api, headers={"MCP-Protocol-Version": "9999-01-01"})
    st4, _, obj4, _, _ = req("GET", api, headers={"MCP-Protocol-Version": "2024-11-05"})
    check("A5 /api 版本无关（非法版本头不拒 / 正常版本不影响）",
          st3 == 200 and st4 == 200, f"st(9999)={st3} st(2024)={st4}")
    if st3 == 200 and st4 == 200:
        print("结论 | /api REST 端点无 MCP 版本语义（openApiController 镜像，无版本头消费）")
    # /api 无版本语义 → 每版本 × /mcp JSON-RPC 各一次真实 IP 调用补齐矩阵
    for i, v in enumerate(VERSIONS):
        if v == "2026-07-28":
            payload = modern(10 + i, "getPublicIp", {})
        else:
            payload = {"jsonrpc": "2.0", "id": 10 + i, "method": "tools/call",
                       "params": {"name": "getPublicIp", "arguments": {}}}
        headers = {"MCP-Protocol-Version": v}
        # root 通道 tools/call 须带 SEP-2243 Mcp-Name 头（scope 通道可从路径推导）
        headers["Mcp-Method"] = "tools/call"; headers["Mcp-Name"] = "getPublicIp"
        st5, _, obj5, _, _ = req("POST", "/mcp", payload, headers)
        r = (obj5 or {}).get("result") or {}
        txt5 = "".join(str(c.get("text", "")) for c in r.get("content", []))
        ip = valid_ip(txt5)
        extra = r.get("resultType") == "complete" if v == "2026-07-28" else True
        check(f"A6 /mcp × {v} 公网IP真实调用", st5 == 200 and ip and r.get("isError") is False and extra,
              f"st={st5} ip={ip} rt={r.get('resultType')}")

# ── B: /mcp/{group} × 5 版本真实 IP 调用 ─────────────────────────────────────
def group_toolname():
    st, _, obj, _, _ = req("POST", GROUP_PATH, {"jsonrpc": "2.0", "id": 1, "method": "tools/list",
        "params": {}})
    tools = ((obj or {}).get("result") or {}).get("tools", [])
    return next((t["name"] for t in tools if t["name"].endswith("getPublicIp")), None)

def b_part():
    gname = group_toolname()
    if not gname:
        skip("B 组通道矩阵", "group Test 无 IP 工具"); return
    for i, v in enumerate(VERSIONS):
        if v == "2026-07-28":
            payload = modern(20 + i, gname, {})
        else:
            payload = {"jsonrpc": "2.0", "id": 20 + i, "method": "tools/call",
                       "params": {"name": gname, "arguments": {}}}
        headers = {"MCP-Protocol-Version": v}
        st, sid, obj, _, _ = req("POST", GROUP_PATH, payload, headers)
        r = (obj or {}).get("result") or {}
        txt = "".join(str(c.get("text", "")) for c in r.get("content", []))
        ip = valid_ip(txt)
        check(f"B /mcp/Test × {v} 公网IP真实调用", st == 200 and ip and r.get("isError") is False,
              f"st={st} ip={ip} sid={'y' if sid else 'n'}")

# ── C: 2026-07-28 × /mcp/{group} 无状态真实 IP ────────────────────────────────
def c_part():
    gname = group_toolname()
    if not gname:
        skip("C 2026 group 无状态", "group Test 无 IP 工具"); return
    st, sid, obj, ct, raw = req("POST", GROUP_PATH, modern(30, gname, {}),
                                {"MCP-Protocol-Version": "2026-07-28"})
    r = (obj or {}).get("result") or {}
    txt = "".join(str(c.get("text", "")) for c in r.get("content", []))
    ip = valid_ip(txt)
    check("C1 2026 × /mcp/Test 无状态 tools/call 200", st == 200 and "error" not in (obj or {}),
          f"st={st} {str(obj)[:80]}")
    check("C2 resultType=complete", r.get("resultType") == "complete", f"rt={r.get('resultType')}")
    check("C3 返回真实公网 IPv4 + isError=false", ip and r.get("isError") is False,
          f"ip={ip} txt={txt[:40]}")
    check("C4 响应无 mcp-session-id（无状态）", sid is None, f"sid={sid}")

# ── D: tasks 生命周期 × 中文 scope 通道 ───────────────────────────────────────
def d_part():
    st, _, obj, _, _ = req("POST", SCOPE_PATH, {"jsonrpc": "2.0", "id": 1, "method": "tools/list",
        "params": {}})
    tools = ((obj or {}).get("result") or {}).get("tools", [])
    check("D1 /mcp/{中文服务器} tools/list 200", st == 200 and tools, f"st={st} n={len(tools)}")
    if not tools:
        skip("D tasks 全链路", "scope tools/list 失败"); return
    meta = {PV: "2026-07-28",
            "io.modelcontextprotocol/clientInfo": {"name": "r10d", "version": "1"},
            "io.modelcontextprotocol/clientCapabilities":
                {"extensions": {"io.modelcontextprotocol/tasks": {}}}}
    H26 = {"MCP-Protocol-Version": "2026-07-28", "Mcp-Method": "tools/call", "Mcp-Name": "getPublicIp"}
    st2, _, obj2, _, _ = req("POST", SCOPE_PATH, {"jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": "getPublicIp", "arguments": {}, "task": {"ttlMs": 60000}, "_meta": meta}}, H26)
    res = (obj2 or {}).get("result") or {}
    tid = res.get("taskId")
    ok = (st2 == 200 and res.get("resultType") == "task" and tid
          and all(k in res for k in ("status", "createdAt"))
          and ("pollIntervalMs" in res or "ttlMs" in res))  # pollIntervalMs 规范可选；本实现回 ttlMs
    check("D2 CreateTaskResult 平铺形状（taskId/status/createdAt/调度字段）",
          ok, f"st={st2} {str(res)[:140]}")
    if not tid:
        skip("D3/D4 轮询", "task 创建失败"); return
    HG = {"MCP-Protocol-Version": "2026-07-28", "Mcp-Method": "tasks/get"}
    final = None
    for _ in range(40):
        st3, _, obj3, _, _ = req("POST", SCOPE_PATH,
            {"jsonrpc": "2.0", "id": 3, "method": "tasks/get", "params": {"taskId": tid, "_meta": meta}}, HG)
        task = (obj3 or {}).get("result") or {}
        if task.get("status") in ("completed", "failed", "cancelled"):
            final = task; break
        time.sleep(0.3)
    check("D3 tasks/get 轮询到终态", final is not None and final.get("status") == "completed",
          f"{str(final)[:140]}")
    rt = ((final or {}).get("result") or {})
    txt = "".join(str(c.get("text", "")) for c in rt.get("content", []))
    ip = valid_ip(txt)
    check("D4 终态内嵌 result 含真实公网 IP（resultType=complete）",
          rt.get("resultType") == "complete" and ip, f"ip={ip} rt={rt.get('resultType')}")

if __name__ == "__main__":
    a_part(); b_part(); c_part(); d_part()
    print(f"\n==== rmcp_matrix_gaps_r10: PASS={PASS} FAIL={FAIL} SKIP={SKIPPED} ====")
    if FAILED:
        print("FAILED:", *FAILED, sep="\n  - ")
    import sys; sys.exit(1 if FAIL else 0)
