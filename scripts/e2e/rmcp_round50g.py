#!/usr/bin/env python3
# 套件 G：本轮修复回归（R1-1 伪造任务头非JSON剥离 / R7-4 任务终态通知无回归 / R8-1 _meta:{} 裸请求升格）
import json, http.client, sys

HOST, PORT = "localhost", 23333
PASS, FAIL = [], []
def check(name, ok, detail=""):
    (PASS if ok else FAIL).append(name)
    print(("PASS" if ok else "FAIL"), "|", name, ("| " + str(detail)[:160] if detail and not ok else ""))

def req(method, path, payload=None, headers=None, timeout=60, raw_body=None, raw_ct=None):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {"Accept": "application/json, text/event-stream"}
    if raw_ct: h["Content-Type"] = raw_ct
    elif payload is not None: h["Content-Type"] = "application/json"
    if headers: h.update(headers)
    body = raw_body if raw_body is not None else (json.dumps(payload) if payload is not None else None)
    c.request(method, path, body=body, headers=h)
    r = c.getresponse(); data = r.read().decode("utf-8", "replace")
    sid = r.getheader("mcp-session-id"); st = r.status
    c.close()
    obj = None
    if data.strip().startswith("{"):
        try: obj = json.loads(data)
        except Exception: pass
    if obj is None:
        frames = []
        for line in data.split("\n"):
            if line.startswith("data: "):
                try: frames.append(json.loads(line[6:]))
                except Exception: pass
        obj = frames[-1] if frames else None
    return st, obj, sid, data


import re as _re
def ipv4(text):
    return bool(_re.search(r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b", text or ""))

IP_TOOL = "本机公网ip查询-getPublicIp"

def call_ip(headers=None, scope="/mcp"):
    """无会话升格 tools/call 公网IP 真实调用，返回 (status, obj)"""
    st, obj, sid, _ = req("POST", scope, {
        "jsonrpc": "2.0", "id": 901, "method": "tools/call",
        "params": {"name": IP_TOOL, "arguments": {}},
    }, headers=headers)
    return st, obj

def extract_text(obj):
    try:
        cont = obj["result"]["content"] if "result" in obj else obj["result"]["result"]["content"]
    except Exception:
        return ""
    txt = ""
    for c in cont if isinstance(cont, list) else []:
        if isinstance(c, dict) and c.get("type") == "text": txt += c.get("text", "")
    return txt

# ── G1: 伪造任务头 + 非 JSON body → 不进任务模式（R1-1） ──────────────
st, obj, _, _ = req("POST", "/mcp", raw_body=b"this is not json",
                    headers={"x-mcphub-task-requested": '{"ttl":60000}'})
check("G1.1 非 JSON body + 伪造任务头 → 4xx 解析错误（非 5xx / 非 202）",
      400 <= st < 500 and not (obj and isinstance(obj.get("result"), dict) and obj["result"].get("resultType") == "task"),
      f"status={st} obj={obj}")

# 合法 JSON 但无 params.task + 伪造头 → 同步执行（既有 C 套件断言补充公网IP版）
st, obj, _, _ = req("POST", "/mcp", {
    "jsonrpc": "2.0", "id": 902, "method": "tools/call",
    "params": {"name": IP_TOOL, "arguments": {}},
}, headers={"x-mcphub-task-requested": '{"ttl":60000}'})
ok_sync = st == 200 and obj and "result" in obj and obj["result"].get("resultType") != "task"
check("G1.2 伪造头 + 无 body task → 同步执行公网IP 真实调用", ok_sync and bool(ipv4(extract_text(obj))), f"status={st} text={extract_text(obj)[:80]}")

# ── G2: _meta:{} 裸请求升格（R8-1） ─────────────────────────────────
st, obj, _, _ = req("POST", "/mcp", {
    "jsonrpc": "2.0", "id": 903, "method": "tools/list",
    "params": {"_meta": {}},
})
check("G2.1 _meta:{} 裸 tools/list 升格放行", st == 200 and obj and "result" in obj, f"status={st}")
if obj and "result" in obj:
    names = [t.get("name") for t in obj["result"].get("tools", [])]
    check("G2.2 升格后工具面完整（含 ip 工具）", any("getPublicIp" in n for n in names if n), f"{len(names)} tools")
    check("G2.3 legacy 门控：无 ttlMs 泄漏", "ttlMs" not in obj["result"], obj["result"].get("ttlMs"))

# _meta 带 progressToken 的裸请求仍升格且不挂
st, obj, _, _ = req("POST", "/mcp", {
    "jsonrpc": "2.0", "id": 904, "method": "tools/list",
    "params": {"_meta": {"progressToken": 7}},
}, timeout=30)
check("G2.4 _meta:{progressToken} 裸请求升格不挂", st == 200 and obj is not None and isinstance((obj or {}).get("result", {}).get("tools"), list), f"status={st}")

# ── G3: _meta:{} 裸公网IP 真实调用 ──────────────────────────────────
st, obj, _, _ = req("POST", "/mcp", {
    "jsonrpc": "2.0", "id": 905, "method": "tools/call",
    "params": {"name": IP_TOOL, "arguments": {}, "_meta": {}},
})
check("G3.1 _meta:{} 裸 tools/call 公网IP 真实调用", st == 200 and obj and obj.get("result", {}).get("isError") is False, f"status={st} obj={str(obj)[:200]}")
check("G3.2 返回真实 IP 内容", extract_text(obj).strip() != "")

META26 = {"_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28",
          "io.modelcontextprotocol/clientInfo": {"name": "g-suite", "version": "1"},
          "io.modelcontextprotocol/clientCapabilities": {"extensions": {"io.modelcontextprotocol/tasks": {}}}}}
HDR26 = {"MCP-Protocol-Version": "2026-07-28"}

# ── G4: 任务生命周期无回归（R7-4 通知改动不破坏既有语义） ────────────
st, obj, _, _ = req("POST", "/mcp", {
    "jsonrpc": "2.0", "id": 906, "method": "tools/call",
    "params": {"name": IP_TOOL, "arguments": {}, "task": {"ttl": 60000}, **META26},
}, headers=HDR26)
res = (obj or {}).get("result", {})
tid = res.get("taskId") if isinstance(res, dict) else None  # CreateTaskResult 序列化为扁平 shape
check("G4.1 task 创建（resultType=task + taskId）", st == 200 and res.get("resultType") == "task" and tid, f"status={st} res={str(res)[:160]}")
if tid:
    # 轮询 tasks/result 到终态
    final = None
    for _ in range(20):
        st2, obj2, _, _ = req("POST", "/mcp", {
            "jsonrpc": "2.0", "id": 907, "method": "tasks/result",
            "params": {"taskId": tid, **META26},
        }, headers=HDR26)
        r2 = (obj2 or {}).get("result", {})
        if isinstance(r2, dict) and r2.get("resultType") == "complete":
            final = r2; break
        import time; time.sleep(0.5)
    check("G4.2 tasks/result 终态含公网IP 内容", final is not None and extract_text({"result": final}).strip() != "")

    # tasks/cancel 已终态任务 → -32602
    st3, obj3, _, _ = req("POST", "/mcp", {
        "jsonrpc": "2.0", "id": 908, "method": "tasks/cancel",
        "params": {"taskId": tid, **META26},
    }, headers=HDR26)
    check("G4.3 终态任务 cancel → -32602", (obj3 or {}).get("error", {}).get("code") == -32602, str(obj3)[:120])

# ── G5: subscriptions/listen 事件总线通路（R7-4 publish 无回归） ─────
# SSE 流不结束：用裸连接限量读行（仿套件 E5）
c = http.client.HTTPConnection(HOST, PORT, timeout=6)
listen_body = json.dumps({
    "jsonrpc": "2.0", "id": 909, "method": "subscriptions/listen",
    "params": {"notifications": {"toolsListChanged": True},
               "_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28",
                         "io.modelcontextprotocol/clientInfo": {"name": "g-suite", "version": "1"},
                         "io.modelcontextprotocol/clientCapabilities": {}}},
}).encode()
c.request("POST", "/mcp", body=listen_body,
          headers={"Content-Type": "application/json", "Accept": "text/event-stream",
                   "MCP-Protocol-Version": "2026-07-28"})
r = c.getresponse()
st5 = r.status
frames = ""
try:
    for _ in range(6):
        line = r.readline().decode("utf-8", "replace")
        frames += line
        if "acknowledged" in frames:
            break
except Exception:
    pass  # 流保持打开是规范行为；读到 ack 即可
c.close()
check("G5.1 subscriptions/listen SSE ack 建流（subscriptionId=请求 id）",
      st5 == 200 and "acknowledged" in frames and "909" in frames, f"status={st5} frames={frames[:160]}")

print(f"\n== {len(PASS)}/{len(PASS)+len(FAIL)} passed ==")
sys.exit(1 if FAIL else 0)
