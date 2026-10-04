#!/usr/bin/env python3
# 50轮复核补充套件 B：5 协议版本 x 3 通道全矩阵 + 公网IP 真实调用 + 宽松升格 + 新特性穿透
import json, http.client, sys
from urllib.parse import quote

HOST, PORT = "localhost", 23333
PASS, FAIL = [], []
def check(name, ok, detail=""):
    (PASS if ok else FAIL).append(name)
    print(("PASS" if ok else "FAIL"), "|", name, ("| " + str(detail)[:120] if detail and not ok else ""))

def req(method, path, payload=None, headers=None, timeout=90):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
    if headers: h.update(headers)
    body = json.dumps(payload, ensure_ascii=False).encode("utf-8") if payload is not None else None
    c.request(method, path, body=body, headers=h)
    r = c.getresponse(); data = r.read().decode("utf-8", "replace")
    st = r.status; ct = r.getheader("content-type") or ""
    sid = r.getheader("mcp-session-id")
    c.close()
    obj = None
    if "json" in ct or data.strip().startswith("{"):
        try: obj = json.loads(data)
        except Exception: pass
    if obj is None:
        frames = []
        for l in data.split("\n"):
            if l.startswith("data: "):
                try: frames.append(json.loads(l[6:]))
                except Exception: pass
        obj = frames[-1] if frames else None
    return st, obj, ct, sid

def call_ip(session, prefix, version=None, base="/mcp"):
    name = f"{prefix}-getPublicIp" if prefix else "getPublicIp"
    headers = {"mcp-session-id": session} if session else {}
    if version: headers["MCP-Protocol-Version"] = version
    st, obj, ct, _ = req("POST", base, {"jsonrpc":"2.0","id":50,"method":"tools/call",
        "params":{"name":name,"arguments":{}}}, headers)
    if not (st == 200 and obj and "result" in obj): return False, f"st={st} {str(obj)[:100]}"
    res = obj["result"]
    if res.get("isError"): return False, "isError=true"
    content = res.get("content") or []
    txt = "".join(c.get("text","") for c in content if isinstance(c, dict))
    return (len(txt.strip()) > 0 and "error" not in txt.lower()), txt.strip()

VERSIONS = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"]
CHANNELS = [("root", "/mcp", "本机公网ip查询"), ("group", "/mcp/Test", "本机公网ip查询"), ("scope", "/mcp/" + quote("本机公网ip查询"), "")]

print("== A. legacy 4 版本 x 3 通道 ==")
for ver in VERSIONS:
    for ch_name, base, prefix in CHANNELS:
        st, obj, ct, sid = req("POST", base, {"jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"protocolVersion":ver,"capabilities":{},"clientInfo":{"name":"round50b","version":"1"}}})
        if not (st == 200 and obj and "result" in obj):
            check(f"A {ver}x{ch_name} initialize", False, f"st={st}"); continue
        got = obj["result"].get("protocolVersion")
        check(f"A {ver}x{ch_name} 版本回显一致", got == ver, f"req={ver} resp={got}")
        if not sid:
            check(f"A {ver}x{ch_name} session", False, "no session id"); continue
        st2, obj2, _, _ = req("POST", base, {"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}},
                              {"mcp-session-id":sid, "MCP-Protocol-Version":ver})
        tools = obj2["result"]["tools"] if (st2==200 and obj2 and "result" in obj2) else []
        check(f"A {ver}x{ch_name} tools/list", len(tools) > 0 and any("getPublicIp" in t["name"] for t in tools),
              f"n={len(tools)}")
        leak = "resultType" in json.dumps(obj2.get("result",{})) or "ttlMs" in json.dumps(obj2.get("result",{}))
        check(f"A {ver}x{ch_name} 无 modern 字段泄漏", not leak, "leaked")
        ok, txt = call_ip(sid, prefix, ver, base)
        check(f"A {ver}x{ch_name} 公网IP真实调用", ok, txt)

print("== B. 2026 modern x 3 通道 ==")
META2026 = {"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28",
    "io.modelcontextprotocol/clientInfo":{"name":"round50b","version":"1"},
    "io.modelcontextprotocol/clientCapabilities":{}}}
for ch_name, base, prefix in CHANNELS:
    st, obj, ct, sid = req("POST", base, {"jsonrpc":"2.0","id":1,"method":"server/discover","params":{}})
    if not (st == 200 and obj and "result" in obj):
        check(f"B {ch_name} discover", False, f"st={st}"); continue
    res = obj["result"]
    sv = res.get("supportedVersions") or res.get("_meta",{}).get("io.modelcontextprotocol/supportedVersions") or []
    check(f"B {ch_name} discover resultType+版本集", res.get("resultType")=="complete" and len(sv)>=4, f"sv={sv}")
    st2, obj2, ct2, _ = req("POST", base, {"jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":("本机公网ip查询-getPublicIp" if ch_name!="scope" else "getPublicIp"),"arguments":{}, **META2026}})
    ok = st2==200 and obj2 and "result" in obj2 and not obj2["result"].get("isError")
    check(f"B {ch_name} 2026 无状态公网IP真实调用", ok, f"st={st2} {str(obj2)[:100]}")

print("== C. 宽松升格（裸请求，无 session）==")
for ver in VERSIONS:
    hdr = {"MCP-Protocol-Version": ver} if ver else {}
    st, obj, ct, sid = req("POST", "/mcp", {"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}}, hdr)
    check(f"C 升格 {ver or '无头'} tools/list 放行", st==200 and obj and "result" in obj and len(obj.get("result",{}).get("tools",[]))>0, f"st={st}")
    check(f"C 升格 {ver or '无头'} 无 ttlMs（legacy 门控）", obj is not None and "ttlMs" not in json.dumps(obj.get("result",{})))
    name = "本机公网ip查询-getPublicIp"
    st2, obj2, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":name,"arguments":{}}}, hdr)
    ok = st2==200 and obj2 and "result" in obj2 and not obj2["result"].get("isError")
    txt = "".join(c.get("text","") for c in (obj2 or {}).get("result",{}).get("content",[]) if isinstance(c,dict))
    check(f"C 升格 {ver or '无头'} 公网IP真实调用", ok and txt.strip(), f"st={st2} {txt[:60]}")

print("== D. 版本头校验 ==")
st, obj, _, sid = req("POST", "/mcp", {"jsonrpc":"2.0","id":1,"method":"initialize",
    "params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"m","version":"1"}}})
st2, obj2, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}},
    {"mcp-session-id":sid, "MCP-Protocol-Version":"1999-01-01"})
check("D 错误版本头 宽松放行", st2 == 200 and obj2 and "result" in obj2, f"st={st2}")
st3, obj3, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}},
    {"mcp-session-id":sid, "MCP-Protocol-Version":"2025-06-18"})
check("D 正确版本头放行", st3 == 200 and obj3 and "result" in obj3, f"st={st3}")
bad = {"_meta":{"io.modelcontextprotocol/protocolVersion":"2099-01-01",
    "io.modelcontextprotocol/clientInfo":{"name":"m","version":"1"},
    "io.modelcontextprotocol/clientCapabilities":{}}}
st4, obj4, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":4,"method":"tools/list","params":bad})
err = (obj4 or {}).get("error", {})
check("D 2026 未知版本 -32022", err.get("code") == -32022, f"st={st4} err={err.get('code')}")

print("== E. 2026 新特性 ==")
st, obj, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":5,"method":"ping","params":META2026})
check("E1 2026 ping -32601", (obj or {}).get("error",{}).get("code") == -32601, f"{str(obj)[:100]}")
st, obj, _, sid = req("POST", "/mcp", {"jsonrpc":"2.0","id":1,"method":"initialize",
    "params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"m","version":"1"}}})
st, obj, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":6,"method":"ping","params":{}}, {"mcp-session-id":sid})
check("E1 legacy ping 空 result", st==200 and obj and obj.get("result") == {}, f"{str(obj)[:100]}")
for m in ["tools/list","prompts/list","resources/list"]:
    st, obj, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":7,"method":m,"params":META2026})
    res = (obj or {}).get("result", {})
    check(f"E2 {m} CacheableResult", res.get("resultType")=="complete" and res.get("ttlMs")==30000
          and res.get("cacheScope")=="private", f"{ {k:res.get(k) for k in ['resultType','ttlMs','cacheScope']} }")
st, obj, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":1,"method":"server/discover","params":{}})
res_d = (obj or {}).get("result", {})
caps = res_d.get("capabilities", {})
ext = res_d.get("extensions", {}) or caps.get("extensions", {})
check("E3 2026 核心 tasks 不在 / extensions 有",
      "tasks" not in caps and "io.modelcontextprotocol/tasks" in ext, f"caps={list(caps)} ext={list(ext)}")
META_TASKS = {"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28",
    "io.modelcontextprotocol/clientInfo":{"name":"round50b","version":"1"},
    "io.modelcontextprotocol/clientCapabilities":{"extensions":{"io.modelcontextprotocol/tasks":{}}}}}
st, obj, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":8,"method":"tools/call",
    "params":{"name":"本机公网ip查询-getPublicIp","arguments":{},"task":{"ttl":60000}, **META_TASKS}})
res = (obj or {}).get("result", {})
tid = res.get("taskId") if isinstance(res, dict) else None
if not tid:
    t = res.get("task") if isinstance(res, dict) else None
    tid = (t or {}).get("taskId") if isinstance(t, dict) else None
if tid:
    st, obj, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":9,"method":"tasks/result",
        "params":{"taskId":tid, **META2026}})
    check("E4 tasks/result 可查询", st==200 and obj and ("result" in obj or "error" in obj), f"st={st}")
else:
    check("E4 tasks 创建返回 taskId", False, f"res={str(res)[:120]}")
c = http.client.HTTPConnection(HOST, PORT, timeout=20)
c.request("POST", "/mcp", body=json.dumps({"jsonrpc":"2.0","id":10,"method":"subscriptions/listen",
    "params":{"notifications":{"toolsListChanged":True}, **META2026}}).encode(), 
    headers={"Content-Type":"application/json","Accept":"text/event-stream"})
r = c.getresponse()
st5 = r.status; ct5 = r.getheader("content-type") or ""
first = r.readline().decode("utf-8","replace")
second = r.readline().decode("utf-8","replace")
third = r.readline().decode("utf-8","replace")
c.close()
ack_ok = "acknowledged" in first+second+third and "subscriptionId" in first+second+third
check("E5 subscriptions/listen SSE ack", st5==200 and "text/event-stream" in ct5 and ack_ok,
      f"st={st5} ct={ct5} body={(first+second+third)[:120]}")

print(f"\n== {len(PASS)}/{len(PASS)+len(FAIL)} passed ==")
if FAIL:
    print("FAILED:"); [print(" -", f) for f in FAIL]
sys.exit(1 if FAIL else 0)
