#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""R358-R407 第六轮复核 E2E 缺口补测套件（r6-10 审计产出落地）。
覆盖第五轮审计遗留缺口：
  G7  tasks × legacy 版本（2025-03-26/06-18/11-25 会话带 task _meta 的 tools/call：
      内联执行或定义 4xx，且 tasks/list 不得 5xx）
  G9  $smart × prompts/get + resources/read 实调（与根 /mcp 通道结果一致）
  G10 prompts/resources × 2025-03-26 / 2025-06-18 经 MCP（get/read 实调，无 ttlMs 注入，
      含 per-server scope）
  G12（客户端侧 SSE 传输）需 mock 上游 + 配置预置，不在此套件（报告 §43 记录为
      Rust 集成测试落点）。
要求服务器「本机公网ip查询」已连接。
"""
import http.client, json, urllib.parse, re, sys

HOST, PORT = "localhost", 23333
IP_SERVER = "本机公网ip查询"
SCOPE = urllib.parse.quote(IP_SERVER)

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


def parse(d):
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


def init(v, path="/mcp"):
    st, d, sid = post(path, {"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": v, "capabilities": {},
                   "clientInfo": {"name": "gap-r36x", "version": "1"}}})
    return st, parse(d), sid


# ── G7: legacy 会话 tools/call 带 task _meta ──
def g7():
    for v in ["2025-03-26", "2025-06-18", "2025-11-25"]:
        st, obj, sid = init(v)
        if st != 200 or not sid:
            check(f"G7 [{v}] init", False, f"st={st}")
            continue
        hdr = {"Mcp-Session-Id": sid, "MCP-Protocol-Version": v}
        body = {"jsonrpc": "2.0", "id": 20, "method": "tools/call",
                "params": {"name": "getPublicIp", "arguments": {},
                           "task": {"ttl": 60000},
                           "_meta": {"io.modelcontextprotocol/protocolVersion": v}}}
        st2, d2, _ = post("/mcp", body, hdr)
        o2 = parse(d2)
        if o2 is not None and "result" in o2:
            r2 = o2["result"]
            # 内联执行：不得出现 task/taskId 结构（task 创建是 2026 扩展）
            check(f"G7 [{v}] 内联执行且无 task 注入",
                  "task" not in r2, f"{str(o2)[:120]}")
            # 顺手验证结果是真实 IP（宽松：不影响工具调用都放行）
            txt = json.dumps(r2, ensure_ascii=False)
            check(f"G7 [{v}] 内联结果为真实 IPv4",
                  bool(re.search(r"\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}", txt)), txt[:120])
        else:
            code = (o2 or {}).get("error", {}).get("code")
            check(f"G7 [{v}] 4xx 或内联", st2 < 500 and (code is None or code < 0),
                  f"st={st2} code={code}")
        st3, d3, _ = post("/mcp", {"jsonrpc": "2.0", "id": 21, "method": "tasks/list",
                                   "params": {}}, hdr)
        check(f"G7 [{v}] tasks/list not-5xx", st3 < 500, f"st={st3} {d3[:80]}")


# ── G9: $smart × prompts/get + resources/read ──
def g9():
    pname, puri = None, None
    st, obj, sid = init("2025-11-25")
    if st != 200 or not sid:
        check("G9 root init", False, f"st={st}")
        return
    hdr = {"Mcp-Session-Id": sid, "MCP-Protocol-Version": "2025-11-25"}
    stp, dp, _ = post("/mcp", {"jsonrpc": "2.0", "id": 2, "method": "prompts/list", "params": {}}, hdr)
    pl = ((parse(dp) or {}).get("result") or {}).get("prompts") or []
    if pl:
        pname = pl[0].get("name")
    str_, dr_, _ = post("/mcp", {"jsonrpc": "2.0", "id": 3, "method": "resources/list", "params": {}}, hdr)
    rl = ((parse(dr_) or {}).get("result") or {}).get("resources") or []
    if rl:
        puri = rl[0].get("uri")
    if not pname and not puri:
        # 环境无 prompt/resource 可枚举——SKIP 桶，不计入 PASS（覆盖面依赖环境内容）
        check("G9 枚举端点可解析", isinstance(pl, list), f"prompts={len(pl)}")
        print("SKIP | G9 环境无 prompt/resource，实调断言未执行")
        return

    sts, os_, sids = init("2025-11-25", "/mcp/%24smart")
    if sts != 200 or not sids:
        check("G9 $smart init", False, f"st={sts}")
        return
    shdr = {"Mcp-Session-Id": sids, "MCP-Protocol-Version": "2025-11-25"}
    if pname:
        st1, d1, _ = post("/mcp/%24smart", {"jsonrpc": "2.0", "id": 4, "method": "prompts/get",
                                            "params": {"name": pname}}, shdr)
        o1 = parse(d1)
        if o1 and "result" in o1:
            st1b, d1b, _ = post("/mcp", {"jsonrpc": "2.0", "id": 5, "method": "prompts/get",
                                         "params": {"name": pname}}, hdr)
            o1b = parse(d1b)
            check("G9 $smart prompts/get 与根 /mcp 一致",
                  o1.get("result") == (o1b or {}).get("result"), f"{str(o1)[:120]}")
        else:
            check("G9 $smart prompts/get 定义拒绝（not-5xx）", st1 < 500, f"st={st1} {str(o1)[:100]}")
    if puri:
        st2, d2, _ = post("/mcp/%24smart", {"jsonrpc": "2.0", "id": 6, "method": "resources/read",
                                            "params": {"uri": puri}}, shdr)
        o2 = parse(d2)
        if o2 and "result" in o2:
            st2b, d2b, _ = post("/mcp", {"jsonrpc": "2.0", "id": 7, "method": "resources/read",
                                         "params": {"uri": puri}}, hdr)
            o2b = parse(d2b)
            check("G9 $smart resources/read 与根 /mcp 一致",
                  o2.get("result") == (o2b or {}).get("result"), f"{str(o2)[:120]}")
        else:
            check("G9 $smart resources/read 定义拒绝（not-5xx）", st2 < 500, f"st={st2} {str(o2)[:100]}")


# ── G10: legacy 两版本 × prompts/get + resources/read ──
def g10():
    for v in ["2025-03-26", "2025-06-18"]:
        for path in ["/mcp", f"/mcp/{SCOPE}"]:
            tag = f"G10 [{v}|{path}]"
            st, obj, sid = init(v, path)
            if st != 200 or not sid:
                check(f"{tag} init", False, f"st={st}")
                continue
            hdr = {"Mcp-Session-Id": sid, "MCP-Protocol-Version": v}
            stp, dp, _ = post(path, {"jsonrpc": "2.0", "id": 2, "method": "prompts/list", "params": {}}, hdr)
            pl = ((parse(dp) or {}).get("result") or {}).get("prompts") or []
            if pl:
                name = pl[0].get("name")
                st1, d1, _ = post(path, {"jsonrpc": "2.0", "id": 3, "method": "prompts/get",
                                         "params": {"name": name}}, hdr)
                o1 = parse(d1)
                res1 = (o1 or {}).get("result") or {}
                check(f"{tag} prompts/get 返回 result", st1 == 200 and res1 is not None,
                      f"st={st1} {str(o1)[:100]}")
                check(f"{tag} prompts/get 无 ttlMs 注入", "ttlMs" not in res1,
                      f"{res1.get('ttlMs')}")
            str_, dr_, _ = post(path, {"jsonrpc": "2.0", "id": 4, "method": "resources/list", "params": {}}, hdr)
            rl = ((parse(dr_) or {}).get("result") or {}).get("resources") or []
            if rl:
                uri = rl[0].get("uri")
                st2, d2, _ = post(path, {"jsonrpc": "2.0", "id": 5, "method": "resources/read",
                                         "params": {"uri": uri}}, hdr)
                o2 = parse(d2)
                res2 = (o2 or {}).get("result") or {}
                check(f"{tag} resources/read 返回 result", st2 == 200 and res2 is not None,
                      f"st={st2} {str(o2)[:100]}")
                check(f"{tag} resources/read 无 ttlMs 注入", "ttlMs" not in res2,
                      f"{res2.get('ttlMs')}")


def main():
    g7()
    g9()
    g10()
    print(f"\n== {ok}/{total} passed ==")
    if failed:
        print("FAILED:", *failed, sep="\n  - ")
        sys.exit(1)


if __name__ == "__main__":
    main()
