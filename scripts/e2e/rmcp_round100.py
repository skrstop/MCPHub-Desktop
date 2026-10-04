#!/usr/bin/env python3
"""Round-100 复核增量用例（rmcp 迁移第 2 轮全量复核）。

新增覆盖：
- H1 panic 回归：裸请求 params._meta 为非对象（string/array/number）→ 不得 5xx/挂断，升格放行
- H2 404 语义：REST 未知工具 → 404；错误体含真实原因
- L1 宽松模式 × 全部 5 版本：缺 Accept + 缺 client 元数据，initialize/tools/call 全放行
- C1 版本 × 通道矩阵：每版本共享 /mcp + 单服务器 scope 各一次真实公网IP调用
"""
import json, http.client, sys

HOST, PORT = "127.0.0.1", 23333
PASS, FAIL = [], []
VERSIONS = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25", "2026-07-28"]

def req(method, path, body=None, headers=None, timeout=60):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
    if headers: h.update(headers)
    payload = json.dumps(body, ensure_ascii=False).encode("utf-8") if body is not None else None
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

def init(version, headers=None):
    st, hd, raw = req("POST", "/mcp", {
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": version, "capabilities": {},
                   "clientInfo": {"name": "round100", "version": "0"}}},
        headers=headers or {})
    obj = sse_last(raw, 1)
    return st, hd, obj

def find_ip_tool(sid):
    st, _, raw = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
                     headers={"Mcp-Session-Id": sid})
    obj = sse_last(raw, 2)
    for t in obj.get("result", {}).get("tools", []):
        if "getPublicIp" in t.get("name", ""):
            return t["name"]
    return None

def call_ip(sid, name, rid=3):
    st, _, raw = req("POST", "/mcp", {
        "jsonrpc": "2.0", "id": rid, "method": "tools/call",
        "params": {"name": name, "arguments": {}}},
        headers={"Mcp-Session-Id": sid})
    return st, sse_last(raw, rid)

import re, ipaddress
IP_RE = re.compile(r"(?<![\d.])(?:\d{1,3}\.){3}\d{1,3}(?![\d.])")
def looks_like_ip(text):
    t = (text or "").strip()
    if IP_RE.search(t):
        try:
            ipaddress.ip_address(IP_RE.search(t).group(0)); return True
        except ValueError:
            pass
    return False

# ── H1 panic 回归：_meta 非对象 ─────────────────────────────────────────────
for label, meta in [("string", "x"), ("array", []), ("number", 5), ("null", None)]:
    st, _, raw = req("POST", "/mcp", {
        "jsonrpc": "2.0", "id": 1, "method": "tools/list",
        "params": {"_meta": meta}}, headers={"Accept": "application/json"})
    obj = sse_last(raw, 1)
    ok = st == 200 and obj is not None and "tools" in obj.get("result", {})
    check(f"H1 bare _meta={label} 不挂断且放行", ok, f"status={st} obj={str(obj)[:120]}")

# _meta 为非对象但带版本头（非 bare 路径也要安全）
st, _, raw = req("POST", "/mcp", {
    "jsonrpc": "2.0", "id": 1, "method": "tools/list",
    "params": {"_meta": "junk"}},
    headers={"mcp-protocol-version": "2026-07-28", "Accept": "application/json"})
ok = st in (200, 400)  # 不得 5xx/panic
check("H1 非bare _meta=string 不 5xx", st < 500, f"status={st}")

# ── H2 REST 未知工具 404 ────────────────────────────────────────────────────
st, _, raw = req("POST", "/rest/%E6%9C%AC%E6%9C%BA%E5%85%AC%E7%BD%91ip%E6%9F%A5%E8%AF%A2/call",
                 {"tool": "definitely_not_a_tool_42", "arguments": {}})
ok = st == 404 and ("not found" in raw.lower() or "no such" in raw.lower())
check("H2 REST 未知工具 → 404", ok, f"status={st} body={raw[:120]}")

# ── L1 宽松模式 × 全部 5 版本（缺 Accept + 缺 client 元数据）───────────────
for v in VERSIONS:
    # 无 Accept 头 initialize
    st, hd, obj = init(v, headers={"Accept": "application/json"})
    negotiated = obj.get("result", {}).get("protocolVersion") if obj else None
    ok = st == 200 and negotiated is not None
    # rmcp 语义：2026 是无状态 per-request 版本路由，initialize 不 mint 会话，
    # 协商按 rmcp 降级为 2025-11-25（见全量回归 TC-23，既定基线）。
    expected_nego = "2025-11-25" if v == "2026-07-28" else v
    check(f"L1 宽松[{v}] 无event-stream Accept initialize 放行", ok, f"status={st} nego={negotiated}")
    if not ok:
        continue
    check(f"L1 宽松[{v}] 版本回显一致", negotiated == expected_nego, f"nego={negotiated}")
    sid = hd.get("mcp-session-id")
    if v == "2026-07-28":
        # 2026 无状态：直接调
        st, _, raw = req("POST", "/mcp", {
            "jsonrpc": "2.0", "id": 5, "method": "tools/call",
            "params": {"name": "本机公网ip查询-getPublicIp", "arguments": {}}}, timeout=90)
        obj = sse_last(raw, 5)
        r = obj.get("result", {}) if obj else {}
        check("L1 宽松[2026] 公网IP 真实调用", st == 200 and r.get("isError") is False
              and looks_like_ip(json.dumps(r.get("content", []), ensure_ascii=False)),
              f"status={st} r={str(r)[:120]}")
        continue
    if not sid:
        check(f"L1 宽松[{v}] session 头存在", False, "no session id")
        continue
    name = find_ip_tool(sid)
    ok = name is not None
    check(f"L1 宽松[{v}] IP 工具暴露", ok)
    if not ok:
        continue
    st, obj = call_ip(sid, name)
    r = obj.get("result", {}) if obj else {}
    ok = st == 200 and r.get("isError") is False and looks_like_ip(json.dumps(r.get("content", []), ensure_ascii=False))
    check(f"L1 宽松[{v}] 公网IP 真实调用", ok, f"status={st} r={str(r)[:140]}")

# ── C1 版本 × 通道矩阵（共享 /mcp + 单服务器 scope）────────────────────────
for v in VERSIONS:
    st, hd, obj = init(v)
    sid = hd.get("mcp-session-id")
    ok = st == 200 and sid is not None
    check(f"C1 共享[{v}] initialize+session", ok, f"status={st}")
    if not ok:
        continue
    name = find_ip_tool(sid)
    st, obj = call_ip(sid, name) if name else (0, None)
    r = obj.get("result", {}) if obj else {}
    ok = st == 200 and r.get("isError") is False
    check(f"C1 共享[{v}] 公网IP 调用", ok, f"status={st}")
    if not ok:
        continue
    # 单服务器 scope 通道
    path = "/mcp/%E6%9C%AC%E6%9C%BA%E5%85%AC%E7%BD%91ip%E6%9F%A5%E8%AF%A2"
    if v == "2026-07-28":
        st2, _, raw = req("POST", path, {
            "jsonrpc": "2.0", "id": 8, "method": "tools/call",
            "params": {"name": "getPublicIp", "arguments": {}}}, timeout=90)
        obj = sse_last(raw, 8)
        r = obj.get("result", {}) if obj else {}
        check("C1 单服务器[2026] 公网IP 调用", st2 == 200 and r.get("isError") is False, f"status={st2}")
    else:
        st2, hd2, raw = req("POST", path, {
            "jsonrpc": "2.0", "id": 7, "method": "initialize",
            "params": {"protocolVersion": v, "capabilities": {},
                       "clientInfo": {"name": "round100", "version": "0"}}})
        obj = sse_last(raw, 7)
        nego = obj.get("result", {}).get("protocolVersion") if obj else None
        sid2 = hd2.get("mcp-session-id")
        ok = st2 == 200 and nego == v and sid2
        check(f"C1 单服务器[{v}] initialize（版本回显）", ok, f"status={st2} nego={nego}")
        if ok:
            st3, _, raw = req("POST", path, {
                "jsonrpc": "2.0", "id": 9, "method": "tools/call",
                "params": {"name": "getPublicIp", "arguments": {}}},
                headers={"Mcp-Session-Id": sid2}, timeout=90)
            obj = sse_last(raw, 9)
            r = obj.get("result", {}) if obj else {}
            check(f"C1 单服务器[{v}] 公网IP 调用", st3 == 200 and r.get("isError") is False,
                  f"status={st3} r={str(r)[:120]}")

print(f"\n== {len(PASS)}/{len(PASS)+len(FAIL)} passed ==")
if FAIL:
    print("FAILED:", *FAIL, sep="\n  - ")
sys.exit(1 if FAIL else 0)
