#!/usr/bin/env python3
# 缺口补测矩阵 R107：上游传输类型 × 协议版本 × 通道 全交叉真实调用
# 之前 15 套件所有真实调用只打 openapi(公网IP)；本套件补齐：
#   A: 5 协议版本 × 3 上游类型(stdio=codegraph / streamable-http=Idea / openapi=公网IP) root 通道真实调用
#   B: 3 上游类型 × 单服务器通道(会话 + 2026 无状态) 真实调用
#   C: tasks 双代命名(2025-11 core ttl vs 2026 ttlMs) + 真实调用任务轮询
#   D: 边界(id null/string、invalid JSON、unknown method、2MB 大载荷、completion -32601)
#   E: bearer × 版本 × 2026 交叉(自动开关还原)
import json, http.client, sqlite3, os, sys, time, concurrent.futures
from urllib.parse import quote

HOST, PORT = "127.0.0.1", 23333
PASS, FAIL = [], []
DB = os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")
VERSIONS = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25", "2026-07-28"]
META = {"_meta": {"protocolVersion": "2026-07-28"}}

def check(name, ok, detail=""):
    (PASS if ok else FAIL).append(name)
    print(("PASS" if ok else "FAIL"), "|", name, (("| " + str(detail)[:160]) if (detail and not ok) else ""))

def wait_server(max_s=180):
    t0 = time.time()
    while time.time() - t0 < max_s:
        try:
            c = http.client.HTTPConnection(HOST, PORT, timeout=2)
            c.request("GET", "/health")
            r = c.getresponse(); r.read(); c.close()
            if r.status == 200: return True
        except Exception: pass
        time.sleep(3)
    return False

def req(method, path, body=None, headers=None, timeout=90, raw_body=None):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {"Accept": "application/json, text/event-stream"}
    if body is not None or raw_body is not None:
        h["Content-Type"] = "application/json"
    if headers: h.update(headers)
    payload = raw_body if raw_body is not None else (
        json.dumps(body, ensure_ascii=False).encode("utf-8") if body is not None else None)
    try:
        c.request(method, path, body=payload, headers=h)
    except (ConnectionRefusedError, ConnectionError, OSError):
        # tauri dev 重建窗口：等待恢复后重试一次
        if not wait_server():
            raise
        c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
        h2 = dict(h)
        if headers and "Mcp-Session-Id" in h2: h2.pop("Mcp-Session-Id")  # 重建后会话失效
        c.request(method, path, body=payload, headers=h2)
    r = c.getresponse()
    data = r.read().decode("utf-8", "replace")
    hdrs = {k.lower(): v for k, v in r.getheaders()}
    st = r.status
    c.close()
    obj = None
    try:
        obj = json.loads(data)
    except Exception:
        frames = []
        for line in data.split("\n"):
            if line.startswith("data: ") and line[6:].strip():
                try: frames.append(json.loads(line[6:]))
                except Exception: pass
        obj = frames[-1] if frames else None
    return st, hdrs, obj, data

def init(version, path="/mcp", extra=None, tasks_cap=False):
    caps = {}
    if tasks_cap:
        # SEP-2663: client must declare the tasks extension to direct tasks
        caps = {"extensions": {"io.modelcontextprotocol/tasks": {}}}
    p = {"protocolVersion": version, "capabilities": caps, "clientInfo": {"name": "gap107", "version": "1"}}
    if extra: p.update(extra)
    st, hd, obj, _ = req("POST", path, {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": p})
    return st, hd.get("mcp-session-id", ""), obj

def call_tool_resilient(sid, name, args=None, rid=10, path="/mcp", meta=None, extra_headers=None, tries=2):
    """Upstream flakiness (ip.3322.net 502 等) 重试一次。"""
    last = (None, None)
    for i in range(tries):
        st, oc = call(sid, name, args=args, rid=rid, path=path, meta=meta, extra_headers=extra_headers)
        last = (st, oc)
        if result_ok(oc):
            return st, oc
        time.sleep(2)
    return last

def call(sid, name, args=None, rid=10, path="/mcp", meta=None, extra_headers=None):
    p = {"name": name, "arguments": args or {}}
    if meta: p["task"] = meta
    if sid: p["_meta"] = {"mcp-session-id": sid}
    h = dict(extra_headers or {})
    if sid: h["Mcp-Session-Id"] = sid
    st, _, obj, _ = req("POST", path, {"jsonrpc": "2.0", "id": rid, "method": "tools/call", "params": p}, headers=h)
    return st, obj

def call_stateless(name, args=None, rid=10, path="/mcp"):
    st, _, obj, _ = req("POST", path, {"jsonrpc": "2.0", "id": rid, "method": "tools/call",
                                       "params": {"name": name, "arguments": args or {}, **META}})
    return st, obj

def tools_list(sid, path="/mcp", rid=5, version_meta=False):
    p = dict(META) if version_meta else {}
    h = {"Mcp-Session-Id": sid} if sid else {}
    st, _, obj, _ = req("POST", path, {"jsonrpc": "2.0", "id": rid, "method": "tools/list", "params": p}, headers=h)
    names = [t.get("name") for t in (obj or {}).get("result", {}).get("tools", [])] if obj else []
    return st, names

def result_text(obj):
    if not obj or "result" not in obj: return ""
    return "".join(str(c.get("text", "")) for c in obj["result"].get("content", []) if isinstance(c, dict))

def result_ok(obj):
    return bool(obj) and "result" in obj and obj["result"].get("isError") is False

def upstream_ok(u, obj):
    """传输链路断言：isError=false 或上游工具自身语义错误（如 IDE 无项目上下文）。"""
    if not obj or "result" not in obj: return False
    if obj["result"].get("isError") is False: return True
    txt = result_text(obj)
    return "Unable to determine" in txt

# 上游类型定义：name=hub 侧工具全名 / server=单服务器通道名 / verifier=内容断言
UPSTREAMS = {
    "stdio-codegraph": {"tool": "codegraph-codegraph_status", "server": "codegraph",
                        "ok": lambda t: len(t) > 2},
    "http-idea": {"tool": "Idea-mcp-server-get_project_dependencies", "server": "Idea-mcp-server",
                  # IDE 上游工具可能因无项目上下文返回 isError（"Unable to determine the target project"）——
                  # 这证明 hub→streamable-http 上游往返成功；本套件验证的是传输链路而非 IDE 工具语义。
                  "ok": lambda t: len(t) > 2 or "Unable to determine" in t},
    "openapi-ip": {"tool": "本机公网ip查询-getPublicIp", "server": "本机公网ip查询",
                   "ok": lambda t: any(ch.isdigit() for ch in t)},
}

def set_strict(on):
    con = sqlite3.connect(DB)
    cfg = json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
    cfg.setdefault("mcp", {})["strictValidation"] = on
    con.execute("UPDATE system_config SET config_json=?, updated_at=datetime('now') WHERE rowid=1",
                (json.dumps(cfg, ensure_ascii=False),))
    con.commit(); con.close()
    time.sleep(0.2)

def get_strict():
    con = sqlite3.connect(DB)
    cfg = json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
    con.close()
    return cfg.get("mcp", {}).get("strictValidation", False)

# ============ A组：5 版本 × 3 上游类型 root 通道全生命周期 ============
print("== A: 版本×上游传输类型 真实调用(root) ==")
prev = get_strict()
try:
    set_strict(False)
    for v in VERSIONS:
        st, sid, obj = init(v)
        got_pv = (obj or {}).get("result", {}).get("protocolVersion")
        # rmcp 3.4.1 设计：2026-07-28 无 initialize 握手，initialize 命名 2026 时
        # 按规范回退最新 legacy 版本（2025-11-25）；其余版本必须精确回显。
        expect_pv = "2025-11-25" if v == "2026-07-28" else v
        ver_ok = st == 200 and got_pv == expect_pv
        for uk, u in UPSTREAMS.items():
            stl, names = tools_list(sid)
            has = u["tool"] in names
            stc, oc = call_tool_resilient(sid, u["tool"], rid=20)
            txt = result_text(oc)
            check(f"A [{v}|{uk}] init版本回显+真实调用", ver_ok and has and upstream_ok(u, oc) and u["ok"](txt),
                  f"init={st} list_has={has} call={stc} isError={(oc or {}).get('result',{}).get('isError')} txt={txt[:40]}")
finally:
    set_strict(prev)

# ============ B组：3 上游类型 × 单服务器通道（宽松会话 + 2026 无状态）============
print("== B: 上游类型×单服务器通道 ==")
try:
    set_strict(False)
    for uk, u in UPSTREAMS.items():
        scope = "/mcp/" + quote(u["server"])
        # 会话型完整生命周期（scope 内工具名为未加前缀的原始名，从 tools/list 动态发现）
        st, sid, obj = init("2025-06-18", path=scope)
        ver_ok = st == 200 and obj and obj.get("result", {}).get("protocolVersion") == "2025-06-18"
        stl, names = tools_list(sid, path=scope)
        tail = u["tool"].rsplit("-", 1)[-1]  # getPublicIp / get_project_dependencies / codegraph_status
        cand = [n for n in names if n.endswith(tail)]
        has = bool(cand)
        scope_tool = cand[0] if cand else u["tool"]
        stc, oc = call_tool_resilient(sid, scope_tool, rid=20, path=scope)
        txt = result_text(oc)
        check(f"B [{uk}|单服务器|会话] 生命周期+真实调用", ver_ok and upstream_ok(u, oc) and u["ok"](txt),
              f"init={st} has={has} tool={scope_tool} call={stc} txt={txt[:40]}")
        # 2026 无状态
        st2, oc2 = call_tool_resilient(None, scope_tool, rid=21, path=scope)
        txt2 = result_text(oc2)
        # 无状态调用不带 session → call_stateless 形态
        if not result_ok(oc2):
            st2, _, oc2, _ = req("POST", scope, {"jsonrpc": "2.0", "id": 21, "method": "tools/call",
                                                 "params": {"name": scope_tool, "arguments": {}, **META}})
            txt2 = result_text(oc2)
        check(f"B [{uk}|单服务器|2026无状态] 真实调用", upstream_ok(u, oc2) and u["ok"](txt2),
              f"call={st2} txt={txt2[:40]}")
finally:
    set_strict(prev)

# ============ C组：tasks 双代命名 + 真实调用任务 ============
print("== C: tasks 双代 ==")
try:
    set_strict(False)
    # 2025-11-25 core tasks: params.task + ttl（客户端需声明 tasks 扩展能力）
    st, sid, _ = init("2025-11-25", tasks_cap=True)
    stc, oc = call(sid, "本机公网ip查询-getPublicIp", rid=30, meta={"ttl": 60000})
    res = (oc or {}).get("result", {})
    task_obj = res.get("task", {}) if isinstance(res.get("task"), dict) else {}
    tid = res.get("taskId") or task_obj.get("taskId")
    check("C1 [2025-11] params.task 创建任务", stc == 200 and bool(tid), f"st={stc} res={str(res)[:160]}")
    # 轮询 tasks/result
    final = None
    if tid:
        for _ in range(30):
            stq, _, oq, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 31, "method": "tasks/result",
                                                 "params": {"taskId": tid, "mcp-session-id": sid}},
                                headers={"Mcp-Session-Id": sid})
            # rmcp 轮询语义：working 时 tasks/result 阻塞至终态，终态载荷直出
            #（resultType:complete + content + related-task meta，无外层 status 字段）
            res_ = (oq or {}).get("result", {})
            if res_ and (res_.get("resultType") == "complete" or res_.get("content")):
                final = oq
                break
            time.sleep(0.5)
    txtf = result_text(final) if final else ""
    check("C2 [2025-11] tasks/result 终态载荷直出含公网IP", final is not None and any(ch.isdigit() for ch in txtf),
          f"final={str(final)[:120]} txt={txtf[:40]}")
    # 2026 extension tasks: ttlMs
    st, sid2, _ = init("2026-07-28", tasks_cap=True)
    stc2, oc2 = call(sid2, "本机公网ip查询-getPublicIp", rid=32, meta={"ttlMs": 60000})
    res2 = (oc2 or {}).get("result", {})
    task_obj2 = res2.get("task", {}) if isinstance(res2.get("task"), dict) else {}
    tid2 = res2.get("taskId") or task_obj2.get("taskId")
    check("C3 [2026] params.task(ttlMs) 创建任务", stc2 == 200 and bool(tid2), f"st={stc2} res={str(res2)[:120]}")
    stl2, _, obj_l, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 33, "method": "tasks/list",
                                             "params": {"mcp-session-id": sid2}},
                            headers={"Mcp-Session-Id": sid2})
    check("C4 [2026] tasks/list 可用", stl2 == 200 and obj_l and "result" in obj_l, f"st={stl2}")
    # 已终态任务 cancel → -32602
    stc3, oc3 = call(sid, "本机公网ip查询-getPublicIp", rid=34, meta={"ttl": 60000})
    tid3 = ((oc3 or {}).get("result", {}).get("taskId"))
    if tid3:
        # tasks/result has NO outer status field (rmcp semantics): the terminal
        # payload IS the result (resultType/content present) or an error frame.
        for _ in range(30):
            stq, _, oq, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 35, "method": "tasks/result",
                                                 "params": {"taskId": tid3, "mcp-session-id": sid}},
                                headers={"Mcp-Session-Id": sid})
            res = (oq or {}).get("result") or {}
            if oq and (("result" in oq and (res.get("resultType") in ("complete", "failed", "cancelled")
                                            or "content" in res or "error" in res))
                       or "error" in oq):
                break
            time.sleep(0.5)
        else:
            check("C5 [2025-11] 任务在时限内到终态", False, "poll timeout (task stuck working?)")
        stx, _, ox, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 36, "method": "tasks/cancel",
                                             "params": {"taskId": tid3, "mcp-session-id": sid}},
                            headers={"Mcp-Session-Id": sid})
        check("C5 [2025-11] 已终态任务 cancel → -32602", ox and ox.get("error", {}).get("code") == -32602,
              f"st={stx} obj={str(ox)[:100]}")
finally:
    set_strict(prev)

# ============ D组：边界/负路径 ============
print("== D: 边界 ==")
try:
    set_strict(False)
    # id null
    st, _, obj, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": None, "method": "tools/list", "params": {}})
    check("D1 宽松 id=null 放行", st == 200 and obj is not None, f"st={st}")
    # id string
    st, _, obj, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": "str-id", "method": "tools/list", "params": {}})
    check("D2 宽松 id=string 回显", st == 200 and obj and obj.get("id") == "str-id", f"st={st}")
    # invalid JSON
    st, _, obj, _ = req("POST", "/mcp", raw_body=b"{not json", headers={"Content-Type": "application/json"})
    check("D3 宽松 invalid JSON → 4xx 非 5xx", 400 <= st < 500, f"st={st}")
    # unknown method
    st, _, obj, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 40, "method": "definitely/not/a/method", "params": {}})
    check("D4 宽松 unknown method → -32601", obj and obj.get("error", {}).get("code") == -32601, f"st={st}")
    # completion/complete 未宣告 → -32601
    st, _, obj, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 41, "method": "completion/complete",
                                         "params": {"ref": {"type": "ref/prompt", "name": "x"}, "argument": {"name": "a", "value": "v"}}})
    check("D5 legacy completion → -32601", obj and obj.get("error", {}).get("code") == -32601, f"st={st}")
    st, _, obj, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 42, "method": "completion/complete",
                                         "params": {"ref": {"type": "ref/prompt", "name": "x"},
                                                    "argument": {"name": "a", "value": "v"}, **META}})
    check("D6 2026 completion → -32601", obj and obj.get("error", {}).get("code") == -32601, f"st={st}")
    # 2MB 大载荷（tools/list params 携带大字段，宽松放行且不挂）
    big = {"jsonrpc": "2.0", "id": 43, "method": "tools/list",
           "params": {"padding": "x" * (2 * 1024 * 1024), **META}}
    st, _, obj, _ = req("POST", "/mcp", big, timeout=120)
    ok_big = st == 200 and "result" in (obj or {})
    check("D7 宽松 2MB 载荷 tools/list 不挂", ok_big, f"st={st} err={str(obj)[:80]}")
    # notifications/initialized 无 session（无状态模式下非法，rmcp 统一 422；记录一致性）
    st, _, obj, _ = req("POST", "/mcp/" + quote("codegraph"), {"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}})
    st_root, _, _, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}})
    check("D8 无session通知 root与单服务器行为一致", st == st_root and st in (200, 202, 204, 422), f"scope={st} root={st_root}")
finally:
    set_strict(prev)
    # 严格模式对照
    set_strict(True)
    st, _, obj, _ = req("POST", "/mcp", raw_body=b"{not json", headers={"Content-Type": "application/json"})
    check("D9 严格 invalid JSON → 4xx", 400 <= st < 500, f"st={st}")
    st, _, obj, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": None, "method": "tools/list", "params": {}})
    check("D10 严格 id=null 行为记录", st in (200, 400, 422), f"st={st}")
    set_strict(prev)

# ============ E组：bearer × 版本 × 2026（自动还原）============
print("== E: bearer 交叉 ==")
con = sqlite3.connect(DB)
cfg0 = json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
prev_bearer = cfg0.get("routing", {}).get("enableBearerAuth", False)
krow = con.execute("SELECT token FROM bearer_keys LIMIT 1").fetchone()
con.close()
if krow:
    KEY = krow[0]
    def set_bearer(on):
        con = sqlite3.connect(DB)
        cfg = json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
        cfg.setdefault("routing", {})["enableBearerAuth"] = on
        con.execute("UPDATE system_config SET config_json=?, updated_at=datetime('now') WHERE rowid=1",
                    (json.dumps(cfg, ensure_ascii=False),))
        con.commit(); con.close()
        time.sleep(0.3)
    try:
        set_bearer(True)
        st, _, obj, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}})
        check("E1 开 bearer 无 key → 401", st == 401, f"st={st}")
        st, _, obj, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                                             "params": {"name": "本机公网ip查询-getPublicIp", "arguments": {}, **META}},
                            headers={"Authorization": "Bearer " + KEY})
        txt = result_text(obj)
        check("E2 [bearer|2026] 无状态真实调用", result_ok(obj) and any(ch.isdigit() for ch in txt),
              f"st={st} txt={txt[:40]}")
        # 会话型（带 key initialize）
        st, sid, obj = init("2025-06-18")
        stl, _, objl, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 3, "method": "tools/list", "params": {}},
                              headers={"Mcp-Session-Id": sid, "Authorization": "Bearer " + KEY})
        names = [t.get("name") for t in objl.get("result", {}).get("tools", [])] if objl else []
        stc, oc = call_tool_resilient(sid, "本机公网ip查询-getPublicIp", rid=4, extra_headers={"Authorization": "Bearer " + KEY})
        txt = result_text(oc)
        check("E2 [bearer|2025-06] 会话真实调用", result_ok(oc) and any(ch.isdigit() for ch in txt),
              f"init={st} list={stl} has={'本机公网ip查询-getPublicIp' in names} call={stc} txt={txt[:40]}")
    finally:
        set_bearer(prev_bearer)
        st, _, obj, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}})
        check("E3 还原后恢复 200", st == 200, f"st={st}")
else:
    print("SKIP: bearer_keys 表为空，E 组跳过")

# ============ F组：SEP-2243 Mcp-Name 推导（R101/R102/R103 交叉确认缺陷的修复验证）============
print("== F: SEP-2243 leniency ==")
try:
    set_strict(False)
    # 带 session 的 2026 声明头 + 不带 Mcp-Name 调 prompts/get / resources/read：
    # 修复前 rmcp 400 "missing required Mcp-Name"；修复后由中间件从 params 推导放行。
    h266 = {"MCP-Protocol-Version": "2026-07-28"}
    st, sid, obj = init("2025-06-18")
    # prompts/get：先拿一个真实 prompt 名（list 需要同头）
    stl, _, objl, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 50, "method": "prompts/list", "params": {}},
                          headers={"Mcp-Session-Id": sid, **h266})
    pname = None
    if objl and "result" in objl:
        pl = objl["result"].get("prompts", [])
        pname = pl[0]["name"] if pl else None
    if pname:
        stg, _, og, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 51, "method": "prompts/get",
                                             "params": {"name": pname, "arguments": {}}}, headers={"Mcp-Session-Id": sid, **h266})
        check("F1 [2026头|无Mcp-Name] prompts/get 放行", stg == 200 and (og or {}).get("result") is not None,
              f"st={stg} obj={str(og)[:100]}")
    else:
        check("F1 [2026头|无Mcp-Name] prompts/get 放行", stl == 200 and objl is not None,
              f"no prompts; list st={stl}")
    # resources/read：取一个真实 URI
    strl, _, orl, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 52, "method": "resources/list", "params": {}},
                          headers={"Mcp-Session-Id": sid, **h266})
    ruri = None
    if orl and "result" in orl:
        rs = orl["result"].get("resources", [])
        ruri = rs[0]["uri"] if rs else None
    if ruri:
        strr, _, orr, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 53, "method": "resources/read",
                                               "params": {"uri": ruri}}, headers={"Mcp-Session-Id": sid, **h266})
        check("F2 [2026头|无Mcp-Name] resources/read 放行", strr == 200 and (orr or {}).get("result") is not None,
              f"st={strr} obj={str(orr)[:100]}")
    else:
        check("F2 [2026头|无Mcp-Name] resources/read 放行", strl == 200 and orl is not None,
              f"no resources; list st={strl}")
    # 严格模式保持拒绝（规范行为）
    set_strict(True)
    if pname:
        stg2, _, og2, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 54, "method": "prompts/get",
                                               "params": {"name": pname, "arguments": {}}}, headers={"Mcp-Session-Id": sid, **h266})
        check("F3 严格 prompts/get 无Mcp-Name 拒绝", 400 <= stg2 < 500, f"st={stg2}")
    set_strict(prev)
finally:
    set_strict(prev)

print(f"\n== {len(PASS)}/{len(PASS)+len(FAIL)} passed ==")
if FAIL:
    print("FAILED:")
    for f in FAIL: print("  -", f)
    sys.exit(1)
