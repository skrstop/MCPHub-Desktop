#!/usr/bin/env python3
# 地毯式复核 R200：基线同步后全量矩阵（scripts/e2e/rmcp_carpet_r200.py）
#
# 覆盖（对应 2026-10-07 未提交基线同步：origin 85a530f..3b004d3，59 commits）：
#   A. 5 协议版本 × IP 服务器(openapi) × 5 通道(root/group/单服务器/$smart/$smart组) 真实公网IP调用
#      + 每版本版本回显一致性断言
#   A2. 5 协议版本 × {stdio=codegraph, streamable-http=Idea} root 通道真实调用（openapi 在 A 组）
#   B. #1286 toolDefinitionFields 每字段逐一验证：
#      annotations / outputSchema / title / execution / icons / _meta × (search/describe)
#      + 默认(title,annotations) + 空列表(core four) + 未知字段丢弃 + live 缺字段静默省略
#   C. #1234 fullSchemaTopN：0/1/2/未设置 × (meta listing 形状 / 命中裁剪 / 提示文案)
#      + progressiveDisclosure 优先级覆盖
#   D. #1238 组成员 pinnedTools：$smart/{group} 列表 + 直调 + tools 选择收窄 + meta 名拒绝 + 测毕还原
#   E. #1277 TTL：2026 tools/list ttlMs 有界(≤5000)；prompts/resources list+read ttl=0；$smart=0
#   F. 宽松模式 × 全部 5 版本：裸请求升格（无 initialize/无 Accept/无 session）真实公网IP调用
#      + 缺 jsonrpc / jsonrpc 1.0 / 缺 params / 缺 Content-Type 放行
#   G. 严格模式：缺 Accept 拒绝 + 裸请求拒绝（DB 开关，finally 还原）
#   H. Bearer 顺序修复：无效 key → 2026 prompts/list 401（非空 200）；REST POST 无 key 401；GET 门控
#      （DB 开关 + bearer key，finally 还原）
#   I. 清理断言：GET /mcp 无 session → 400；/mcp/message 退役；
#      2024-11-05 会话 tools/list 无 annotations/outputSchema、call 结果无 structuredContent（strip_2024）
#   J. $smart/{group} 搜索 group 门控（成员 tools 选择过滤）
# 依赖：应用运行于 127.0.0.1:23333；DB 含 group「Test」（成员含「本机公网ip查询」tools=all，
#      playwright/tools=all）；Smart Routing 已启用（standard 模式）。
import json, http.client, sqlite3, os, sys, time, re, ipaddress
from urllib.parse import quote
import sys as _sysc
_sysc.path.insert(0, __import__("os").path.dirname(os.path.abspath(__file__)))
from pin_helper import pin, unpin

HOST, PORT = "127.0.0.1", 23333
DB = os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")
PASS, FAIL = [], []
_last_body = ""
SMART_LAST_ERR = ""
IP_SERVER = "本机公网ip查询"
IP_SERVER_ENC = quote(IP_SERVER, safe="")
IP_TOOL = "getPublicIp"
SEP_IP = f"{IP_SERVER}-{IP_TOOL}"
GROUP = "Test"
GROUP_ENC = quote(GROUP, safe="")
VERSIONS = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25", "2026-07-28"]
META26 = {"_meta": {"protocolVersion": "2026-07-28"}}

# MCP $smart 面 list/call pin parity：A 组 $smart 直调用例需要 pin（结束还原）
pin(IP_SERVER, IP_TOOL)

def check(name, ok, detail=""):
    (PASS if ok else FAIL).append(name)
    print(("PASS" if ok else "FAIL"), "|", name, (("| " + str(detail)[:200]) if (detail and not ok) else ""))

def req(method, path, body=None, headers=None, timeout=90, raw_body=None, default_accept=True):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {}
    if default_accept:
        h["Accept"] = "application/json, text/event-stream"
    if body is not None or raw_body is not None:
        h["Content-Type"] = "application/json"
    if headers: h.update(headers)
    payload = raw_body if raw_body is not None else (
        json.dumps(body, ensure_ascii=False).encode("utf-8") if body is not None else None)
    c.request(method, path, body=payload, headers=h)
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
    global _last_body
    _last_body = data[-160:] if data else ""
    return st, hdrs, obj, data

def init(version, path="/mcp", extra=None, headers=None):
    p = {"protocolVersion": version, "capabilities": {}, "clientInfo": {"name": "carpet200", "version": "1"}}
    if extra: p.update(extra)
    st, hd, obj, _ = req("POST", path, {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": p}, headers=headers)
    return st, hd.get("mcp-session-id", ""), obj

def call(sid, name, args=None, rid=10, path="/mcp", meta=None, extra_headers=None):
    p = {"name": name, "arguments": args or {}}
    if meta: p["task"] = meta
    if sid: p["_meta"] = {"mcp-session-id": sid}
    h = dict(extra_headers or {})
    if sid: h["Mcp-Session-Id"] = sid
    st, _, obj, _ = req("POST", path, {"jsonrpc": "2.0", "id": rid, "method": "tools/call", "params": p}, headers=h)
    return st, obj

def call_ip_resilient(sid, name, args=None, rid=10, path="/mcp", tries=6):
    """ip.3322.net 限流（偶发空 IP/502）——重试直到文本含公网 IP。"""
    last = (None, None)
    for i in range(tries):
        st, oc = call(sid, name, args=args, rid=rid, path=path)
        last = (st, oc)
        if result_ok(oc) and has_ip(result_text(oc)):
            return st, oc
        time.sleep(3)
    return last

def call_resilient(sid, name, args=None, rid=10, path="/mcp", meta=None, extra_headers=None, tries=2):
    last = (None, None)
    for i in range(tries):
        st, oc = call(sid, name, args=args, rid=rid, path=path, meta=meta, extra_headers=extra_headers)
        last = (st, oc)
        if result_ok(oc): return st, oc
        time.sleep(2)
    return last

def call_stateless(name, args=None, rid=10, path="/mcp", version="2026-07-28"):
    st, _, obj, _ = req("POST", path, {"jsonrpc": "2.0", "id": rid, "method": "tools/call",
        "params": {"name": name, "arguments": args or {},
                   "_meta": {"protocolVersion": version}}})
    return st, obj

def tools_list(sid, path="/mcp", rid=5, meta26=False):
    p = dict(META26) if meta26 else {}
    h = {"Mcp-Session-Id": sid} if sid else {}
    st, _, obj, _ = req("POST", path, {"jsonrpc": "2.0", "id": rid, "method": "tools/list", "params": p}, headers=h)
    tools = (obj or {}).get("result", {}).get("tools", []) if obj else []
    return st, tools, obj

def result_text(obj):
    if not obj or "result" not in obj: return ""
    return "".join(str(c.get("text", "")) for c in obj["result"].get("content", []) if isinstance(c, dict))

def result_ok(obj):
    return bool(obj) and "result" in obj and obj["result"].get("isError") is False

import re as _re
IPV4_RE = _re.compile(r"(\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3})")

def has_ip(txt):
    for m in IPV4_RE.finditer(txt or ""):
        try:
            ip = ipaddress.ip_address(m.group(1))
            if not (ip.is_private or ip.is_loopback or ip.is_link_local or ip.is_multicast or ip.is_reserved):
                return True
        except Exception:
            continue
    return False

def wait_connected(server, max_s=120):
    """上游断连（Idea streamable-http 在 IDE 忙时掉线）会令 smart 搜索命中为空——
    B/C 组前置等待该服务器 connected。"""
    con = sqlite3.connect(DB)
    t0 = time.time()
    while time.time() - t0 < max_s:
        try:
            # 通过单服务器通道 initialize+tools/list 探活
            st, data = req("POST", f"/mcp/{quote(server)}", {"jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "probe", "version": "1"}}})
            m = re.search(r"session-id: *(\S+)", data, re.I)
            sid = m.group(1) if m else ""
            if sid:
                st2, data2 = req("POST", f"/mcp/{quote(server)}", {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
                                 headers={"Mcp-Session-Id": sid})
                o = parse_any(data2)
                if o and "result" in o and (o["result"].get("tools") or []):
                    return True
        except Exception:
            pass
        time.sleep(3)
    return False

def parse_any(data):
    try:
        return json.loads(data)
    except Exception:
        frames = []
        for line in data.split("\n"):
            if line.startswith("data: ") and line[6:].strip():
                try: frames.append(json.loads(line[6:]))
                except Exception: pass
        return frames[-1] if frames else None

def db_read_config():
    con = sqlite3.connect(DB)
    cfg = json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
    con.close()
    return cfg

def db_write_config(cfg):
    con = sqlite3.connect(DB)
    con.execute("UPDATE system_config SET config_json=?, updated_at=datetime('now') WHERE rowid=1",
                (json.dumps(cfg, ensure_ascii=False),))
    con.commit(); con.close()
    time.sleep(0.3)

def set_smart(key, value):
    cfg = db_read_config()
    sr = cfg.setdefault("smartRouting", {})
    if value is None:
        sr.pop(key, None)
    else:
        sr[key] = value
    db_write_config(cfg)

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

if not wait_server():
    print("FATAL: server not reachable at", HOST, PORT)
    sys.exit(2)

def smart_search(sid, query, limit=10, path="/mcp", meta26=False, rid=50):
    """smart_route_search via tools/call; returns parsed JSON payload of the text.
    桥接 wire shape: result.content[0].text = JSON(text_response envelope)，
    envelope.content[0].text = payload JSON —— 逐层解包。"""
    global SMART_LAST_ERR
    st, obj = call(sid, "smart_route_search", {"query": query, "limit": limit}, rid=rid, path=path,
                   extra_headers={} if not meta26 else None)
    if not result_ok(obj):
        SMART_LAST_ERR = "st_raw=" + json.dumps({"res": (obj or {}).get("result"), "err": (obj or {}).get("error")}, ensure_ascii=False)[:200] + " body=" + (globals().get("_last_body") or "")[:120]
        return None
    cur = None
    try:
        cur = json.loads(obj["result"]["content"][0]["text"])
        while isinstance(cur, dict) and "tools" not in cur and "content" in cur:
            cur = json.loads(cur["content"][0]["text"])
        SMART_LAST_ERR = ""
        return cur
    except Exception as e:
        SMART_LAST_ERR = f"unwrap {e}: {str(cur)[:120]}"
        return cur

def smart_describe(sid, tool_name, path="/mcp", rid=51):
    st, obj = call(sid, "smart_route_describe", {"toolName": tool_name}, rid=rid, path=path)
    if not result_ok(obj): return None
    cur = None
    try:
        cur = json.loads(obj["result"]["content"][0]["text"])
        while isinstance(cur, dict) and "tool" not in cur and "content" in cur:
            cur = json.loads(cur["content"][0]["text"])
        return cur
    except Exception:
        return cur

def smart_describe_retry(sid, tool_name, path="/mcp", tries=3):
    d = None
    for _ in range(tries):
        d = smart_describe(sid, tool_name, path=path)
        if d and "tool" in d:
            return d
        time.sleep(2)
    return d

# ============ A组：5 版本 × IP服务器 × 5 通道 真实公网IP调用 ============
print("== A: 版本 × IP(openapi) × 通道 真实调用矩阵 ==")
cfg0 = db_read_config()
strict0 = cfg0.get("mcp", {}).get("strictValidation", False)
STRICT_BASELINE = strict0  # raw baseline for final restore (suites must not leak config changes)
if strict0:
    cfg = db_read_config(); cfg.setdefault("mcp", {})["strictValidation"] = False; db_write_config(cfg)

for v in VERSIONS:
    time.sleep(1)  # ip.3322.net 限流节流
    # --- root channel ---
    st, sid, obj = init(v)
    expect_pv = "2025-11-25" if v == "2026-07-28" else v  # rmcp 3.4.1: 2026 无 initialize 握手回退最新 legacy
    got_pv = (obj or {}).get("result", {}).get("protocolVersion")
    stc, oc = call_ip_resilient(sid, SEP_IP, rid=11)
    check(f"A[{v}|root] 版本回显+IP真实调用", st == 200 and got_pv == expect_pv and result_ok(oc) and has_ip(result_text(oc)),
          f"init={st} pv={got_pv}/{expect_pv} call={stc} txt={result_text(oc)[:60]}")

    # --- single-server channel ---
    st2, sid2, obj2 = init(v, path=f"/mcp/{IP_SERVER_ENC}")
    stc2, oc2 = call_ip_resilient(sid2, IP_TOOL, rid=12, path=f"/mcp/{IP_SERVER_ENC}")
    check(f"A[{v}|single] 单服务器通道真实调用", st2 == 200 and result_ok(oc2) and has_ip(result_text(oc2)),
          f"init={st2} call={stc2} txt={result_text(oc2)[:60]}")

    # --- group channel ---
    st3, sid3, obj3 = init(v, path=f"/mcp/{GROUP_ENC}")
    stc3, oc3 = call_ip_resilient(sid3, SEP_IP, rid=13, path=f"/mcp/{GROUP_ENC}")
    check(f"A[{v}|group] 组通道前缀真实调用", st3 == 200 and result_ok(oc3) and has_ip(result_text(oc3)),
          f"init={st3} call={stc3} txt={result_text(oc3)[:60]}")

    # --- $smart root: search -> call meta flow ---
    st4, sid4, obj4 = init(v, path="/mcp/$smart")
    payload = smart_search(sid4, "public ip address", limit=5, path="/mcp/$smart")
    hit_names = [t.get("name") for t in (payload or {}).get("tools", [])]
    stc4, oc4 = call_ip_resilient(sid4, "smart_route_call", {"toolName": SEP_IP, "arguments": {}}, rid=14, path="/mcp/$smart")
    check(f"A[{v}|$smart] 元工具search+call真实IP", payload is not None and SEP_IP in hit_names and result_ok(oc4) and has_ip(result_text(oc4)),
          f"init={st4} hits={hit_names} call={stc4} txt={result_text(oc4)[:60]}")

    # --- $smart/{group} ---
    st5, sid5, obj5 = init(v, path=f"/mcp/$smart/{GROUP_ENC}")
    payload5 = smart_search(sid5, "public ip address", limit=5, path=f"/mcp/$smart/{GROUP_ENC}")
    hit5 = [t.get("name") for t in (payload5 or {}).get("tools", [])]
    stc5, oc5 = call_ip_resilient(sid5, "smart_route_call", {"toolName": SEP_IP, "arguments": {}}, rid=15, path=f"/mcp/$smart/{GROUP_ENC}")
    check(f"A[{v}|$smart/group] 组智能路由真实IP", payload5 is not None and SEP_IP in hit5 and result_ok(oc5) and has_ip(result_text(oc5)),
          f"init={st5} hits={hit5} call={stc5} txt={result_text(oc5)[:60]}")

    # --- 2026 stateless direct (root) ---
    if v == "2026-07-28":
        stc6, oc6 = call_stateless(SEP_IP, rid=16)
        check("A[2026|stateless] 无状态真实IP调用", result_ok(oc6) and has_ip(result_text(oc6)),
              f"call={stc6} txt={result_text(oc6)[:60]}")

# ============ A2组：版本 × {stdio=codegraph, http=Idea} root 通道真实调用 ============
print("== A2: 版本 × stdio/http 上游 真实调用(root) ==")
UP2 = {
    "stdio-codegraph": {"tool": "codegraph-codegraph_status",
                        "ok": lambda t: len(t) > 2},
    "http-idea": {"tool": "Idea-mcp-server-get_project_dependencies",
                  "ok": lambda t: len(t) > 2 or "Unable to determine" in t},
}
for v in VERSIONS:
    st, sid, obj = init(v)
    for uk, u in UP2.items():
        stc, oc = call_resilient(sid, u["tool"], rid=21, tries=2)
        txt = result_text(oc)
        okc = result_ok(oc) or ("Unable to determine" in txt)
        check(f"A2[{v}|{uk}] 真实调用", okc and u["ok"](txt),
              f"call={stc} isError={(oc or {}).get('result',{}).get('isError')} txt={txt[:60]}")

# ============ B组：#1286 toolDefinitionFields 每字段逐一 ============
print("== B: toolDefinitionFields 每字段验证 ==")
# 上游连通性前置：Idea 掉线时其工具不在索引可用集，命中为空是环境现象
IDEA_READY = wait_connected("Idea-mcp-server", max_s=60)
smart0 = db_read_config().get("smartRouting", {})
try:
    IDEA_FULL = "Idea-mcp-server-analyze_calls"  # 有 annotations; 部分工具无 outputSchema
    IDEA_WITH_SCHEMA = "Idea-mcp-server-search_text"  # 有 annotations + outputSchema
    PW_FULL = "playwright-browser_close"  # annotations 存在（title 在 annotations 内，无顶层 title）
    QUERY_IDEA = "analyze calls hierarchy"
    QUERY_PW = "close the browser page"

    def search_hits(sid, query, path="/mcp/$smart", limit=8):
        p = smart_search(sid, query, limit=limit, path=path)
        return (p or {}).get("tools", []) or []

    def fields_of(entry):
        return set(entry.keys())

    CORE = {"name", "description", "inputSchema", "serverName", "score"}

    # B0: 清除自定义（默认 = title, annotations）
    set_smart("toolDefinitionFields", None)
    st, sid, _ = init("2025-11-25", path="/mcp/$smart")
    hits = search_hits(sid, QUERY_PW)
    bc = next((h for h in hits if h.get("name") == PW_FULL), None)
    check("B[default] search命中含annotations(title在annotations内不额外)",
          bc is not None and "annotations" in fields_of(bc) and "title" not in fields_of(bc),
          f"hit={bc}")
    # B1: describe 默认字段（偶发上游抖动重试）
    d = None
    for _ in range(3):
        d = smart_describe(sid, PW_FULL, path="/mcp/$smart")
        if d and "tool" in d:
            break
        time.sleep(2)
    dtool = (d or {}).get("tool", {})
    check("B[default] describe含annotations", "annotations" in set(dtool.keys()),
          f"describe={dtool}")
    del sid

    # 每字段逐一：annotations / outputSchema / title / execution / icons / _meta
    skipped = []
    for field, query, tool in [
        ("annotations", QUERY_PW, PW_FULL),
        ("outputSchema", "search text", IDEA_WITH_SCHEMA),
        ("title", QUERY_PW, PW_FULL),        # 上游无顶层 title → 必须静默省略
        ("execution", QUERY_PW, PW_FULL),    # rmcp 3.4.1 无 execution → 静默省略
        ("icons", QUERY_PW, PW_FULL),        # 上游无 icons → 静默省略
        ("_meta", QUERY_PW, PW_FULL),        # 上游无 _meta → 静默省略
    ]:
        if field == "outputSchema" and not IDEA_READY:
            skipped.append(f"B[{field}]")
            print(f"SKIP | B[{field}] Idea-mcp-server 未连接（环境）")
            continue
        set_smart("toolDefinitionFields", [field])
        st, sid, _ = init("2025-11-25", path="/mcp/$smart")
        hits = search_hits(sid, query)
        tgt = next((h for h in hits if h.get("name") == tool), None)
        if field in ("annotations", "outputSchema"):
            expect_present = field in fields_of(tgt or {})  # 上游真实存在该字段（先确认目标工具确实有）
            check(f"B[{field}] search按配置携带", tgt is not None and expect_present,
                  f"hit_fields={fields_of(tgt) if tgt else None}")
        else:
            # 上游不提供 → 必须静默省略，绝不能出现 null/占位
            absent = tgt is not None and field not in fields_of(tgt)
            check(f"B[{field}] 上游缺失时静默省略", absent, f"hit_fields={fields_of(tgt) if tgt else None}")
        # describe 同字段策略
        d = smart_describe_retry(sid, tool, path="/mcp/$smart")
        dtool = (d or {}).get("tool", {})
        if field in ("annotations", "outputSchema"):
            check(f"B[{field}] describe按配置携带", field in set(dtool.keys()), f"describe_fields={set(dtool.keys())}")
        else:
            check(f"B[{field}] describe上游缺失省略", field not in set(dtool.keys()), f"describe_fields={set(dtool.keys())}")
        del sid

    # B2: 组合字段 annotations+outputSchema
    set_smart("toolDefinitionFields", ["annotations", "outputSchema"])
    st, sid, _ = init("2025-11-25", path="/mcp/$smart")
    hits = search_hits(sid, "search text")
    tgt = next((h for h in hits if h.get("name") == IDEA_WITH_SCHEMA), None)
    fset = fields_of(tgt or {})
    if IDEA_READY:
        check("B[combo] annotations+outputSchema 同时携带",
              tgt is not None and "annotations" in fset and "outputSchema" in fset, f"hit_fields={fset}")
    else:
        print("SKIP | B[combo] Idea-mcp-server 未连接（环境）")
    del sid

    # B3: 空列表 = core four only
    set_smart("toolDefinitionFields", [])
    st, sid, _ = init("2025-11-25", path="/mcp/$smart")
    hits = search_hits(sid, QUERY_PW)
    tgt = next((h for h in hits if h.get("name") == PW_FULL), None)
    fset = fields_of(tgt or {})
    check("B[empty] 空列表仅core four", tgt is not None and fset == CORE, f"hit_fields={fset} core={CORE}")
    d = smart_describe_retry(sid, PW_FULL, path="/mcp/$smart")
    dtool = (d or {}).get("tool", {})
    check("B[empty] describe仅core字段", set(dtool.keys()) == {"name", "description", "inputSchema", "serverName"},
          f"describe_fields={set(dtool.keys())}")
    del sid

    # B4: 未知字段被丢弃（保留合法的）
    set_smart("toolDefinitionFields", ["bogusField", "annotations"])
    st, sid, _ = init("2025-11-25", path="/mcp/$smart")
    hits = search_hits(sid, QUERY_PW)
    tgt = next((h for h in hits if h.get("name") == PW_FULL), None)
    fset = fields_of(tgt or {})
    check("B[unknown] 未知字段丢弃+annotations保留",
          tgt is not None and "annotations" in fset and "bogusField" not in fset, f"hit_fields={fset}")
    del sid

    # B5: REST $smart search 同样受 toolDefinitionFields 影响（同一 handle_search_tools）
    st, _, restobj, _ = req("POST", "/api/$smart/search", {"query": QUERY_PW, "limit": 5})
    check("B[rest] /api/$smart/search 正常返回", st == 200 and restobj is not None, f"st={st}")
finally:
    cfg = db_read_config()
    cfg["smartRouting"] = smart0
    db_write_config(cfg)

# ============ C组：#1234 fullSchemaTopN ============
print("== C: fullSchemaTopN ==")
try:
    # C0: 未设置 → standard 模式 2 个 meta 工具（search/call），所有命中带 inputSchema
    set_smart("fullSchemaTopN", None)
    st, sid, _ = init("2025-11-25", path="/mcp/$smart")
    _, tl, _ = tools_list(sid, path="/mcp/$smart")
    names = [t.get("name") for t in tl]
    check("C[none] standard默认2个meta工具", "smart_route_describe" not in names and "smart_route_search" in names and "smart_route_call" in names,
          f"names={names}")
    hits = smart_search(sid, "browser", path="/mcp/$smart", limit=8)
    entries = (hits or {}).get("tools", [])
    all_schema = all("inputSchema" in e for e in entries) if entries else False
    check("C[none] 所有命中带inputSchema", bool(entries) and all_schema, f"entries={len(entries)}")
    del sid

    # C1: fullSchemaTopN=2 → 3 个 meta 工具 + 前2带schema，其余裁剪 + 提示文案
    set_smart("fullSchemaTopN", 2)
    st, sid, _ = init("2025-11-25", path="/mcp/$smart")
    _, tl, _ = tools_list(sid, path="/mcp/$smart")
    names = [t.get("name") for t in tl]
    check("C[n=2] describe进入meta列表", "smart_route_describe" in names, f"names={names}")
    # search 描述提示 top 2
    desc = next((t.get("description") for t in tl if t.get("name") == "smart_route_search"), "")
    check("C[n=2] search描述含first 2提示", "first 2 results" in desc, desc[:120])
    p = smart_search(sid, "browser", limit=6, path="/mcp/$smart")
    entries = (p or {}).get("tools", [])
    with_schema = [e for e in entries if "inputSchema" in e]
    without = [e for e in entries if "inputSchema" not in e]
    guideline = ((p or {}).get("metadata") or {}).get("guideline", "")
    check("C[n=2] 恰好前2带schema其余裁剪", len(entries) >= 3 and len(with_schema) == 2 and len(without) >= 1,
          f"total={len(entries)} with={len(with_schema)} without={len(without)} err={SMART_LAST_ERR}")
    check("C[n=2] guideline提describe_tool", "smart_route_describe" in guideline, guideline[:160] + " err=" + SMART_LAST_ERR)
    # describe 补 schema
    trimmed_name = without[0]["name"] if without else None
    d = smart_describe_retry(sid, trimmed_name, path="/mcp/$smart") if trimmed_name else None
    dtool = (d or {}).get("tool", {})
    check("C[n=2] describe补出inputSchema", "inputSchema" in set(dtool.keys()), f"describe={dtool}")
    del sid

    # C2: fullSchemaTopN=0 → 全部裁剪 + 文案「do not include」
    set_smart("fullSchemaTopN", 0)
    st, sid, _ = init("2025-11-25", path="/mcp/$smart")
    _, tl, _ = tools_list(sid, path="/mcp/$smart")
    desc = next((t.get("description") for t in tl if t.get("name") == "smart_route_search"), "")
    check("C[n=0] 描述含do not include", "do not include the inputSchema" in desc, desc[:160])
    p = smart_search(sid, "browser", limit=4, path="/mcp/$smart")
    entries = (p or {}).get("tools", [])
    check("C[n=0] 全部命中无inputSchema", bool(entries) and all("inputSchema" not in e for e in entries),
          f"entries={[e.get('name') for e in entries][:3]} err={SMART_LAST_ERR}")
    del sid

    # C3: progressive 覆盖 fullSchemaTopN（progressive 模式下 topN 忽略）
    set_smart("progressiveDisclosure", True)
    set_smart("fullSchemaTopN", 2)
    st, sid, _ = init("2025-11-25", path="/mcp/$smart")
    _, tl, _ = tools_list(sid, path="/mcp/$smart")
    names = [t.get("name") for t in tl]
    check("C[progressive] progressive模式3工具+忽略topN", "smart_route_describe" in names and "smart_route_call" in names,
          f"names={names}")
    p = smart_search(sid, "browser", limit=4, path="/mcp/$smart")
    entries = (p or {}).get("tools", [])
    check("C[progressive] 命中无inputSchema(渐进模式)", bool(entries) and all("inputSchema" not in e for e in entries),
          f"entries={[e.get('name') for e in entries][:3]} err={SMART_LAST_ERR}")
    del sid
finally:
    cfg = db_read_config()
    cfg["smartRouting"] = smart0
    db_write_config(cfg)

# ============ D组：#1238 组成员 pinnedTools ============
print("== D: 组成员 pinnedTools ==")
con = sqlite3.connect(DB)
row = con.execute("SELECT servers FROM groups WHERE name=?", (GROUP,)).fetchone()
orig_members = row[0] if row else None
con.close()
try:
    members = json.loads(orig_members)
    # 找到 IP 成员，加 pinnedTools=[getPublicIp]
    for m in members:
        if m.get("name") == IP_SERVER:
            m["pinnedTools"] = [IP_TOOL]
    con = sqlite3.connect(DB)
    con.execute("UPDATE groups SET servers=? WHERE name=?", (json.dumps(members, ensure_ascii=False), GROUP))
    con.commit(); con.close()
    time.sleep(0.4)

    st, sid, _ = init("2025-11-25", path=f"/mcp/$smart/{GROUP_ENC}")
    _, tl, _ = tools_list(sid, path=f"/mcp/$smart/{GROUP_ENC}")
    names = [t.get("name") for t in tl]
    meta_names = [n for n in names if n and n.startswith("smart_route_")]
    pinned_listed = [n for n in names if n and IP_TOOL in n]
    check("D[listing] $smart/{Test} 列出组成员pin", len(meta_names) >= 2 and any(n == SEP_IP or n == IP_TOOL for n in pinned_listed),
          f"names={names[:12]}")
    # 直调 pin（Test 组可见成员 = playwright + IP → 多服务器 → 前缀名）
    pin_name = SEP_IP if SEP_IP in names else IP_TOOL
    stc, oc = call_resilient(sid, pin_name, rid=31, path=f"/mcp/$smart/{GROUP_ENC}")
    check("D[direct] pin直调真实IP", result_ok(oc) and has_ip(result_text(oc)), f"call={stc} txt={result_text(oc)[:60]}")
    # 非 pin 工具拒绝（playwright-browser_close 在 Test 组可见但未 pin）
    stc2, oc2 = call(sid, "playwright-browser_close", rid=32, path=f"/mcp/$smart/{GROUP_ENC}")
    errtxt = (oc2 or {}).get("error", {}).get("message", "") if oc2 and "error" in oc2 else ""
    check("D[reject] 未pin工具直调被拒", oc2 is not None and ("not a pinned tool" in errtxt or "is not a pinned" in errtxt),
          f"call={stc2} err={errtxt[:80]}")
    del sid

    # D2: tools 选择收窄：pin 不在成员 tools 选择内 → 不列出、不可调
    members2 = json.loads(orig_members)
    for m in members2:
        if m.get("name") == IP_SERVER:
            m["pinnedTools"] = [IP_TOOL]
            m["tools"] = ["somethingElse"]  # 选择不含 pin
    con = sqlite3.connect(DB)
    con.execute("UPDATE groups SET servers=? WHERE name=?", (json.dumps(members2, ensure_ascii=False), GROUP))
    con.commit(); con.close()
    time.sleep(0.4)
    st, sid, _ = init("2025-11-25", path=f"/mcp/$smart/{GROUP_ENC}")
    _, tl, _ = tools_list(sid, path=f"/mcp/$smart/{GROUP_ENC}")
    names = [t.get("name") for t in tl]
    leaked = [n for n in names if n and IP_TOOL in n]
    check("D[narrow] 选择外pin不列出", not leaked, f"leaked={leaked}")
    stc3, oc3 = call(sid, SEP_IP, rid=33, path=f"/mcp/$smart/{GROUP_ENC}")
    errtxt3 = ""
    if oc3 and "error" in oc3: errtxt3 = oc3["error"].get("message", "")
    check("D[narrow] 选择外pin直调被拒", "pinned" in errtxt3 or "not found" in errtxt3.lower(), f"err={errtxt3[:80]}")
    del sid

    # D3: meta 名 pin 拒绝（pin 叫 smart_route_search → 不可列不可调）
    members3 = json.loads(orig_members)
    for m in members3:
        if m.get("name") == IP_SERVER:
            m["pinnedTools"] = ["smart_route_search"]
    con = sqlite3.connect(DB)
    con.execute("UPDATE groups SET servers=? WHERE name=?", (json.dumps(members3, ensure_ascii=False), GROUP))
    con.commit(); con.close()
    time.sleep(0.4)
    st, sid, _ = init("2025-11-25", path=f"/mcp/$smart/{GROUP_ENC}")
    _, tl, _ = tools_list(sid, path=f"/mcp/$smart/{GROUP_ENC}")
    names = [t.get("name") for t in tl]
    smart_named = [n for n in names if n and IP_SERVER in n and "smart_route" in n]
    check("D[meta-name] meta名pin不重复列出", not smart_named, f"leaked={smart_named}")
    del sid
finally:
    if orig_members is not None:
        con = sqlite3.connect(DB)
        con.execute("UPDATE groups SET servers=? WHERE name=?", (orig_members, GROUP))
        con.commit(); con.close()
        time.sleep(0.3)

# ============ E组：#1277 TTL 行为 ============
print("== E: TTL(freshness) ==")
st, sid, obj = init("2026-07-28")
# 2026 initialize 回退 2025-11-25 → 会话仍可断言 tools/list（is_2026_session 需要 2026 会话，
# rmcp 2026 无握手 → 用 _meta 无状态请求断言）
stl, _, tl_obj = tools_list(sid)
ttl_from_list = (tl_obj or {}).get("result", {}).get("ttlMs")
if ttl_from_list is None and tl_obj and "result" in tl_obj:
    ttl_from_list = tl_obj["result"].get("ttlMs")
check("E[2026-session-list] tools/list ttlMs 有界(≤5000)", ttl_from_list is None or ttl_from_list <= 5000,
      f"ttlMs={ttl_from_list}")
# 无状态 2026 tools/list
sts, _, sobj, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 5, "method": "tools/list", "params": dict(META26)})
sttl = (sobj or {}).get("result", {}).get("ttlMs")
check("E[2026-stateless] tools/list ttlMs≤5000", sttl is None or sttl <= 5000, f"ttlMs={sttl}")
# prompts/resources → ttl 0（或无键）
st9, _, pobj, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 6, "method": "prompts/list", "params": dict(META26)})
pttl = (pobj or {}).get("result", {}).get("ttlMs")
check("E[2026] prompts/list ttl=0", pttl in (0, None), f"ttlMs={pttl}")
st10, _, robj, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 7, "method": "resources/list", "params": dict(META26)})
rttl = (robj or {}).get("result", {}).get("ttlMs")
check("E[2026] resources/list ttl=0", rttl in (0, None), f"ttlMs={rttl}")
# $smart → 0
sts2, _, sobj2, _ = req("POST", "/mcp/$smart", {"jsonrpc": "2.0", "id": 8, "method": "tools/list", "params": dict(META26)})
sttl2 = (sobj2 or {}).get("result", {}).get("ttlMs")
check("E[2026|$smart] ttl=0", sttl2 in (0, None), f"ttlMs={sttl2}")

# ============ F组：宽松模式 × 全部 5 版本（用户要求：所有版本都支持宽松） ============
print("== F: 宽松模式全版本 ==")
def bare_call(path="/mcp", version_meta=True, rid=40):
    params = {"name": SEP_IP, "arguments": {}}
    if version_meta:
        params["_meta"] = {"protocolVersion": "2025-11-25"}  # 由 leniency 升格
    return req("POST", path, {"jsonrpc": "2.0", "id": rid, "method": "tools/call", "params": params},
               default_accept=False)  # 无 Accept 头 → 宽松补全

for v in VERSIONS:
    params = {"name": SEP_IP, "arguments": {}}
    if v != "2026-07-28":
        params["_meta"] = {"protocolVersion": v}
    st, _, obj, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 40, "method": "tools/call", "params": params},
                        default_accept=False)
    check(f"F[{v}] 裸请求升格真实IP调用", result_ok(obj) and has_ip(result_text(obj)),
          f"st={st} txt={result_text(obj)[:60]}")
# F2: 缺 jsonrpc / jsonrpc 1.0 / 缺 params / 缺 Content-Type（2025-11-25 代表性 + 2026）
raw_variants = [
    ("缺jsonrpc", {"id": 41, "method": "tools/call", "params": {"name": SEP_IP, "arguments": {}, "_meta": {"protocolVersion": "2025-11-25"}}}),
    ("jsonrpc1.0", {"jsonrpc": "1.0", "id": 42, "method": "tools/call", "params": {"name": SEP_IP, "arguments": {}, "_meta": {"protocolVersion": "2025-11-25"}}}),
]
for nm, body in raw_variants:
    st, _, obj, _ = req("POST", "/mcp", body)
    check(f"F[{nm}] 宽松放行+真实IP", result_ok(obj) and has_ip(result_text(obj)), f"st={st} txt={result_text(obj)[:50]}")
# 缺 params：JSON-RPC 合法省略 params；裸 tools/list（无需 params）必须放行；
# 裸 tools/call 缺 params → 注入 {} 后缺 name，rmcp 拒绝为预期（无法指名工具）。
st, _, obj, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 43, "method": "tools/list"},
                    default_accept=False)
check("F[缺params] 裸tools/list注入{}放行", st == 200 and obj and "result" in obj, f"st={st}")
st, _, obj, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 43, "method": "tools/call"})
check("F[缺params] tools/call缺name拒绝(预期)", st >= 400 or (obj and "error" in obj), f"st={st}")
# 缺 Content-Type 由 leniency 注入：单独发一个无 CT 的 raw 请求
c = http.client.HTTPConnection(HOST, PORT, timeout=60)
c.request("POST", "/mcp", body=json.dumps({"jsonrpc": "2.0", "id": 44, "method": "tools/call",
    "params": {"name": SEP_IP, "arguments": {}, **META26}}).encode(), headers={"Accept": "application/json, text/event-stream"})
r = c.getresponse(); d = r.read().decode("utf-8", "replace"); c.close()
try:
    o = json.loads(d)
except Exception:
    o = None
    for line in d.split("\n"):
        if line.startswith("data: "):
            try: o = json.loads(line[6:])
            except Exception: pass
check("F[缺Content-Type] 宽松注入放行", result_ok(o) and has_ip(result_text(o)), f"st={r.status} txt={result_text(o)[:50]}")

# ============ G组：严格模式 ============
print("== G: 严格模式 ==")
cfg = db_read_config()
prev_strict = cfg.get("mcp", {}).get("strictValidation", False)
try:
    cfg.setdefault("mcp", {})["strictValidation"] = True
    db_write_config(cfg)
    time.sleep(0.3)
    st, _, obj, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "s", "version": "1"}}},
        default_accept=False)
    check("G[strict] 缺Accept拒绝", st >= 400, f"st={st}")
    st2, _, obj2, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": SEP_IP, "arguments": {}, "_meta": {"protocolVersion": "2025-11-25"}}},
        default_accept=False)
    check("G[strict] 裸请求拒绝", st2 >= 400, f"st={st2}")
    # 正常头可用
    st3, sid3, obj3 = init("2025-11-25")
    stc3, oc3 = call_resilient(sid3, SEP_IP, rid=45)
    check("G[strict] 完整头正常调用", st3 == 200 and result_ok(oc3) and has_ip(result_text(oc3)),
          f"init={st3} txt={result_text(oc3)[:50]}")
finally:
    cfg = db_read_config()
    # Restore the RAW baseline, not the mid-suite value (F group ran lenient,
    # so prev_strict was already False — restoring it would permanently
    # disable strictValidation for operators whose baseline is strict).
    cfg.setdefault("mcp", {})["strictValidation"] = STRICT_BASELINE
    db_write_config(cfg)

# ============ H组：Bearer 顺序修复 + REST 门控 ============
print("== H: Bearer ==")
cfg = db_read_config()
prev_bearer = cfg.get("routing", {}).get("enableBearerAuth", False)
TEST_KEY = "e2e-r200-bearer-key"
con = sqlite3.connect(DB)
con.execute("DELETE FROM bearer_keys WHERE token=?", (TEST_KEY,))
con.execute("INSERT INTO bearer_keys (id, name, token, enabled, access_type, allowed_groups, allowed_servers) VALUES (?,?,?,?,?,?,?)",
            ("e2e-r200-bk-0001", "r200", TEST_KEY, 1, "all", "[]", "[]"))
con.commit(); con.close()
try:
    cfg = db_read_config()
    cfg.setdefault("routing", {})["enableBearerAuth"] = True
    db_write_config(cfg)
    time.sleep(0.5)
    # H1: 无效 key → 2026 prompts/list 401（修复点：先 bearer 后 smart/空列表短路）
    st, _, obj, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 1, "method": "prompts/list", "params": dict(META26)},
                        headers={"Authorization": "Bearer wrong-key-xyz"})
    check("H[order] 无效key→prompts 401(非空200)", st == 401, f"st={st}")
    st, _, obj, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 2, "method": "resources/list", "params": dict(META26)},
                        headers={"Authorization": "Bearer wrong-key-xyz"})
    check("H[order] 无效key→resources 401", st == 401, f"st={st}")
    # H2: 有效 key → 正常
    st, _, obj, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": {"name": SEP_IP, "arguments": {}, **META26}},
        headers={"Authorization": f"Bearer {TEST_KEY}"})
    check("H[valid] 有效key真实IP调用", result_ok(obj) and has_ip(result_text(obj)), f"st={st} txt={result_text(obj)[:50]}")
    # H3: REST POST 无 key → 401（新增中间件）
    st, _, obj, _ = req("POST", f"/rest/{IP_SERVER_ENC}/call", {"tool": IP_TOOL, "arguments": {}})
    check("H[rest] REST POST无key 401", st == 401, f"st={st}")
    st, _, obj, _ = req("POST", f"/rest/{IP_SERVER_ENC}/call", {"tool": IP_TOOL, "arguments": {}},
                        headers={"Authorization": f"Bearer {TEST_KEY}"})
    # REST 响应形状：{result: [...], is_error: false}
    rest_txt = ""
    if isinstance(obj, dict):
        rest_txt = "".join(str(c.get("text", "")) for c in obj.get("result", []) if isinstance(c, dict))
    check("H[rest] REST POST带key真实IP", st == 200 and obj.get("is_error") is False and has_ip(rest_txt),
          f"st={st} body={str(obj)[:120]}")
    # H4: GET /rest/{server}/tools 无 key → 401
    st, _, obj, _ = req("GET", f"/rest/{IP_SERVER_ENC}/tools")
    check("H[rest] REST GET无key 401", st == 401, f"st={st}")
    # H5: /health 不需要 bearer
    st, _, obj, _ = req("GET", "/health")
    check("H[health] health免bearer", st == 200, f"st={st}")
finally:
    cfg = db_read_config()
    cfg.setdefault("routing", {})["enableBearerAuth"] = prev_bearer
    db_write_config(cfg)
    con = sqlite3.connect(DB)
    con.execute("DELETE FROM bearer_keys WHERE token=?", (TEST_KEY,))
    con.commit(); con.close()

# ============ I组：清理断言 + strip_2024 ============
print("== I: 清理/strip_2024 ==")
st, _, obj, _ = req("GET", "/mcp")
check("I[retired] GET /mcp无session 400", st == 400, f"st={st}")
st, _, obj, retired_body = req("POST", "/mcp/message", {"jsonrpc": "2.0", "id": 1, "method": "ping"})
# 2024 双端点已退役：/mcp/message 现为普通 scope 路径（裸升格/退役语义），绝不能再出现
# 2024 endpoint 行为（返回含 "endpoint" 的 SSE 事件）。宽松模式下裸 ping 升格 2026 → -32601（HTTP 200）。
retired_ok = (st >= 400) or (obj is not None and "error" in obj) or ("endpoint" not in (retired_body or ""))
check("I[retired] /mcp/message 退役(无endpoint行为)", retired_ok, f"st={st} body={str(obj)[:120]}")
# 2024 会话 strip：playwright browser_close 带 annotations；2024 会话必须剥离
st, sid24, _ = init("2024-11-05")
_, tl24, _ = tools_list(sid24)
bc24 = next((t for t in tl24 if t.get("name") == "playwright-browser_close"), None)
check("I[2024] tools/list剥离annotations/outputSchema",
      bc24 is not None and "annotations" not in bc24 and "outputSchema" not in bc24,
      f"tool_keys={sorted(bc24.keys()) if bc24 else None}")
_, tl25, _ = tools_list(sid24)  # same session, but re-init 2025 for contrast
st, sid25, _ = init("2025-11-25")
_, tl25, _ = tools_list(sid25)
bc25 = next((t for t in tl25 if t.get("name") == "playwright-browser_close"), None)
check("I[2025] tools/list保留annotations", bc25 is not None and "annotations" in bc25,
      f"tool_keys={sorted(bc25.keys()) if bc25 else None}")
# 2024 call 结果剥离 structuredContent（IP 工具结果有文本 content；structuredContent 若上游有则剥离）
stc, oc24 = call_resilient(sid24, SEP_IP, rid=46)
res24 = (oc24 or {}).get("result", {})
check("I[2024] call结果无structuredContent", result_ok(oc24) and "structuredContent" not in res24,
      f"res_keys={sorted(res24.keys())}")
stc, oc25 = call_resilient(sid25, SEP_IP, rid=47)
res25 = (oc25 or {}).get("result", {})
check("I[2025] call结果键正常", result_ok(oc25), f"res_keys={sorted(res25.keys())}")

# ============ J组：$smart/{group} 搜索 group 门控 ============
print("== J: $smart 组搜索门控 ==")
st, sid, _ = init("2025-11-25", path=f"/mcp/$smart/{GROUP_ENC}")
p = smart_search(sid, "browser page", path=f"/mcp/$smart/{GROUP_ENC}", limit=10)
entries = (p or {}).get("tools", [])
servers_in = {e.get("serverName") for e in entries}
# Test 组可见成员 = playwright + IP；命中应只来自组内
check("J[group-gate] 搜索命中仅组内服务器", bool(entries) and servers_in <= {"playwright", IP_SERVER},
      f"servers={servers_in}")
del sid

print()
print(f"TOTAL: {len(PASS)+len(FAIL)} | PASS: {len(PASS)} | FAIL: {len(FAIL)}")
unpin(IP_SERVER, IP_TOOL)
if FAIL:
    print("FAILED CASES:")
    for f in FAIL: print(" -", f)
sys.exit(1 if FAIL else 0)
