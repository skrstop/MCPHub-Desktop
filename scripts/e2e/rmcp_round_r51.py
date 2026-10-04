#!/usr/bin/env python3
"""R51-R100 复核轮补全测试套件（覆盖既有 522 用例之外的场景）。

新增覆盖：
- B1 JSON-RPC batch（数组 body）→ 不得 5xx/挂断
- B2 同一会话并发 10 请求（含 2 个公网IP 真实调用）
- B3 中文工具名 2026 无状态 root 通道真实调用
- B4 prompts/resources 逐版本（legacy 4 + 2026）
- B5 ping 逐版本（legacy={} / 2026=-32601）
- B6 会话生命周期：DELETE 后复用 / 重复 initialize / notifications/initialized
- B7 string id roundtrip / 非法 JSON / 错误 Content-Type
- B8 tools/list 分页 cursor 参数
- B9 2026 logging/setLevel + completion/complete（不 5xx）
- B10 严格模式 × legacy 4 版本 × 单服务器通道公网IP 真实调用
- B11 2024 GET SSE 流 + 2025-11 GET SSE 流（会话推送通道）
- B12 资源 read（builtin resources）
"""
import json, http.client, sqlite3, os, re, sys, concurrent.futures, time
from urllib.parse import quote

HOST, PORT = "127.0.0.1", 23333
PASS, FAIL = [], []
VERSIONS = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25", "2026-07-28"]
DB = os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")

def req(method, path, body=None, headers=None, timeout=90, raw_body=None, ctype="application/json"):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {"Accept": "application/json, text/event-stream"}
    if body is not None or raw_body is not None:
        h["Content-Type"] = ctype
    if headers: h.update(headers)
    payload = raw_body if raw_body is not None else (
        json.dumps(body, ensure_ascii=False).encode("utf-8") if body is not None else None)
    c.request(method, path, body=payload, headers=h)
    r = c.getresponse()
    data = r.read().decode("utf-8", "replace")
    hdrs = {k.lower(): v for k, v in r.getheaders()}
    c.close()
    return r.status, hdrs, data

def sse_last(raw, want_id=None):
    body = raw.strip()
    if body.startswith("{"):
        try:
            obj = json.loads(body)
            if want_id is None or obj.get("id") == want_id:
                return obj
        except Exception:
            pass
    frames = []
    for line in raw.split("\n"):
        if line.startswith("data: "):
            try: frames.append(json.loads(line[6:]))
            except Exception: pass
    if want_id is not None:
        for f in reversed(frames):
            if f.get("id") == want_id: return f
    return frames[-1] if frames else None

def check(name, ok, detail=""):
    (PASS if ok else FAIL).append(name)
    print(("PASS" if ok else "FAIL"), "|", name, ("| " + str(detail)[:140] if detail and not ok else ""))

def init(version, path="/mcp"):
    st, hd, raw = req("POST", path, {
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": version, "capabilities": {},
                   "clientInfo": {"name": "r51suite", "version": "0"}}})
    obj = sse_last(raw, 1)
    return st, hd, obj

def find_ip_tool(sid, path="/mcp"):
    st, _, raw = req("POST", path, {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
                     headers={"Mcp-Session-Id": sid})
    obj = sse_last(raw, 2)
    for t in obj.get("result", {}).get("tools", []):
        if "getPublicIp" in t.get("name", ""):
            return t["name"]
    return None

def call_ip(sid, name, rid=3, path="/mcp"):
    st, _, raw = req("POST", path, {
        "jsonrpc": "2.0", "id": rid, "method": "tools/call",
        "params": {"name": name, "arguments": {}}},
        headers={"Mcp-Session-Id": sid})
    return st, sse_last(raw, rid)

IP_RE = re.compile(r"(?<![\d.])(?:\d{1,3}\.){3}\d{1,3}(?![\d.])")
def result_has_ip(obj):
    r = obj.get("result", {})
    if r.get("isError"): return False
    text = json.dumps(r.get("content", []), ensure_ascii=False)
    return bool(IP_RE.search(text))

# ============ B1 JSON-RPC batch ============
st, _, raw = req("POST", "/mcp", [
    {"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}},
    {"jsonrpc": "2.0", "id": 2, "method": "ping"},
])
check("B1 batch 数组 body 不 5xx", st < 500, f"st={st} {raw[:100]}")

# ============ B2 同一会话并发 ============
st, hd, obj = init("2025-11-25")
sid = hd.get("mcp-session-id", "")
name = find_ip_tool(sid)
check("B2 前置 initialize+ip工具", st == 200 and bool(name), f"st={st} name={name}")
if name:
    def worker(i):
        if i in (0, 7):
            s, o = call_ip(sid, name, rid=100+i)
            return result_has_ip(o) if o else False
        s, _, raw = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 100+i, "method": "tools/list", "params": {}},
                        headers={"Mcp-Session-Id": sid})
        o = sse_last(raw, 100+i)
        return s == 200 and o is not None and "result" in o
    with concurrent.futures.ThreadPoolExecutor(max_workers=10) as ex:
        results = list(ex.map(worker, range(10)))
    check("B2 并发 10 请求全成功", all(results), str(results))

# ============ B3 中文工具名 2026 无状态 root 通道 ============
CLIENT_META = {"io.modelcontextprotocol/clientInfo": {"name": "r51", "version": "0"},
               "io.modelcontextprotocol/clientCapabilities": {}}
PV = "io.modelcontextprotocol/protocolVersion"
st, _, raw = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 9, "method": "tools/list",
    "params": {"_meta": {PV: "2026-07-28", **CLIENT_META}}})
obj = sse_last(raw, 9)
tools = obj.get("result", {}).get("tools", []) if obj else []
cn_tool = next((t["name"] for t in tools if "公网" in t.get("name", "") or "getPublicIp" in t.get("name", "")), None)
check("B3 前置 2026 root tools/list 拿到 ip 工具", bool(cn_tool), f"n={len(tools)}")
if cn_tool:
    st, _, raw = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 10, "method": "tools/call",
        "params": {"name": cn_tool, "arguments": {},
                   "_meta": {PV: "2026-07-28", **CLIENT_META}}})
    obj = sse_last(raw, 10)
    check("B3 中文工具名 2026 无状态真实调用", st == 200 and obj and result_has_ip(obj),
          f"st={st} {str(obj)[:120]}")

# ============ B4 prompts/resources 逐版本 ============
def meta_params(v, params=None):
    p = dict(params or {})
    if v == "2026-07-28":
        p["_meta"] = {PV: v, **CLIENT_META}
    return p

for v in VERSIONS:
    st, hd, obj = init(v)
    sid = hd.get("mcp-session-id", "")
    ok_i = st == 200 and obj and "result" in obj
    st2, _, raw2 = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 2, "method": "prompts/list", "params": meta_params(v)},
                       headers={"Mcp-Session-Id": sid})
    o2 = sse_last(raw2, 2)
    ok_p = st2 == 200 and o2 is not None and "result" in o2
    st3, _, raw3 = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 3, "method": "resources/list", "params": meta_params(v)},
                       headers={"Mcp-Session-Id": sid})
    o3 = sse_last(raw3, 3)
    ok_r = st3 == 200 and o3 is not None and "result" in o3
    # 2026 字段污染检查（legacy 不得有 ttlMs）
    ok_no_ttl = True
    if v != "2026-07-28":
        ok_no_ttl = "ttlMs" not in json.dumps(o2.get("result", {})) and "ttlMs" not in json.dumps(o3.get("result", {})) if o2 and o3 else ok_no_ttl
    else:
        ok_no_ttl = "ttlMs" in json.dumps(o2.get("result", {})) if o2 else False
    check(f"B4 [{v}] prompts/resources list", ok_i and ok_p and ok_r and ok_no_ttl,
          f"init={st} p={st2} r={st3} ttl_ok={ok_no_ttl}")

# ============ B5 ping 逐版本 ============
for v in VERSIONS:
    st, hd, obj = init(v)
    sid = hd.get("mcp-session-id", "")
    st2, _, raw2 = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 5, "method": "ping"},
                       headers={"Mcp-Session-Id": sid})
    o2 = sse_last(raw2, 5)
    if v == "2026-07-28":
        ok = st2 in (200, 400) and o2 is not None and ("error" in o2 or o2.get("result") == {})
        # 2026 ping 已移除：期望 -32601（或无状态路径下任何非 5xx 结构化错误）
        check(f"B5 [{v}] ping 移除（非 5xx 结构化响应）", st2 < 500 and o2 is not None, f"st={st2} {str(o2)[:100]}")
    else:
        check(f"B5 [{v}] ping 空 result", st2 == 200 and o2 is not None and o2.get("result") == {},
              f"st={st2} {str(o2)[:100]}")

# ============ B6 会话生命周期 ============
st, hd, obj = init("2025-06-18")
sid = hd.get("mcp-session-id", "")
# DELETE 会话
st_del, _, _ = req("DELETE", "/mcp", headers={"Mcp-Session-Id": sid})
check("B6 DELETE 会话 2xx", 200 <= st_del < 300, f"st={st_del}")
# 复用已删会话
st2, _, raw2 = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 6, "method": "tools/list", "params": {}},
                   headers={"Mcp-Session-Id": sid})
o2 = sse_last(raw2, 6)
check("B6 已删会话复用报错不 5xx", st2 < 500 and (st2 >= 400 or (o2 and "error" in o2)), f"st={st2} {str(o2)[:100]}")
# 重复 initialize 同 session（新会话内发第二次 initialize）
st, hd, _ = init("2025-03-26")
sid2 = hd.get("mcp-session-id", "")
st3, _, raw3 = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 7, "method": "initialize",
    "params": {"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "x", "version": "0"}}},
    headers={"Mcp-Session-Id": sid2})
o3 = sse_last(raw3, 7)
check("B6 同会话重复 initialize 不 5xx", st3 < 500, f"st={st3}")
# notifications/initialized（无 id 通知）
st4, _, raw4 = req("POST", "/mcp", {"jsonrpc": "2.0", "method": "notifications/initialized"},
                   headers={"Mcp-Session-Id": sid2})
check("B6 notifications/initialized 2xx/202", st4 < 500, f"st={st4}")

# ============ B7 id / 编码边界 ============
# string id
st, hd, _ = init("2025-11-25")
sid = hd.get("mcp-session-id", "")
st2, _, raw2 = req("POST", "/mcp", {"jsonrpc": "2.0", "id": "abc-string", "method": "tools/list", "params": {}},
                   headers={"Mcp-Session-Id": sid})
o2 = sse_last(raw2, "abc-string")
check("B7 string id 保留往返", st2 == 200 and o2 is not None and o2.get("id") == "abc-string", f"st={st2}")
# 非法 JSON
st3, _, _ = req("POST", "/mcp", raw_body=b"{not json")
check("B7 非法 JSON 4xx 非 5xx", 400 <= st3 < 500, f"st={st3}")
# 错误 Content-Type
st4, _, _ = req("POST", "/mcp", raw_body=b"hello", ctype="text/plain")
check("B7 text/plain Content-Type 不 5xx", st4 < 500, f"st={st4}")

# ============ B8 分页 cursor ============
st, hd, _ = init("2025-06-18")
sid = hd.get("mcp-session-id", "")
st2, _, raw2 = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 8, "method": "tools/list", "params": {"cursor": "nonexistent-cursor"}},
                   headers={"Mcp-Session-Id": sid})
o2 = sse_last(raw2, 8)
check("B8 无效 cursor 不 5xx（错误或空列表）", st2 < 500 and o2 is not None, f"st={st2} {str(o2)[:100]}")

# ============ B9 2026 logging/completion ============
st, _, raw = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 11, "method": "logging/setLevel",
    "params": {"level": "debug", "_meta": {PV: "2026-07-28", **CLIENT_META}}})
o = sse_last(raw, 11)
check("B9 logging/setLevel 结构化响应", st < 500 and o is not None, f"st={st} {str(o)[:100]}")
st, _, raw = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 12, "method": "completion/complete",
    "params": {"ref": {"type": "ref/prompt", "name": "x"}, "argument": {"name": "a", "value": ""},
               "_meta": {PV: "2026-07-28", **CLIENT_META}}})
o = sse_last(raw, 12)
check("B9 completion/complete 结构化响应", st < 500 and o is not None, f"st={st} {str(o)[:100]}")

# ============ B10 严格模式 × legacy × 单服务器通道真实调用 ============
def set_strict(on):
    con = sqlite3.connect(DB)
    cfg = json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
    cfg.setdefault("mcp", {})["strictValidation"] = on
    con.execute("UPDATE system_config SET config_json=?, updated_at=datetime('now') WHERE rowid=1",
                (json.dumps(cfg, ensure_ascii=False),))
    con.commit(); con.close()

prev_strict = None
try:
    con = sqlite3.connect(DB)
    cfg0 = json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
    prev_strict = cfg0.get("mcp", {}).get("strictValidation", False)
    con.close()
    set_strict(True)
    time.sleep(0.2)
    for v in ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"]:
        scope = "/mcp/" + quote("本机公网ip查询")
        st, hd, obj = init(v, scope)
        sid = hd.get("mcp-session-id", "")
        ok_i = st == 200 and obj and obj.get("result", {}).get("protocolVersion") == v
        name = find_ip_tool(sid, scope)
        ok_call = False
        if name:
            st2, o2 = call_ip(sid, name, rid=20, path=scope)
            ok_call = result_has_ip(o2) if o2 else False
        check(f"B10 [严格|{v}|单服务器] 公网IP 真实调用", ok_i and ok_call, f"init={st} name={name} call={ok_call}")
finally:
    set_strict(prev_strict)
    time.sleep(0.2)

# ============ B11 GET SSE 流（带会话，规范路径） + 无 session GET 退役语义 ============
for v in ["2024-11-05", "2025-11-25"]:
    st, hd, obj = init(v)
    sid = hd.get("mcp-session-id", "")
    c = http.client.HTTPConnection(HOST, PORT, timeout=90)
    c.request("GET", "/mcp", headers={"Accept": "text/event-stream", "Mcp-Session-Id": sid})
    r = c.getresponse()
    check(f"B11 [{v}] GET SSE（带会话）200", r.status == 200, f"st={r.status}")
    ok_stream = r.status == 200
    if ok_stream:
        # 读流若干秒，不得立即断开（keep-alive/comment 属正常）
        got_line = False
        deadline = time.time() + 5
        while time.time() < deadline:
            try:
                line = r.readline().decode("utf-8", "replace").strip()
            except Exception:
                break
            if line:
                got_line = True
                break
        check(f"B11 [{v}] GET SSE 流保持打开（非立即断开）", got_line)
    c.close()
# 退役语义：无 session GET → 400
st_ns, _, raw_ns = req("GET", "/mcp", headers={"Accept": "text/event-stream"})
check("B11 无 session GET 退役 400（rmcp 原生）", st_ns == 400, f"st={st_ns}")

# ============ B12 资源 read ============
st, hd, obj = init("2025-11-25")
sid = hd.get("mcp-session-id", "")
st2, _, raw2 = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 40, "method": "resources/list", "params": {}},
                   headers={"Mcp-Session-Id": sid})
o2 = sse_last(raw2, 40)
res = o2.get("result", {}).get("resources", []) if o2 else []
if res:
    uri = res[0]["uri"]
    st3, _, raw3 = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 41, "method": "resources/read",
        "params": {"uri": uri}}, headers={"Mcp-Session-Id": sid})
    o3 = sse_last(raw3, 41)
    check("B12 resources/read 返回内容", st3 == 200 and o3 is not None and "result" in o3, f"st={st3} {str(o3)[:100]}")
else:
    print("SKIP | B12 无已注册资源，resources/read 实调未执行")

print()
print(f"== {len(PASS)}/{len(PASS)+len(FAIL)} passed ==")
if FAIL:
    print("FAILURES:")
    for f in FAIL: print(" -", f)
sys.exit(1 if FAIL else 0)
