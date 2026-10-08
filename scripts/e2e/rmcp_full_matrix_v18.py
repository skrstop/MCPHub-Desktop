#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""全矩阵套件 v18（R458+ 复核轮固化）：每协议版本 × 每通道 × 宽松/严格 × 真实公网IP调用。

要求：
- openapi 类型服务器「本机公网ip查询」（上游 ip.3322.net）已连接；
- 分组 Test 存在且包含该服务器（复用 rmcp_public_ip_matrix / round50d 的既有环境约定）。

矩阵：
  版本: 2024-11-05 / 2025-03-26 / 2025-06-18 / 2025-11-25 / 2026-07-28
  通道: root /mcp · group /mcp/Test · scope /mcp/{server} · $smart /mcp/$smart · REST /rest · /api openapi
  模式: 宽松（strictValidation=false，默认）+ 严格（true，DB 热切换，测毕还原）
每项断言（真实调用类）：HTTP 200 + 请求版本==响应版本 + 文本含严格校验的公网 IPv4（ipaddress 解析）。
新特性：server/discover、resultType:complete、CacheableResult(ttlMs/cacheScope，2026 会话)、
ping 门控（2026 -32601 / legacy 空 result）、2026 tasks 扩展门控（tasks/result 2026 -32601）、
SEP-2243 头、严格模式缺陷拒绝（4xx 明确）、2024 退役端点、通知不挂。
"""
import http.client, json, sqlite3, os, re, sys, time, ipaddress, urllib.parse, base64
import os as _os, sys as _sys
_sys.path.insert(0, _os.path.dirname(os.path.abspath(__file__)))
from pin_helper import pin, unpin

def mcp_name_header(tool_name):
    """SEP-2243/RFC2047 encoded-word for non-latin1 tool names; verbatim otherwise."""
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
PV_KEY = "io.modelcontextprotocol/protocolVersion"
CLIENT_META = {PV_KEY: None, "io.modelcontextprotocol/clientInfo": {"name": "m18", "version": "1"},
               "io.modelcontextprotocol/clientCapabilities": {}}
VERSIONS = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25", "2026-07-28"]
IP_RE = re.compile(r"(?<![\d.])(?:\d{1,3}\.){3}\d{1,3}(?![\d.])")

PASS, FAIL, FAILED = [], [], []

def check(name, cond, detail=""):
    if cond:
        PASS.append(name); print(f"PASS | {name}" + (f" ({detail})" if detail else ""))
    else:
        FAIL.append(name); FAILED.append(name); print(f"FAIL | {name} :: {detail[:160]}")

def valid_ip(text):
    m = IP_RE.search(text or "")
    if not m: return None
    try:
        ipaddress.ip_address(m.group(0)); return m.group(0)
    except ValueError:
        return None

def post(path, body, headers=None, timeout=90):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {"Accept": "application/json, text/event-stream", "Content-Type": "application/json"}
    if headers: h.update(headers)
    c.request("POST", path, body=json.dumps(body, ensure_ascii=False).encode(), headers=h)
    r = c.getresponse(); d = r.read().decode("utf-8", "replace")
    sid = r.getheader("mcp-session-id")
    ct = r.getheader("content-type") or ""
    c.close()
    # SSE 帧 → 取最后一个 data: 行的 JSON；纯 JSON 直接解析
    obj = None
    if "data:" in d:
        for line in d.splitlines():
            if line.startswith("data:"):
                try: obj = json.loads(line[5:].strip())
                except Exception: pass
    if obj is None:
        try: obj = json.loads(d)
        except Exception: pass
    return r.status, obj, d, sid, ct

CUR_MODE = ["宽松"]
def mode_is_lenient():
    return CUR_MODE[0] == "宽松"

def set_strict(on):
    con = sqlite3.connect(DB)
    cfg = json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
    cfg.setdefault("mcp", {})["strictValidation"] = bool(on)
    con.execute("UPDATE system_config SET config_json=?, updated_at=datetime('now') WHERE rowid=1",
                (json.dumps(cfg, ensure_ascii=False),))
    con.commit(); con.close()

def get_strict():
    con = sqlite3.connect(DB)
    cfg = json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
    con.close()
    return cfg.get("mcp", {}).get("strictValidation", False)

def full_session(path, version, tag, real_call=True, scope_tool=None):
    """完整 lifecycle：initialize（版本回显一致）→ initialized → tools/call 真实公网IP。"""
    meta = dict(CLIENT_META); meta[PV_KEY] = version
    params = {"protocolVersion": version, "capabilities": {}, "clientInfo": {"name": "m18", "version": "1"}}
    if version == "2026-07-28":
        params["_meta"] = meta
    st, obj, d, sid, ct = post(path, {"jsonrpc":"2.0","id":1,"method":"initialize","params":params})
    pv = (obj or {}).get("result", {}).get("protocolVersion") if obj else None
    if version == "2026-07-28":
        # rmcp 设计：2026 不走 initialize 协商，fallback 最新 legacy 2025-11-25
        check(f"{tag} initialize 200 + 2026 fallback 2025-11-25", st == 200 and pv == "2025-11-25", f"st={st} pv={pv}")
    else:
        check(f"{tag} initialize 200 + 版本回显一致", st == 200 and pv == version, f"st={st} pv={pv}")
    check(f"{tag} initialize serverInfo 存在", bool((obj or {}).get("result", {}).get("serverInfo", {}).get("name")), "")
    if sid:
        st2, _, _, _, _ = post(path, {"jsonrpc":"2.0","method":"notifications/initialized"}, {"Mcp-Session-Id": sid})
        check(f"{tag} notifications/initialized 不挂", st2 in (200, 202), f"st={st2}")
    if not real_call:
        return sid
    call_name = scope_tool or IP_TOOL
    body = {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name": call_name, "arguments": {}}}
    hdrs = {"Mcp-Session-Id": sid} if sid else {}
    if version == "2026-07-28":
        body["params"]["_meta"] = meta
        hdrs["MCP-Protocol-Version"] = "2026-07-28"
        hdrs["Mcp-Method"] = "tools/call"
        hdrs["Mcp-Name"] = mcp_name_header(call_name)
    st, obj, d, _, _ = post(path, body, hdrs)
    ip = valid_ip(d)
    rt = (obj or {}).get("result", {}).get("resultType")
    check(f"{tag} tools/call 公网IP 真实调用", st == 200 and ip is not None, f"st={st} ip={ip}")
    if version == "2026-07-28":
        check(f"{tag} resultType=complete 注入", rt == "complete", f"resultType={rt}")
    else:
        check(f"{tag} resultType 不注入（legacy）", rt is None, f"resultType={rt}")
    # ping：2026 已移除 → -32601；legacy → 空 result
    phdrs = {"Mcp-Session-Id": sid} if sid else {}
    pbody = {"jsonrpc":"2.0","id":3,"method":"ping","params":{}}
    if version == "2026-07-28":
        phdrs["MCP-Protocol-Version"] = "2026-07-28"
        phdrs["Mcp-Method"] = "ping"
        pbody["params"]["_meta"] = meta
    st, obj, d, _, _ = post(path, pbody, phdrs)
    if version == "2026-07-28":
        check(f"{tag} ping → -32601（2026 已移除）", (obj or {}).get("error", {}).get("code") == -32601, f"{str(obj)[:100]}")
    else:
        check(f"{tag} ping → 空 result", st == 200 and "result" in (obj or {}), f"st={st} {str(obj)[:100]}")
    # tasks 门控：2026 → -32601；legacy → 正常 JSON-RPC 响应（result 或工具级错误）
    thdrs = {"Mcp-Session-Id": sid} if sid else {}
    tbody = {"jsonrpc":"2.0","id":4,"method":"tasks/result","params":{"taskId":"nonexistent"}}
    if version == "2026-07-28":
        thdrs["MCP-Protocol-Version"] = "2026-07-28"
        thdrs["Mcp-Method"] = "tasks/result"
        tbody["params"]["_meta"] = meta
    st, obj, d, _, _ = post(path, tbody, thdrs)
    # R158 语义：tasks/result 对 2026 客户端可用（modern 字段名）；未知 taskId 一律 -32602
    check(f"{tag} tasks/result 未知任务 → -32602", (obj or {}).get("error", {}).get("code") == -32602, f"{str(obj)[:100]}")
    return sid

def discover_and_cache(tag, with_session_hdr=None):
    """2026 server/discover + CacheableResult。"""
    meta = dict(CLIENT_META); meta[PV_KEY] = "2026-07-28"
    body = {"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta": meta}}
    st, obj, d, sid, ct = post("/mcp", body, {**(with_session_hdr or {}), "MCP-Protocol-Version": "2026-07-28", "Mcp-Method": "server/discover"})
    res = (obj or {}).get("result") or {}
    svs = res.get("supportedVersions") or []
    check(f"{tag} discover 200 + 全 5 版本", st == 200 and all(v in svs for v in VERSIONS), f"st={st} versions={svs}")
    check(f"{tag} discover ttlMs=3600000 + cacheScope=public",
          res.get("ttlMs") == 3600000 and res.get("cacheScope") == "public", f"{res.get('ttlMs')}/{res.get('cacheScope')}")
    check(f"{tag} discover resultType=complete", res.get("resultType") == "complete", f"{res.get('resultType')}")
    st, obj, d, _, _ = post("/mcp", {"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"_meta": meta}},
                            {**(with_session_hdr or {}), "MCP-Protocol-Version": "2026-07-28", "Mcp-Method": "tools/list"})
    res = (obj or {}).get("result") or {}
    tools = res.get("tools") or []
    has_ip = any(IP_SERVER in t.get("name", "") for t in tools)
    # Origin #1277 parity (R118 round): positive TTL requires upstream
    # freshness records — bounded ≤5000, gateway/builtin → 0. Scope stays private.
    check(f"{tag} tools/list 2026 会话带 ttlMs≤5000/private",
          st == 200 and isinstance(res.get("ttlMs"), (int, float)) and res.get("ttlMs") <= 5000
          and res.get("cacheScope") == "private",
          f"ttl={res.get('ttlMs')} scope={res.get('cacheScope')}")
    check(f"{tag} tools/list 含公网IP服务器", has_ip, f"tools={len(tools)}")

def unsupported_version(tag):
    """2026 通道声明不支持版本 → -32022 + supported 列表。"""
    meta = dict(CLIENT_META); meta[PV_KEY] = "2024-01-01"
    body = {"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name": IP_TOOL, "arguments": {}, "_meta": meta}}
    st, obj, d, _, _ = post("/mcp", body)
    err = (obj or {}).get("error") or {}
    # 宽松模式：unknown _meta pv 注入 unknown header → rmcp 结构化 -32022 + supported；
    # 严格模式：缺版本头 → -32020（同样是明确拒绝，非 5xx）
    if mode_is_lenient():
        check(f"{tag} 不支持版本 → -32022", err.get("code") == -32022, f"{str(obj)[:120]}")
        supported = ((err.get("data") or {}) if isinstance(err.get("data"), dict) else {}).get("supported") or []
        check(f"{tag} -32022 data.supported 列表", len(supported) == 5, f"{supported}")
    else:
        check(f"{tag} 不支持版本 → 明确拒绝(非5xx)", st < 500 and err.get("code") in (-32020, -32022), f"{str(obj)[:120]}")

def channels(version, mode):
    tag = f"[{mode}/{version}]"
    CUR_MODE[0] = mode
    if version == "2026-07-28":
        # 无状态：root + scope + group 直接调用
        full_session("/mcp", version, f"{tag}root", real_call=True)
        full_session(f"/mcp/{SCOPE}", version, f"{tag}scope", real_call=True, scope_tool="getPublicIp")
        full_session(f"/mcp/{GROUP}", version, f"{tag}group", real_call=True, scope_tool=IP_TOOL)
        # $smart：meta 工具链（search → describe）
        meta = dict(CLIENT_META); meta[PV_KEY] = version
        shdrs = {"MCP-Protocol-Version": "2026-07-28", "Mcp-Method": "tools/call", "Mcp-Name": "smart_route_search"}
        body = {"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"smart_route_search","arguments":{"query":"ip"},
                "_meta": meta}}
        st, obj, d, _, _ = post("/mcp/$smart", body, shdrs)
        txt = d
        check(f"{tag}$smart smart_route_search 200 + 结果", st == 200 and (obj or {}).get("result") is not None, f"st={st}")
        shdrs["Mcp-Name"] = "smart_route_call"
        body = {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"smart_route_call",
                "arguments":{"toolName": IP_TOOL,"arguments":{}},"_meta": meta}}
        st, obj, d, _, _ = post("/mcp/$smart", body, shdrs)
        ip = valid_ip(d)
        check(f"{tag}$smart smart_route_call 公网IP", st == 200 and ip is not None, f"st={st} ip={ip}")
        discover_and_cache(f"{tag}")
        unsupported_version(f"{tag}")  # 读取 CUR_MODE 判定宽松/严格预期
    else:
        full_session("/mcp", version, f"{tag}root")
        full_session(f"/mcp/{SCOPE}", version, f"{tag}scope", real_call=True, scope_tool="getPublicIp")
        full_session(f"/mcp/{GROUP}", version, f"{tag}group", real_call=True, scope_tool=IP_TOOL)
        # $smart legacy：initialize 会话 → smart_route_call
        st, obj, d, sid, _ = post("/mcp/$smart", {"jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"m18","version":"1"}}})
        if sid:
            post("/mcp/$smart", {"jsonrpc":"2.0","method":"notifications/initialized"}, {"Mcp-Session-Id": sid})
            st, obj, d, _, _ = post("/mcp/$smart", {"jsonrpc":"2.0","id":2,"method":"tools/call",
                "params":{"name":"smart_route_call","arguments":{"toolName": IP_TOOL,"arguments":{}}}},
                {"Mcp-Session-Id": sid})
            ip = valid_ip(d)
            check(f"{tag}$smart smart_route_call 公网IP", st == 200 and ip is not None, f"st={st} ip={ip}")
        else:
            check(f"{tag}$smart initialize 会话", False, f"st={st}")

def rest_and_api(mode):
    tag = f"[{mode}]"
    # REST 单服务器
    c = http.client.HTTPConnection(HOST, PORT, timeout=60)
    c.request("POST", f"/rest/{SCOPE}/call", body=json.dumps({"tool":"getPublicIp","arguments":{}}).encode(),
              headers={"Content-Type":"application/json","Accept":"application/json, text/event-stream"})
    r = c.getresponse(); d = r.read().decode()
    ip = valid_ip(d)
    check(f"{tag}REST /rest/:server/call 公网IP", r.status == 200 and ip is not None, f"st={r.status} ip={ip}")
    c.close()
    # REST group
    c = http.client.HTTPConnection(HOST, PORT, timeout=60)
    c.request("POST", f"/rest/group/{GROUP}/call",
              body=json.dumps({"server":IP_SERVER,"tool":"getPublicIp","arguments":{}}).encode(),
              headers={"Content-Type":"application/json","Accept":"application/json, text/event-stream"})
    r = c.getresponse(); d = r.read().decode()
    ip = valid_ip(d)
    check(f"{tag}REST /rest/group/{GROUP}/call 公网IP", r.status == 200 and ip is not None, f"st={r.status} ip={ip}")
    c.close()
    # /api openapi 兼容端点（GET tools）
    c = http.client.HTTPConnection(HOST, PORT, timeout=60)
    c.request("GET", f"/api/{SCOPE}/tools/{SCOPE}/getPublicIp", headers={"Accept":"application/json"})
    r = c.getresponse(); d = r.read().decode()
    ip = valid_ip(d)
    check(f"{tag}/api GET tools 公网IP", r.status == 200 and ip is not None, f"st={r.status} ip={ip}")
    c.close()
    # /api openapi.json 形状
    c = http.client.HTTPConnection(HOST, PORT, timeout=60)
    c.request("GET", "/api/openapi.json", headers={"Accept":"application/json"})
    r = c.getresponse(); d = r.read().decode()
    try:
        spec = json.loads(d)
        ok = r.status == 200 and spec.get("openapi", "").startswith("3.") and spec.get("paths")
    except Exception:
        ok = False
    check(f"{tag}/api/openapi.json 形状", ok, f"st={r.status} len={len(d)}")
    c.close()

def leniency_paths(mode):
    """宽松：缺 Accept/缺 Content-Type/非法版本头 放行；严格：4xx 拒绝。不影响工具调用的都放行（宽松）。"""
    tag = f"[{mode}]"
    if mode == "宽松":
        # 缺 Accept 头 initialize → 放行
        c = http.client.HTTPConnection(HOST, PORT, timeout=60)
        c.request("POST", "/mcp", body=json.dumps({"jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"m18","version":"1"}}}).encode(),
            headers={"Content-Type":"application/json"})
        r = c.getresponse(); d = r.read().decode()
        try: pv = json.loads([l for l in d.splitlines() if l.startswith("data:")][-1][5:])["result"]["protocolVersion"]
        except Exception: pv = None
        check(f"{tag}缺 Accept 放行 + 版本回显", r.status == 200 and pv == "2025-11-25", f"st={r.status} pv={pv}")
        c.close()
        # 非法版本头（宽松剥离放行）
        c = http.client.HTTPConnection(HOST, PORT, timeout=60)
        c.request("POST", "/mcp", body=json.dumps({"jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"m18","version":"1"}}}).encode(),
            headers={"Content-Type":"application/json","Accept":"application/json, text/event-stream",
                     "MCP-Protocol-Version":"1999-01-01"})
        r = c.getresponse(); d = r.read().decode()
        try: pv = json.loads([l for l in d.splitlines() if l.startswith("data:")][-1][5:])["result"]["protocolVersion"]
        except Exception: pv = None
        check(f"{tag}非法版本头剥离放行", r.status == 200 and pv == "2025-11-25", f"st={r.status} pv={pv}")
        c.close()
        # 裸请求升格：无会话直接 tools/call → 公网IP
        meta = dict(CLIENT_META); meta[PV_KEY] = "2025-11-25"
        st, obj, d, _, _ = post("/mcp", {"jsonrpc":"2.0","id":1,"method":"tools/call",
            "params":{"name":IP_TOOL,"arguments":{},"_meta":meta}})
        ip = valid_ip(d)
        check(f"{tag}裸请求升格 tools/call 公网IP", st == 200 and ip is not None, f"st={st} ip={ip}")
    else:
        # 严格：缺 Accept → 4xx
        c = http.client.HTTPConnection(HOST, PORT, timeout=60)
        c.request("POST", "/mcp", body=json.dumps({"jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"m18","version":"1"}}}).encode(),
            headers={"Content-Type":"application/json"})
        r = c.getresponse(); d = r.read().decode()
        check(f"{tag}缺 Accept → 4xx 明确拒绝", 400 <= r.status < 500 and r.status != 404, f"st={r.status}")
        c.close()
        # 严格：非法版本头带会话 → 400
        st, obj, d, sid, _ = post("/mcp", {"jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"m18","version":"1"}}})
        c = http.client.HTTPConnection(HOST, PORT, timeout=60)
        c.request("POST", "/mcp", body=json.dumps({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}).encode(),
            headers={"Content-Type":"application/json","Accept":"application/json, text/event-stream",
                     "Mcp-Session-Id": sid or "", "MCP-Protocol-Version":"1999-01-01"})
        r = c.getresponse(); r.read()
        check(f"{tag}非法版本头+会话 → 4xx", 400 <= r.status < 500, f"st={r.status}")
        c.close()

def retire_2024(mode):
    """2024 双端点退役：无会话 GET /mcp 4xx；/mcp/message 422（不再有 endpoint 行为）。"""
    tag = f"[{mode}]"
    c = http.client.HTTPConnection(HOST, PORT, timeout=30)
    c.request("GET", "/mcp", headers={"Accept":"text/event-stream"})
    r = c.getresponse(); r.read(); c.close()
    check(f"{tag}无会话 GET /mcp → 4xx（2024 退役）", 400 <= r.status < 500, f"st={r.status}")
    c = http.client.HTTPConnection(HOST, PORT, timeout=30)
    c.request("POST", "/mcp/message", body=json.dumps({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}).encode(),
              headers={"Content-Type":"application/json","Accept":"application/json, text/event-stream"})
    r = c.getresponse(); r.read(); c.close()
    if mode == "宽松":
        check(f"{tag}/mcp/message 宽松升格放行", r.status == 200, f"st={r.status}")
    else:
        check(f"{tag}/mcp/message 严格退役 422", r.status == 422, f"st={r.status}")

def notifications_and_edges(mode):
    tag = f"[{mode}]"
    st, obj, d, sid, _ = post("/mcp", {"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"m18","version":"1"}}})
    # 畸形 JSON 不挂（5xx = 失败）
    c = http.client.HTTPConnection(HOST, PORT, timeout=30)
    c.request("POST", "/mcp", body=b"{not-json", headers={"Content-Type":"application/json","Accept":"application/json"})
    r = c.getresponse(); r.read(); c.close()
    check(f"{tag}畸形 JSON → 4xx 不挂", 400 <= r.status < 500, f"st={r.status}")
    # notifications/cancelled 不挂
    st, obj, d, _, _ = post("/mcp", {"jsonrpc":"2.0","method":"notifications/cancelled",
        "params":{"requestId":1}}, {"Mcp-Session-Id": sid} if sid else None)
    check(f"{tag}notifications/cancelled 不挂", st in (200, 202), f"st={st}")
    # 未知工具 → -32602
    st, obj, d, _, _ = post("/mcp", {"jsonrpc":"2.0","id":9,"method":"tools/call",
        "params":{"name":"no/such/tool","arguments":{}}}, {"Mcp-Session-Id": sid} if sid else None)
    check(f"{tag}未知工具 → -32602", (obj or {}).get("error", {}).get("code") == -32602, f"{str(obj)[:100]}")
    # string id 保留
    st, obj, d, _, _ = post("/mcp", {"jsonrpc":"2.0","id":"str-id-1","method":"ping","params":{}},
        {"Mcp-Session-Id": sid} if sid else None)
    check(f"{tag}string id 保留", (obj or {}).get("id") == "str-id-1", f"id={((obj or {}).get('id'))}")
    # DELETE 会话
    if sid:
        c = http.client.HTTPConnection(HOST, PORT, timeout=30)
        c.request("DELETE", "/mcp", headers={"Mcp-Session-Id": sid})
        r = c.getresponse(); r.read(); c.close()
        check(f"{tag}DELETE 会话 → 2xx", 200 <= r.status < 300, f"st={r.status}")

def main():
    print(f"== 全矩阵 v18：{len(VERSIONS)} 版本 × 全通道 × 宽松/严格 ==")
    orig_strict = get_strict()
    pin("本机公网ip查询", "getPublicIp")
    try:
        # ── 宽松模式（默认）──
        set_strict(False); time.sleep(0.4)
        for v in VERSIONS:
            channels(v, "宽松")
        rest_and_api("宽松")
        leniency_paths("宽松")
        retire_2024("宽松")
        notifications_and_edges("宽松")
        # ── 严格模式 ──
        set_strict(True); time.sleep(0.4)
        # 严格模式下全部版本全通道真实调用（规范合规客户端不受影响）
        for v in VERSIONS:
            channels(v, "严格")
        rest_and_api("严格")
        leniency_paths("严格")
        retire_2024("严格")
        notifications_and_edges("严格")

    finally:
        set_strict(orig_strict); time.sleep(0.3)
        unpin("本机公网ip查询", "getPublicIp")
    print(f"\n== {len(PASS)}/{len(PASS)+len(FAIL)} passed ==")
    if FAILED:
        print("FAILED:", ", ".join(FAILED[:20]))
        sys.exit(1)

if __name__ == "__main__":
    main()
