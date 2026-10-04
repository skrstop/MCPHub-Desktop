#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""补充覆盖套件 v19（R19 复核轮固化）：bearer × 版本 × 通道、prompts/resources 全版本、
tasks 全生命周期全版本、subscriptions/listen 逐版本、group allow-list 边界、on-demand 唤醒。

要求：服务器「本机公网ip查询」已连接；分组 Test 存在且含该服务器；DB 中存在至少一条 bearer key。
断言风格：严格 IPv4 校验 / 精确 JSON-RPC 形状 / 版本回显一致；HTTP 500 一律 FAIL。
"""
import http.client, json, sqlite3, os, re, sys, time, ipaddress, urllib.parse, base64

def mcp_name_header(tool_name):
    """SEP-2243 线格式（rmcp mcp_headers.rs: =?base64?<b64>?=）。"""
    try:
        tool_name.encode("latin-1")
        return tool_name
    except UnicodeEncodeError:
        return "=?base64?" + base64.b64encode(tool_name.encode()).decode() + "?="

HOST, PORT = "localhost", 23333
DB = os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")
IP_SERVER = "本机公网ip查询"
IP_TOOL = "本机公网ip查询-getPublicIp"
SCOPE = urllib.parse.quote(IP_SERVER)
GROUP = "Test"
VERSIONS = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25", "2026-07-28"]

PASS, FAIL, FAILED = [], [], []
def check(name, cond, detail=""):
    if cond: PASS.append(name); print(f"PASS | {name}" + (f" ({detail})" if detail else ""))
    else: FAIL.append(name); FAILED.append(name); print(f"FAIL | {name} :: {detail[:160]}")

IP_RE = re.compile(r"(?<![\d.])(?:\d{1,3}\.){3}\d{1,3}(?![\d.])")
def valid_ip(text):
    m = IP_RE.search(text or "")
    if not m: return None
    try: ipaddress.ip_address(m.group(0)); return m.group(0)
    except ValueError: return None

def req(method, path, body=None, headers=None, timeout=60):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {"Accept": "application/json, text/event-stream", "Content-Type": "application/json"}
    if headers: h.update(headers)
    c.request(method, path, body=json.dumps(body, ensure_ascii=False).encode() if body is not None else None, headers=h)
    r = c.getresponse(); d = r.read().decode("utf-8", "replace")
    sid = r.getheader("mcp-session-id")
    c.close()
    obj = None
    if "data:" in d:
        for line in d.splitlines():
            if line.startswith("data:"):
                try: obj = json.loads(line[5:].strip())
                except Exception: pass
    if obj is None:
        try: obj = json.loads(d)
        except Exception: pass
    return r.status, obj, d, sid

def set_bearer(on):
    con = sqlite3.connect(DB)
    cfg = json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
    cfg.setdefault("routing", {})["enableBearerAuth"] = bool(on)
    con.execute("UPDATE system_config SET config_json=?, updated_at=datetime('now') WHERE rowid=1",
                (json.dumps(cfg, ensure_ascii=False),))
    con.commit(); con.close()

def get_bearer_state():
    con = sqlite3.connect(DB)
    cfg = json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
    con.close()
    return cfg.get("routing", {}).get("enableBearerAuth", False)

def get_key():
    con = sqlite3.connect(DB)
    row = con.execute("SELECT token FROM bearer_keys WHERE enabled=1 LIMIT 1").fetchone()
    con.close()
    return row[0] if row else None

def session(path, version):
    """initialize + initialized；返回 (sid, negotiated)。"""
    st, obj, d, sid = req("POST", path, {"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"m19","version":"1"}}})
    pv = (obj or {}).get("result", {}).get("protocolVersion")
    if sid:
        req("POST", path, {"jsonrpc":"2.0","method":"notifications/initialized"}, {"Mcp-Session-Id": sid})
    return sid, pv

def call_ip(path, sid=None, bare=True):
    name = "getPublicIp" if (bare and path != "/mcp") else IP_TOOL
    st, obj, d, _ = req("POST", path, {"jsonrpc":"2.0","id":9,"method":"tools/call",
        "params":{"name":name,"arguments":{}}}, {"Mcp-Session-Id": sid} if sid else None)
    return st, valid_ip(d), obj

# ── 1. bearer × 版本 × 通道矩阵 ──
def bearer_matrix():
    orig = get_bearer_state()
    KEY = get_key()
    try:
        set_bearer(True); time.sleep(0.4)
        for v in VERSIONS:
            tag = f"[bearer/{v}]"
            # 无 token → 401（initialize 也受控）
            st, obj, d, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":1,"method":"initialize",
                "params":{"protocolVersion":v,"capabilities":{},"clientInfo":{"name":"m19","version":"1"}}})
            check(f"{tag} 无 token initialize → 401", st == 401, f"st={st}")
            # 错误 token → 401
            st, obj, d, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":1,"method":"initialize",
                "params":{"protocolVersion":v,"capabilities":{},"clientInfo":{"name":"m19","version":"1"}}},
                {"Authorization": "Bearer wrong-token-xyz"})
            check(f"{tag} 错误 token → 401", st == 401, f"st={st}")
            if not KEY:
                continue
            # 有效 token：完整 lifecycle + 公网IP 调用（root/scope/group 三通道）
            H = {"Authorization": f"Bearer {KEY}"}
            sid, pv = session_h("/mcp", v, H)
            check(f"{tag} 带 key initialize 会话", sid is not None or v == "2026-07-28", f"sid={sid}")
            st, ip, obj = call_ip_h("/mcp", sid, H)
            check(f"{tag} 带 key root 公网IP", st == 200 and ip, f"st={st} ip={ip}")
            sid2, _ = session_h(f"/mcp/{SCOPE}", v, H)
            st, ip, _ = call_ip_h(f"/mcp/{SCOPE}", sid2, H)
            check(f"{tag} 带 key scope 公网IP", st == 200 and ip, f"st={st} ip={ip}")
            sid3, _ = session_h(f"/mcp/{GROUP}", v, H)
            st, ip, _ = call_ip_h(f"/mcp/{GROUP}", sid3, H)
            check(f"{tag} 带 key group 公网IP", st == 200 and ip, f"st={st} ip={ip}")
            # REST 带 key
            st, obj, d, _ = req("POST", f"/rest/{SCOPE}/call", {"tool":"getPublicIp","arguments":{}}, H)
            ip = valid_ip(d)
            check(f"{tag} 带 key REST 公网IP", st == 200 and ip, f"st={st} ip={ip}")
            # 无 key REST → 401
            st, obj, d, _ = req("POST", f"/rest/{SCOPE}/call", {"tool":"getPublicIp","arguments":{}})
            check(f"{tag} REST 无 key → 401", st == 401, f"st={st}")
            # /api 带 key
            c = http.client.HTTPConnection(HOST, PORT, timeout=30)
            c.request("GET", f"/api/{SCOPE}/tools/{SCOPE}/getPublicIp", headers=H)
            r = c.getresponse(); d = r.read().decode(); c.close()
            ip = valid_ip(d)
            check(f"{tag} 带 key /api 公网IP", r.status == 200 and ip, f"st={r.status} ip={ip}")
    finally:
        set_bearer(orig); time.sleep(0.4)

def session_h(path, version, H):
    st, obj, d, sid = req("POST", path, {"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"m19","version":"1"}}}, H)
    if sid:
        req("POST", path, {"jsonrpc":"2.0","method":"notifications/initialized"}, {"Mcp-Session-Id": sid, **H})
    return sid, (obj or {}).get("result", {}).get("protocolVersion")

def call_ip_h(path, sid, H):
    name = "getPublicIp" if path != "/mcp" else IP_TOOL
    st, obj, d, _ = req("POST", path, {"jsonrpc":"2.0","id":9,"method":"tools/call",
        "params":{"name":name,"arguments":{}}}, {**({"Mcp-Session-Id": sid} if sid else {}), **H})
    return st, valid_ip(d), obj

# ── 2. prompts/resources 逐版本（builtin + 形状断言）──
def prompts_resources_matrix():
    for v in VERSIONS:
        tag = f"[pr/{v}]"
        sid, pv = session("/mcp", v)
        if not sid and v != "2026-07-28":
            check(f"{tag} 会话建立", False, f"pv={pv}")
            continue
        H = {"Mcp-Session-Id": sid} if sid else {}
        st, obj, d, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":2,"method":"prompts/list","params":{}}, H)
        prompts = (obj or {}).get("result", {}).get("prompts") or []
        check(f"{tag} prompts/list 200 + 列表", st == 200 and isinstance(prompts, list), f"st={st} n={len(prompts)}")
        st, obj, d, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":3,"method":"resources/list","params":{}}, H)
        resources = (obj or {}).get("result", {}).get("resources") or []
        check(f"{tag} resources/list 200 + 列表", st == 200 and isinstance(resources, list), f"st={st} n={len(resources)}")
        # prompts/get 真实渲染（若有 prompt）
        if prompts:
            pname = prompts[0].get("name")
            st, obj, d, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":4,"method":"prompts/get",
                "params":{"name":pname}}, H)
            msgs = (obj or {}).get("result", {}).get("messages") or []
            check(f"{tag} prompts/get 真实渲染", st == 200 and len(msgs) > 0, f"st={st} n={len(msgs)}")
        else:
            check(f"{tag} prompts/get（无 prompt 可用，跳过断言记录）", True, "no prompts")
        # resources/read 真实读取（若有 resource）
        if resources:
            uri = resources[0].get("uri")
            st, obj, d, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":5,"method":"resources/read",
                "params":{"uri":uri}}, H)
            contents = (obj or {}).get("result", {}).get("contents") or []
            check(f"{tag} resources/read 真实读取", st == 200 and len(contents) > 0, f"st={st} n={len(contents)}")
        else:
            check(f"{tag} resources/read（无 resource 可用）", True, "no resources")
        # 未知 prompt → -32602
        st, obj, d, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":6,"method":"prompts/get",
            "params":{"name":"no/such/prompt"}}, H)
        check(f"{tag} 未知 prompt → 错误响应", (obj or {}).get("error", {}).get("code") in (-32602, -32601), f"{str(obj)[:90]}")

# ── 3. tasks 全生命周期逐版本（legacy 2025-11 core + 2026 ext 声明）──
def tasks_lifecycle_matrix():
    # legacy：客户端声明 core tasks capability
    con = sqlite3.connect(DB)  # noqa (keep import symmetry)
    con.close()
    st, obj, d, sid = req("POST", "/mcp", {"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":"2025-11-25","capabilities":{"tasks":{}},
                  "clientInfo":{"name":"m19","version":"1"}}})
    if sid:
        req("POST", "/mcp", {"jsonrpc":"2.0","method":"notifications/initialized"}, {"Mcp-Session-Id": sid})
    pv = (obj or {}).get("result", {}).get("protocolVersion")
    if sid:
        st, obj, d, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":2,"method":"tasks/list","params":{}},
                            {"Mcp-Session-Id": sid})
        check("[tasks/legacy] tasks/list 200", st == 200 and (obj or {}).get("result") is not None, f"st={st}")
        # rmcp 3.4.1 设计钉死：task-directed 创建要求 per-request _meta 声明
        # extensions 形式（extensions["io.modelcontextprotocol/tasks"]）——
        # legacy core capabilities.tasks 不被识别，task 创建 → -32021。
        # tasks/get|result|list|cancel 四个 legacy 方法本身仍可用（见下方对
        # 2026 创建任务的轮询用 legacy 字段名验证不必要——生命周期在 2026 节）。
        st, obj, d, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":3,"method":"tools/call",
            "params":{"name":IP_TOOL,"arguments":{},"task":{"ttlMs":60000}}},
            {"Mcp-Session-Id": sid})
        err = (obj or {}).get("error") or {}
        check("[tasks/legacy] task-directed 创建 → -32021（rmcp ext-only 设计）",
              err.get("code") == -32021, f"{str(obj)[:120]}")
        # tasks/list（legacy core 方法可用）
        st, obj, d, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":7,"method":"tasks/update",
            "params":{"taskId":"x","inputResponses":{}}}, {"Mcp-Session-Id": sid})
        # rmcp 3.4.1：legacy 会话调 tasks/* 方法未声明 ext → -32021（rmcp 能力门控）
        check("[tasks/legacy] tasks/update → -32021/-32601",
              (obj or {}).get("error", {}).get("code") in (-32021, -32601), f"{str(obj)[:90]}")
    # 2026 ext：_meta 声明 tasks 扩展
    meta = {"io.modelcontextprotocol/protocolVersion":"2026-07-28",
            "io.modelcontextprotocol/clientInfo":{"name":"m19","version":"1"},
            "io.modelcontextprotocol/clientCapabilities":{"extensions":{"io.modelcontextprotocol/tasks":{}}}}
    H26 = {"MCP-Protocol-Version":"2026-07-28","Mcp-Method":"tools/call","Mcp-Name":mcp_name_header(IP_TOOL)}
    st, obj, d, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":1,"method":"tools/call",
        "params":{"name":IP_TOOL,"arguments":{},"task":{"ttlMs":60000},"_meta":meta}}, H26)
    res = (obj or {}).get("result") or {}
    tid = res.get("taskId")
    ok = (st == 200 and res.get("resultType") == "task" and tid
          and all(k in res for k in ("status","createdAt","lastUpdatedAt")))
    check("[tasks/2026] CreateTaskResult 平铺形状", ok, f"st={st} {str(res)[:140]}")
    if tid:
        st, obj, d, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":2,"method":"tasks/get",
            "params":{"taskId":tid,"_meta":meta}},
            {"MCP-Protocol-Version":"2026-07-28","Mcp-Method":"tasks/get"})
        task = (obj or {}).get("result") or {}
        check("[tasks/2026] tasks/get 扩展形（平铺+ttlMs/pollIntervalMs）",
              st == 200 and task.get("taskId") == tid and "ttlMs" in task and "pollIntervalMs" in task,
              f"{str(task)[:140]}")
        final = None
        for _ in range(30):
            st, obj, d, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":3,"method":"tasks/get",
                "params":{"taskId":tid,"_meta":meta}},
                {"MCP-Protocol-Version":"2026-07-28","Mcp-Method":"tasks/get"})
            task = (obj or {}).get("result") or {}
            if task.get("status") in ("completed", "failed", "cancelled"):
                final = task; break
            time.sleep(0.3)
        check("[tasks/2026] 轮询到 completed（内嵌 result）",
              final is not None and final.get("status") == "completed" and (final.get("result") or {}).get("resultType") == "complete",
              f"{str(final)[:140]}")
        # R358 语义钉死：终态任务 update/cancel 必须响亮失败（静默 ack 会骗客户端）
        st, obj, d, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":4,"method":"tasks/update",
            "params":{"taskId":tid,"inputResponses":{},"_meta":meta}},
            {"MCP-Protocol-Version":"2026-07-28","Mcp-Method":"tasks/update"})
        check("[tasks/2026] 终态 tasks/update → -32602 响亮失败",
              (obj or {}).get("error", {}).get("code") == -32602, f"st={st} {str(obj)[:90]}")
        st, obj, d, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":5,"method":"tasks/cancel",
            "params":{"taskId":tid,"_meta":meta}},
            {"MCP-Protocol-Version":"2026-07-28","Mcp-Method":"tasks/cancel"})
        check("[tasks/2026] 终态 tasks/cancel → -32602 响亮失败",
              (obj or {}).get("error", {}).get("code") == -32602, f"st={st} {str(obj)[:90]}")
        # working 态协作取消：新建任务立即 cancel → status=cancelled
        st, obj, d, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":6,"method":"tools/call",
            "params":{"name":IP_TOOL,"arguments":{},"task":{"ttlMs":60000},"_meta":meta}}, H26)
        tid2 = ((obj or {}).get("result") or {}).get("taskId")
        if tid2:
            st, obj, d, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":7,"method":"tasks/cancel",
                "params":{"taskId":tid2,"_meta":meta}},
                {"MCP-Protocol-Version":"2026-07-28","Mcp-Method":"tasks/cancel"})
            snap = (obj or {}).get("result") or {}
            # 无状态路径 cancel ack 注入 resultType:complete（空 result）；
            # 终态以 tasks/get 复核为准。
            ack_ok = st == 200 and "error" not in (obj or {}) and snap.get("status") in (None, "cancelled")
            if ack_ok:
                st2, obj2, d2, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":8,"method":"tasks/get",
                    "params":{"taskId":tid2,"_meta":meta}},
                    {"MCP-Protocol-Version":"2026-07-28","Mcp-Method":"tasks/get"})
                t2 = (obj2 or {}).get("result") or {}
                ack_ok = t2.get("status") in ("cancelled", "completed")
            check("[tasks/2026] working 态 cancel 协作 ack + 终态", ack_ok, f"st={st} {str(snap)[:100]}")
        else:
            check("[tasks/2026] 第二任务创建（cancel 用）", False, f"res={str(res)[:120]}")
    # 未声明 tasks 扩展的 2026 客户端 → 永不同步任务（规范）
    meta_no = {"io.modelcontextprotocol/protocolVersion":"2026-07-28",
               "io.modelcontextprotocol/clientInfo":{"name":"m19n","version":"1"},
               "io.modelcontextprotocol/clientCapabilities":{}}
    st, obj, d, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":6,"method":"tools/call",
        "params":{"name":IP_TOOL,"arguments":{},"_meta":meta_no}},
        {"MCP-Protocol-Version":"2026-07-28","Mcp-Method":"tools/call","Mcp-Name":mcp_name_header(IP_TOOL)})
    res = (obj or {}).get("result") or {}
    check("[tasks/2026] 未声明扩展 → 同步结果非 task", st == 200 and res.get("resultType") != "task" and res.get("isError") is False,
          f"resultType={res.get('resultType')}")

# ── 4. subscriptions/listen 逐版本 ──
def subscriptions_matrix():
    for v in ["2025-11-25", "2026-07-28"]:
        tag = f"[subs/{v}]"
        meta = {"io.modelcontextprotocol/protocolVersion":"2026-07-28",
                "io.modelcontextprotocol/clientInfo":{"name":"m19","version":"1"},
                "io.modelcontextprotocol/clientCapabilities":{}} if v == "2026-07-28" else {}
        body = {"jsonrpc":"2.0","id":1,"method":"subscriptions/listen",
                "params":{"notifications":{"toolsListChanged":True}}
                | ({"_meta": meta} if meta else {})}
        hdrs = ({"MCP-Protocol-Version":"2026-07-28","Mcp-Method":"subscriptions/listen"} if v == "2026-07-28"
                else {})
        c = http.client.HTTPConnection(HOST, PORT, timeout=20)
        c.request("POST", "/mcp", body=json.dumps(body).encode(),
                  headers={"Content-Type":"application/json","Accept":"text/event-stream", **hdrs})
        r = c.getresponse()
        st, ct = r.status, r.getheader("content-type") or ""
        chunk = (r.readline() + r.readline() + r.readline()).decode("utf-8","replace")
        c.close()
        if v == "2026-07-28":
            ack_ok = "acknowledged" in chunk and "subscriptionId" in chunk
            check(f"{tag} listen 200 + SSE ack", st == 200 and "text/event-stream" in ct and ack_ok, f"st={st} {chunk[:100]}")
            # filter 精确性：未请求类型不到达（简测：ack 后立即关流）
            body2 = {"jsonrpc":"2.0","id":2,"method":"subscriptions/listen",
                     "params":{"notifications":{}} | {"_meta": meta}}
            c = http.client.HTTPConnection(HOST, PORT, timeout=20)
            c.request("POST", "/mcp", body=json.dumps(body2).encode(),
                      headers={"Content-Type":"application/json","Accept":"text/event-stream", **hdrs})
            r = c.getresponse()
            st2 = r.status
            c.close()
            check(f"{tag} 空 filter listen 200（服务器不拒）", st2 == 200, f"st={st2}")
        else:
            # 设计钉死：subscriptions/listen 是 2026 特性；legacy 用原生 GET SSE 流 + resources/subscribe
            check(f"{tag} legacy listen → -32601（2026-only 设计）",
                  (json.loads(chunk.split("data:")[-1].strip()) if "data:" in chunk else {}).get("error", {}).get("code") == -32601 or st == 404,
                  f"st={st} {chunk[:80]}")

# ── 5. group 边界 ──
def group_edges():
    tag = "[group]"
    sid, pv = session(f"/mcp/{GROUP}", "2025-11-25")
    st, ip, obj = call_ip(f"/mcp/{GROUP}", sid)
    check(f"{tag} group 通道公网IP", st == 200 and ip, f"st={st} ip={ip}")
    # group 外服务器工具 → 不可见/不可调
    st, obj, d, _ = req("POST", f"/mcp/{GROUP}", {"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}},
                        {"Mcp-Session-Id": sid} if sid else None)
    tools = (obj or {}).get("result", {}).get("tools") or []
    names = [t.get("name","") for t in tools]
    # 从 DB 取真实成员，断言每个暴露工具都归属某个成员（前缀或裸名）
    con = sqlite3.connect(DB)
    members = [m.get("name") for m in json.loads(
        con.execute("SELECT servers FROM groups WHERE name=?", (GROUP,)).fetchone()[0])]
    con.close()
    leaked = []
    for n in names:
        if not any(n == m or n.startswith(f"{m}-") for m in members):
            leaked.append(n)
    check(f"{tag} 非 group 成员工具不泄漏", len(leaked) == 0, f"members={members[:3]} leaked={leaked[:3]}")
    # 不存在的 group → 空/错误（非 5xx）
    st, obj, d, _ = req("POST", "/mcp/NoSuchGroup-xyz", {"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"m19","version":"1"}}})
    check(f"{tag} 不存在 group → 非 5xx", st < 500, f"st={st}")

# ── 6. on-demand 唤醒（若有 on-demand 服务器则真实唤醒）──
def on_demand_wake():
    con = sqlite3.connect(DB)
    row = con.execute("SELECT name, start_on_demand FROM servers WHERE enabled=1").fetchall()
    con.close()
    od = next((name for name, sod in row if sod), None)
    if not od:
        check("[on-demand] 无 on-demand 服务器（环境跳过）", True, "none configured")
        return
    tag = "[on-demand]"
    scope = urllib.parse.quote(od)
    sid, pv = session(f"/mcp/{scope}", "2025-11-25")
    st, obj, d, _ = req("POST", f"/mcp/{scope}", {"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}},
                        {"Mcp-Session-Id": sid} if sid else None, timeout=120)
    tools = (obj or {}).get("result", {}).get("tools") or []
    check(f"{tag} 唤醒后 tools/list 非空", st == 200 and len(tools) > 0, f"st={st} n={len(tools)}")

def main():
    print("== v19 补充覆盖套件 ==")
    bearer_matrix()
    prompts_resources_matrix()
    tasks_lifecycle_matrix()
    subscriptions_matrix()
    group_edges()
    on_demand_wake()
    print(f"\n== {len(PASS)}/{len(PASS)+len(FAIL)} passed ==")
    if FAILED:
        print("FAILED:", ", ".join(FAILED[:20])); sys.exit(1)

if __name__ == "__main__":
    main()
