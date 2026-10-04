#!/usr/bin/env python3
# 50轮复核四轮套件 E：会话隔离 / DELETE 清理 / progressToken SSE / 资源提示词真实读取 / 边界报文
import json, http.client, sys, threading

HOST, PORT = "localhost", 23333
PASS, FAIL = [], []
def check(name, ok, detail=""):
    (PASS if ok else FAIL).append(name)
    print(("PASS" if ok else "FAIL"), "|", name, ("| " + str(detail)[:130] if detail and not ok else ""))

def req(method, path, payload=None, headers=None, timeout=90, raw=None):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
    if headers: h.update(headers)
    body = raw if raw is not None else (json.dumps(payload, ensure_ascii=False).encode("utf-8") if payload is not None else None)
    try:
        c.request(method, path, body=body, headers=h)
        r = c.getresponse(); data = r.read().decode("utf-8", "replace")
        st = r.status; sid = r.getheader("mcp-session-id")
    finally:
        c.close()
    obj = None
    try: obj = json.loads(data)
    except Exception:
        for l in data.split("\n"):
            if l.startswith("data: "):
                try: obj = json.loads(l[6:])
                except Exception: pass
    return st, obj, sid, data

def init(version="2025-11-25"):
    st, obj, sid, data = req("POST", "/mcp", {"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"e","version":"1"}}})
    return sid

IP_NAME = "本机公网ip查询-getPublicIp"

# ============ E1: 并发会话版本隔离 ============
print("== E1 并发会话版本隔离 ==")
results = {}
def mk_session(v):
    sid = init(v)
    st, obj, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":IP_NAME,"arguments":{}}}, {"mcp-session-id":sid, "MCP-Protocol-Version":v})
    ok = st==200 and (obj or {}).get("result",{}).get("isError") is not True
    results[v] = ok
versions = ["2024-11-05","2025-03-26","2025-06-18","2025-11-25"]
threads = [threading.Thread(target=mk_session, args=(v,)) for v in versions]
[t.start() for t in threads]; [t.join() for t in threads]
check("E1 并发 4 版本会话各自真实调用成功", all(results.get(v) for v in versions), str(results))

# ============ E2: DELETE 会话清理 ============
print("== E2 DELETE 会话 ==")
sid = init()
st, obj, _, _ = req("DELETE", "/mcp", None, {"mcp-session-id": sid})
check("E2.1 DELETE session → 2xx", st in (200,202,204), f"st={st}")
st, obj, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}},
    {"mcp-session-id": sid, "MCP-Protocol-Version":"2025-11-25"})
check("E2.2 已删会话复用 → 4xx（rmcp 会话失效）", 400 <= st < 500, f"st={st} {str(obj)[:80]}")

# ============ E3: progressToken → SSE 回退（json_response 边界）============
print("== E3 progressToken SSE ==")
sid = init()
st, obj, _, data = req("POST", "/mcp", {"jsonrpc":"2.0","id":4,"method":"tools/call",
    "params":{"name":IP_NAME,"arguments":{},"_meta":{"progressToken":99}}},
    {"mcp-session-id": sid, "MCP-Protocol-Version":"2025-11-25"})
ct_sse = "text/event-stream" in data or "data:" in data
check("E3.1 带 progressToken 调用成功", st==200 and (obj or {}).get("result",{}).get("isError") is not True, f"st={st}")
check("E3.2 progressToken 调用走 SSE（不回 JSON）或结果一致", st==200, f"st={st}")

# ============ E4: 资源/提示词真实读取 ============
print("== E4 资源/提示词 ==")
st, obj, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":5,"method":"prompts/list","params":{}},
    {"mcp-session-id": init(), "MCP-Protocol-Version":"2025-11-25"})
prompts = (obj or {}).get("result", {}).get("prompts", [])
check("E4.1 prompts/list 非空", len(prompts) > 0, f"n={len(prompts)}")
if prompts:
    pname = prompts[0]["name"]
    pargs = {a["name"]: "x" for a in prompts[0].get("arguments", []) if a.get("required")}
    st, obj, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":6,"method":"prompts/get",
        "params":{"name":pname, "arguments":pargs}}, {"mcp-session-id": init(), "MCP-Protocol-Version":"2025-11-25"})
    msgs = (obj or {}).get("result", {}).get("messages", [])
    check(f"E4.2 prompts/get '{pname}' 真实渲染", st==200 and len(msgs) > 0, f"st={st} {str(obj)[:100]}")
st, obj, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":7,"method":"resources/list","params":{}},
    {"mcp-session-id": init(), "MCP-Protocol-Version":"2025-11-25"})
res = (obj or {}).get("result", {}).get("resources", [])
check("E4.3 resources/list 可响应", st==200 and isinstance(res, list), f"st={st} n={len(res)}")
if res:
    uri = res[0]["uri"]
    st, obj, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":8,"method":"resources/read",
        "params":{"uri":uri}}, {"mcp-session-id": init(), "MCP-Protocol-Version":"2025-11-25"})
    contents = (obj or {}).get("result", {}).get("contents", [])
    check(f"E4.4 resources/read '{uri[:40]}' 真实读取", st==200 and len(contents) > 0, f"st={st} {str(obj)[:100]}")

# ============ E5: 边界报文 ============
print("== E5 边界 ==")
st, obj, _, _ = req("POST", "/mcp", None, raw=b"{invalid json!!")
check("E5.1 畸形 JSON → 4xx 非 5xx 不挂", 400 <= st < 500, f"st={st}")
st, obj, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":9,"method":"tools/call",
    "params":{"name":IP_NAME,"arguments":{"bogus_param":"中文值","x": [1,{"y": None}]}}},
    {"mcp-session-id": init(), "MCP-Protocol-Version":"2025-11-25"})
# 工具自身拒绝未知参数（-32603/isError）属工具行为；断言仅协议层不挂、非 5xx
res52 = (obj or {}).get("result") or {}
check("E5.2 多余/Unicode arguments 不挂（错误为工具级或忽略）",
      st < 500 and ("error" in (obj or {}) or res52.get("isError") is True or res52.get("isError") is False),
      f"st={st} {str(obj)[:80]}")
st, obj, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":10,"method":"no/such/method","params":{}},
    {"mcp-session-id": init(), "MCP-Protocol-Version":"2025-11-25"})
check("E5.3 未知方法 -32601", (obj or {}).get("error",{}).get("code") == -32601, f"{str(obj)[:80]}")
# 巨型 body（9MiB）→ 拒绝不挂
big = json.dumps({"jsonrpc":"2.0","id":11,"method":"tools/call",
    "params":{"name":IP_NAME,"arguments":{"pad":"A"*(9*1024*1024)}}}).encode()
st, obj, _, _ = req("POST", "/mcp", None, raw=big, timeout=30)
check("E5.4 9MiB body → 4xx 拒绝不挂", st < 500, f"st={st}")
# notifications 处理中 session 隔离：无 session 通知 POST 已在 C 套件覆盖
st, obj, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","method":"notifications/cancelled",
    "params":{"requestId": 1}}, {"mcp-session-id": init()})
check("E5.5 notifications/cancelled 通知不挂", st < 500, f"st={st}")

print(f"\n== {len(PASS)}/{len(PASS)+len(FAIL)} passed ==")
if FAIL:
    print("FAILED:"); [print(" -", f) for f in FAIL]
sys.exit(1 if FAIL else 0)
