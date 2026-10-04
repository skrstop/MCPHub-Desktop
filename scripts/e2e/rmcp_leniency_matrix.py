#!/usr/bin/env python3
# 宽松模式 × 版本 × 通道：缺陷请求放行后 版本回显一致 + 门控不污染
import json, http.client, urllib.parse, sys

HOST, PORT = "localhost", 23333
PASS, FAIL = [], []
def check(name, ok, detail=""):
    (PASS if ok else FAIL).append(name)
    print(("PASS" if ok else "FAIL"), "|", name, ("| " + str(detail)[:100] if detail and not ok else ""))

def req(method, path, payload=None, headers=None, timeout=60):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {"Content-Type": "application/json"}   # 故意不带 Accept：测宽松补全
    if headers: h.update(headers)
    body = json.dumps(payload) if payload is not None else None
    c.request(method, path, body=body, headers=h)
    r = c.getresponse(); data = r.read().decode("utf-8","replace")
    st=r.status; sid=r.getheader("mcp-session-id"); ct=r.getheader("content-type") or ""
    c.close(); obj=None
    if data.strip().startswith("{"):
        try: obj=json.loads(data)
        except Exception: pass
    if obj is None:
        fr=[json.loads(l[6:]) for l in data.split("\n") if l.startswith("data: ") and l[6:].strip()]
        obj=fr[-1] if fr else None
    return st, sid, obj, ct

IP_SCOPE = f"/mcp/{urllib.parse.quote('本机公网ip查询')}"
PV = "io.modelcontextprotocol/protocolVersion"
CLIENT_META = {"io.modelcontextprotocol/clientInfo":{"name":"m","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}}

# ── R2: 宽松 × 每 legacy 版本：无 Accept + 无版本头的 initialize 放行 + 版本回显一致 ──
for v in ["2024-11-05","2025-03-26","2025-11-25"]:
    st, sid, obj, ct = req("POST","/mcp",{"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":v,"capabilities":{},"clientInfo":{"name":"l","version":"1"}}})
    check(f"宽松[{v}] 无Accept initialize 放行", st==200 and obj and "result" in obj, f"st={st}")
    check(f"宽松[{v}] 版本回显一致", obj and obj["result"].get("protocolVersion")==v, str(obj)[:80])
    # 带上 session 后 tools/list（无 Accept）放行 + legacy 无 ttlMs（门控不污染）
    st,_,obj,_ = req("POST","/mcp",{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}},
                     {"Mcp-Session-Id":sid, "MCP-Protocol-Version":v})
    tools = obj.get("result",{}).get("tools",[]) if obj else []
    check(f"宽松[{v}] tools/list 放行(无Accept)", st==200 and len(tools)>0, f"n={len(tools)}")
    check(f"宽松[{v}] legacy 无 ttlMs(门控不污染)", obj and "ttlMs" not in obj.get("result",{}), str(obj)[:80])
    check(f"宽松[{v}] IP 工具暴露", any(t["name"].endswith("getPublicIp") for t in tools))
    # 公网IP 真实调用
    st,_,obj,_ = req("POST","/mcp",{"jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":next((t["name"] for t in tools if t["name"].endswith("getPublicIp"))),"arguments":{}}},
        {"Mcp-Session-Id":sid,"MCP-Protocol-Version":v})
    txt="".join(str(c.get("text","")) for c in obj.get("result",{}).get("content",[])) if obj else ""
    check(f"宽松[{v}] 公网IP 真实调用", st==200 and obj and obj.get("result",{}).get("isError") is False and len(txt)>0, txt[:70])
    req("DELETE","/mcp",None,{"Mcp-Session-Id":sid})

# ── R3: 宽松 × 2026 modern 缺陷请求（缺 client 元数据+缺 Mcp-Method）× 单服务器通道 ──
st,_,obj,_ = req("POST", IP_SCOPE,
    {"jsonrpc":"2.0","id":9,"method":"tools/list","params":{"_meta":{PV:"2026-07-28"}}},
    {"MCP-Protocol-Version":"2026-07-28"})
ok = st==200 and obj and len(obj.get("result",{}).get("tools",[]))>0
check("宽松[2026] 单服务器 缺件 tools/list 放行", ok, f"st={st} {str(obj)[:80]}")
if ok:
    check("宽松[2026] 单服务器 CacheableResult(ttlMs)", "ttlMs" in obj["result"])
    # modern IP 真实调用（裸名，中间件注入 Mcp-Method/Mcp-Name）
    st,_,obj,_ = req("POST", IP_SCOPE,
        {"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"getPublicIp","arguments":{},"_meta":{PV:"2026-07-28"}}},
        {"MCP-Protocol-Version":"2026-07-28"})
    txt="".join(str(c.get("text","")) for c in obj.get("result",{}).get("content",[])) if obj else ""
    check("宽松[2026] 单服务器 modern 公网IP 调用", st==200 and obj and obj.get("result",{}).get("isError") is False, f"st={st} {txt[:60]}")

# ── R4: 宽松 × 分组通道（/mcp/Test 缺 Accept initialize）──
st,_,obj,_ = req("POST", "/mcp/Test", {"jsonrpc":"2.0","id":1,"method":"initialize",
    "params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"l","version":"1"}}})
check("宽松[2025-11] /mcp/Test 无Accept 放行", st==200 and obj and "result" in obj, f"st={st}")
check("宽松[2025-11] /mcp/Test 版本回显一致", obj and obj["result"].get("protocolVersion")=="2025-11-25")

# ── R5: 裸请求升格（所有版本无 session 无 initialize 直达工具调用）──
print("── 裸请求升格（宽松：无会话直达）──")
for hdr_v in [None, "2025-11-25", "2024-11-05"]:
    h = {} if hdr_v is None else {"MCP-Protocol-Version": hdr_v}
    label = hdr_v or "无版本头"
    st,_,obj,_ = req("POST","/mcp",{"jsonrpc":"2.0","id":20,"method":"tools/list","params":{}},h)
    tools = obj.get("result",{}).get("tools",[]) if obj else []
    check(f"升格[{label}] 裸 tools/list 直达", st==200 and len(tools)>0, f"st={st} {str(obj)[:70]}")
    check(f"升格[{label}] legacy 门控(无ttlMs)", obj and "ttlMs" not in obj.get("result",{}), str(obj)[:70])
    st,_,obj,_ = req("POST","/mcp",{"jsonrpc":"2.0","id":21,"method":"tools/call",
        "params":{"name":"本机公网ip查询-getPublicIp","arguments":{}}},h)
    txt="".join(str(c.get("text","")) for c in obj.get("result",{}).get("content",[])) if obj else ""
    check(f"升格[{label}] 裸公网IP 真实调用", st==200 and obj and obj.get("result",{}).get("isError") is False, f"st={st} {txt[:60]}")
# 单服务器 scope 裸调用
st,_,obj,_ = req("POST", IP_SCOPE, {"jsonrpc":"2.0","id":22,"method":"tools/call","params":{"name":"getPublicIp","arguments":{}}})
txt="".join(str(c.get("text","")) for c in obj.get("result",{}).get("content",[])) if obj else ""
check("升格[单服务器] 裸 getPublicIp 调用", st==200 and obj and obj.get("result",{}).get("isError") is False, f"st={st} {txt[:60]}")
# 有会话的请求不被升格影响（仍走会话路径）
st, sid, obj, _ = req("POST","/mcp",{"jsonrpc":"2.0","id":1,"method":"initialize",
    "params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"m","version":"1"}}})
check("升格: 正常会话流不受影响", st==200 and sid is not None)

print(f"\n== {len(PASS)}/{len(PASS)+len(FAIL)} passed ==")
if FAIL:
    print("FAILED:", *FAIL, sep="\n  - "); sys.exit(1)
