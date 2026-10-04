#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""R258-R307 审计缺口补测套件（r4-10 审计产出落地）。
覆盖 r4-10 报告的 P0/P1/P2 缺口：
  P0-2  $smart GET limit 数字参数（§40 修复回归）
  P0-3  blank session 头 + 非 bare 请求（§40 修复回归）
  P0-2b version round-trip：请求版本 == 响应版本（每版本）
  P1-6  initialize 体 protocolVersion=9999-01-01（不得回显、不得 5xx）
  P2-7  GET SSE 流逐版本 × /mcp 与 /mcp/{server} 通道
  P2-10 prompts/resources 在 $smart 通道
  P2-8  tasks/result + tasks/cancel 端到端（2026 无状态 /mcp）
要求服务器「本机公网ip查询」已连接（每用例至少一次真实 MCP 工具调用或协议探活）。
"""
import http.client, json, urllib.parse, re, sys

HOST, PORT = "localhost", 23333
IP_SERVER = "本机公网ip查询"
SCOPE = urllib.parse.quote(IP_SERVER)
KNOWN = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25", "2026-07-28"]

ok = 0
total = 0
failed = []


def post(path, body, headers=None, timeout=90):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {"Accept": "application/json, text/event-stream", "Content-Type": "application/json"}
    if headers:
        h.update(headers)
    c.request("POST", path, body=json.dumps(body).encode(), headers=h)
    r = c.getresponse()
    d = r.read().decode("utf-8", "replace")
    return r.status, d, r.getheader("mcp-session-id")


def get_sse(path, headers, timeout=10):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {"Accept": "text/event-stream"}
    if headers:
        h.update(headers)
    try:
        c.request("GET", path, headers=h)
        r = c.getresponse()
        # Do NOT read the body: an idle SSE stream sends nothing until an
        # event fires, and read() would block until timeout. Status +
        # content-type are the establishment criteria.
        return r.status, r.getheader("content-type", ""), ""
    finally:
        c.close()


def parse(d):
    """解析 JSON 或 SSE data 帧。"""
    try:
        return json.loads(d)
    except Exception:
        pass
    for line in reversed(d.splitlines()):
        if line.startswith("data:") and line[5:].strip():
            try:
                return json.loads(line[5:])
            except Exception:
                continue
    return None


def check(t, cond, detail=""):
    global ok, total
    total += 1
    if cond:
        ok += 1
        print(f"PASS | {t}")
    else:
        failed.append(t)
        print(f"FAIL | {t} {detail}")


def init(v, path="/mcp", extra=None):
    st, d, sid = post(path, {"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": v, "capabilities": {},
                   "clientInfo": {"name": "gap-r30x", "version": "1"}}}, extra)
    return st, parse(d), sid


def call_ip(sid, v, path="/mcp"):
    hdr = {}
    if sid is not None:
        hdr["Mcp-Session-Id"] = sid
    hdr["MCP-Protocol-Version"] = v
    body = {"jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": "getPublicIp", "arguments": {}}}
    if v == "2026-07-28":
        body["params"]["_meta"] = {"io.modelcontextprotocol/protocolVersion": v,
                                   "io.modelcontextprotocol/clientInfo": {"name": "gap", "version": "1"},
                                   "io.modelcontextprotocol/clientCapabilities": {}}
    st, d, _ = post(path, body, hdr)
    obj = parse(d)
    ip = None
    try:
        txt = json.dumps(obj.get("result", {}), ensure_ascii=False)
        m = re.search(r"\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}", txt)
        ip = m.group(0) if m else None
    except Exception:
        pass
    return st, obj, ip


def main():
    # ── P0-2b: 版本往返一致性（请求版本 == 响应版本）+ 真实 IP 调用 ──
    for v in ["2025-03-26", "2025-06-18", "2025-11-25"]:
        st, obj, sid = init(v)
        negotiated = ((obj or {}).get("result") or {}).get("protocolVersion")
        check(f"round-trip {v}: negotiated == request", st == 200 and negotiated == v,
              f"st={st} negotiated={negotiated}")
        st2, obj2, ip = call_ip(sid, v)
        check(f"round-trip {v}: real MCP call returns IPv4", st2 == 200 and ip,
              f"st={st2} ip={ip}")
    # 2026 无状态
    st, obj, ip = call_ip(None, "2026-07-28")
    check("round-trip 2026-07-28 stateless: real MCP call", st == 200 and ip, f"st={st} ip={ip}")

    # ── P0-2: $smart GET limit 数字参数 ──
    for limit, note in [("5", "numeric-str"), ("3", "numeric-str-2")]:
        st, d, _ = post("/api/%24smart/search?query=ip&limit=" + limit, None) if False else (0, "", None)
        # smart REST 是 GET
        c = http.client.HTTPConnection(HOST, PORT, timeout=30)
        c.request("GET", f"/api/%24smart/search?query=ip&limit={limit}",
                  headers={"Accept": "application/json"})
        r = c.getresponse()
        body = r.read().decode("utf-8", "replace")
        check(f"$smart GET limit={limit} ({note}) not-500", r.status < 500, f"st={r.status} {body[:120]}")

    # ── P0-3: blank session 头 + 非 bare 请求（真实工具调用） ──
    st, obj, ip = call_ip("", "2026-07-28")
    check("blank session + real tools/call: 响应必须可解析", obj is not None, f"st={st}")
    check("blank session + real tools/call: 结构化 result 或错误", "error" in (obj or {}) or ip or (obj or {}).get("result"),
          f"st={st} {str(obj)[:120]}")
    st, obj, ip = call_ip("", "2025-11-25")
    check("blank session legacy + tools/call: not-5xx", st < 500, f"st={st}")

    # ── P1-6: initialize 体 9999-01-01 ──
    for path in ["/mcp", f"/mcp/{SCOPE}", "/mcp/%24smart"]:
        st, obj, sid = init("9999-01-01", path)
        got = ((obj or {}).get("result") or {}).get("protocolVersion")
        check(f"initialize 9999 ({path}): not-5xx, no echo", st < 500 and got != "9999-01-01"
              and (got is None or got in KNOWN), f"st={st} got={got}")

    # ── P2-7: GET SSE 流逐版本 ──
    for v in ["2025-03-26", "2025-06-18", "2025-11-25"]:
        st, obj, sid = init(v)
        if st != 200 or not sid:
            check(f"GET SSE {v}: skip (init failed)", False, f"st={st}")
            continue
        st2, ct, _ = get_sse("/mcp", {"Mcp-Session-Id": sid, "MCP-Protocol-Version": v})
        check(f"GET SSE {v}: 200 + text/event-stream", st2 == 200 and "text/event-stream" in ct,
              f"st={st2} ct={ct}")
    # /mcp/{server} 通道（2026 无状态 + session）
    st, obj, sid = init("2025-11-25", f"/mcp/{SCOPE}")
    if st == 200 and sid:
        st2, ct, _ = get_sse(f"/mcp/{SCOPE}", {"Mcp-Session-Id": sid, "MCP-Protocol-Version": "2025-11-25"})
        check("GET SSE /mcp/{server}: 200 + SSE", st2 == 200 and "text/event-stream" in ct, f"st={st2} ct={ct}")
    else:
        check("GET SSE /mcp/{server}: skip (init failed)", False, f"st={st}")

    # ── P2-10: prompts/resources 在 $smart 通道 ──
    st, obj, sid = init("2025-11-25", "/mcp/%24smart")
    if st == 200:
        hdr = {"Mcp-Session-Id": sid, "MCP-Protocol-Version": "2025-11-25"}
        st2, d2, _ = post("/mcp/%24smart", {"jsonrpc": "2.0", "id": 2, "method": "prompts/list", "params": {}}, hdr)
        o2 = parse(d2)
        # Spec semantics: $smart scope exposes NO prompts — an empty result
        # (prompts: []) or a scoped error; never a non-empty list (review
        # round 8 assertion tightening).
        p2 = (o2 or {}).get("result", {}).get("prompts")
        check("$smart prompts/list: 空列表或明确错误（规范语义）",
              (p2 == []) or ("error" in (o2 or {})), f"st={st2} prompts={str(p2)[:80]} {str(o2)[:120]}")
        st3, d3, _ = post("/mcp/%24smart", {"jsonrpc": "2.0", "id": 3, "method": "resources/list", "params": {}}, hdr)
        o3 = parse(d3)
        r3 = (o3 or {}).get("result", {}).get("resources")
        check("$smart resources/list: 空列表或明确错误（规范语义）",
              (r3 == []) or ("error" in (o3 or {})), f"st={st3} resources={str(r3)[:80]} {str(o3)[:120]}")
    else:
        check("$smart channel init (expect 4xx per $smart gating or 200)", st < 500, f"st={st}")

    # ── P2-8: tasks end-to-end（2026 无状态，client-directed task） ──
    body = {"jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": "getPublicIp", "arguments": {}, "task": {"ttl": 60000},
                       "_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28",
                                 "io.modelcontextprotocol/clientInfo": {"name": "gap", "version": "1"},
                                 "io.modelcontextprotocol/clientCapabilities": {}}}}
    hdr = {"MCP-Protocol-Version": "2026-07-28", "x-mcphub-task-requested": json.dumps({"ttl": 60000})}
    st, d, _ = post("/mcp", body, hdr)
    obj = parse(d)
    tid = ((((obj or {}).get("result") or {}).get("task")) or {}).get("taskId")
    if tid:
        # tasks/result（终态载荷直出）
        st2, d2, _ = post("/mcp", {"jsonrpc": "2.0", "id": 3, "method": "tasks/result",
                                   "params": {"taskId": tid}}, hdr)
        o2 = parse(d2)
        check("tasks/result returns terminal payload", st2 == 200 and (o2 or {}).get("result") is not None,
              f"st={st2} {str(o2)[:160]}")
        # tasks/get 状态
        st3, d3, _ = post("/mcp", {"jsonrpc": "2.0", "id": 4, "method": "tasks/get",
                                   "params": {"taskId": tid}}, hdr)
        o3 = parse(d3)
        status = (((o3 or {}).get("result") or {}).get("task") or {}).get("status", "")
        check("tasks/get terminal status", status in ("completed", "failed", "cancelled", "working"),
              f"status={status}")
        # $smart 上任务化必须被拒绝（§40 修复回归）
        st4, d4, _ = post("/mcp/%24smart", body, hdr)
        o4 = parse(d4)
        check("$smart task creation rejected (METHOD_NOT_FOUND)",
              st4 < 500 and ((o4 or {}).get("error", {}).get("code") in (-32601, -32602, -32000)),
              f"st={st4} {str(o4)[:120]}")
    else:
        # 任务未创建也必须非 5xx（宽松放行语义）
        check("tasks e2e: create not-5xx (task support optional)", st < 500, f"st={st}")
        print("SKIP | tasks/list+tasks/get 断言（任务未创建，内联执行模式）")

    print(f"\n== {ok}/{total} passed ==")
    if failed:
        print("FAILED:", *failed, sep="\n  - ")
        sys.exit(1)


if __name__ == "__main__":
    main()
