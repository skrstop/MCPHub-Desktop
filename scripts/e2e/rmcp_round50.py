#!/usr/bin/env python3
# 50轮复核扩展场景：F2 升格全矩阵 + F3 严格模式 + F4 边界
import json, http.client, urllib.parse, sys, sqlite3, os

HOST, PORT = "localhost", 23333
PASS, FAIL = [], []
def check(n, ok, d=""):
    (PASS if ok else FAIL).append(n)
    print(("PASS" if ok else "FAIL"), "|", n, ("| " + str(d)[:95] if d and not ok else ""))

def raw(method, path, payload=None, headers=None, timeout=60):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {"Content-Type":"application/json","Accept":"application/json, text/event-stream"}
    if headers: h.update(headers)
    b = json.dumps(payload) if payload is not None else None
    c.request(method, path, body=b, headers=h)
    r = c.getresponse(); data=r.read().decode("utf-8","replace")
    st=r.status; sid=r.getheader("mcp-session-id"); c.close(); obj=None
    if data.strip().startswith("{"):
        try: obj=json.loads(data)
        except Exception: pass
    if obj is None:
        fr=[json.loads(l[6:]) for l in data.split("\n") if l.startswith("data: ") and l[6:].strip()]
        obj=fr[-1] if fr else None
    return st, sid, obj, data

def req(method, path, payload=None, headers=None, timeout=60):
    return raw(method, path, payload, headers, timeout)

IP_SCOPE=f"/mcp/{urllib.parse.quote('本机公网ip查询')}"
TEST_SCOPE=f"/mcp/{urllib.parse.quote('Test')}"
PV="io.modelcontextprotocol/protocolVersion"

# ══ F2: 升格 × 3 头形态 × 3 通道（root/分组/单服务器）× IP ══
print("══ F2 升格×版本×通道全矩阵 ══")
for hdr_v in [None,"2025-11-25","2024-11-05"]:
    for path, tool in [("/mcp","本机公网ip查询-getPublicIp"),(TEST_SCOPE,"本机公网ip查询-getPublicIp"),(IP_SCOPE,"getPublicIp")]:
        h = {} if hdr_v is None else {"MCP-Protocol-Version":hdr_v}
        tag = f"升格[{hdr_v or '无头'}]{path}"
        st,_,obj,_ = req("POST",path,{"jsonrpc":"2.0","id":20,"method":"tools/list","params":{}},h)
        tools=obj.get("result",{}).get("tools",[]) if obj else []
        hit = any(t["name"]==tool or t["name"].endswith("getPublicIp") for t in tools)
        check(f"F2 {tag} tools/list", st==200 and hit, f"st={st} n={len(tools)}")
        st,_,obj,_ = req("POST",path,{"jsonrpc":"2.0","id":21,"method":"tools/call","params":{"name":tool,"arguments":{}}},h)
        txt="".join(str(c.get("text","")) for c in obj.get("result",{}).get("content",[])) if obj else ""
        check(f"F2 {tag} IP调用", st==200 and obj and obj.get("result",{}).get("isError") is False and len(txt)>0, f"st={st} {txt[:50]}")

# ══ F3: 严格模式 × 每版本违规拒绝 + 正常放行 ══
print("══ F3 严格模式 ══")
DB=os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")
def set_strict(on):
    con=sqlite3.connect(DB)
    cfg=json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
    cfg.setdefault("mcp",{})["strictValidation"]=on
    con.execute("UPDATE system_config SET config_json=? WHERE rowid=1",(json.dumps(cfg,ensure_ascii=False),))
    con.commit(); con.close()
for v in ["2024-11-05","2025-03-26","2025-11-25"]:
    st,_,obj,_ = req("POST","/mcp",{"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":v,"capabilities":{},"clientInfo":{"name":"m","version":"1"}}},
        {"Accept":"application/json"})
    check(f"F3 严格[{v}] 正常请求仍通(Accept缺陷被拒是S2场景)", st==200, f"st={st}")
set_strict(True)
try:
    st,_,obj,_ = req("POST","/mcp",{"jsonrpc":"2.0","id":30,"method":"tools/list","params":{"_meta":{PV:"2026-07-28"}}},
        {"MCP-Protocol-Version":"2026-07-28"})
    check("F3 严格: 2026缺件拒绝", obj and "error" in obj, f"st={st}")
    st,_,obj,_ = req("POST","/mcp",{"jsonrpc":"2.0","id":31,"method":"tools/list","params":{}},
        {"MCP-Protocol-Version":"2024-11-05"})
    check("F3 严格: 裸请求不升格(拒绝/错误)", st!=200 or (obj and "error" in obj), f"st={st}")
finally:
    set_strict(False)

# ══ F4: 边界场景 ══
print("══ F4 边界 ══")
# F4-1 notifications/initialized（无 id）宽松放行 202
st,_,_,_ = req("POST","/mcp",{"jsonrpc":"2.0","method":"notifications/initialized","params":{}},
    {"Mcp-Session-Id":"nonexistent"})
check("F4-1 无session通知不崩(202/202/404均可)", st in (202,200,404,400), f"st={st}")
# F4-2 非法 JSON body → 4xx 不挂
c=http.client.HTTPConnection(HOST,PORT,timeout=10)
c.request("POST","/mcp",body="{not json",headers={"Content-Type":"application/json","Accept":"application/json, text/event-stream"})
r=c.getresponse(); bad=r.status; r.read(); c.close()
check("F4-2 非法JSON不挂", 400<=bad<500, f"st={bad}")
# F4-3 未知方法/工具（严格+宽松都要对）
st,_,obj,_ = req("POST","/mcp",{"jsonrpc":"2.0","id":40,"method":"no/such","params":{}})
check("F4-3 未知方法 -32601", obj and obj.get("error",{}).get("code")==-32601, str(obj)[:70])
# F4-4 unicode 工具名暴露 + 调用（宽松升格 root 裸调用中文前缀名）
st,_,obj,_ = req("POST","/mcp",{"jsonrpc":"2.0","id":41,"method":"tools/call",
    "params":{"name":"本机公网ip查询-getPublicIp","arguments":{}}})
check("F4-4 中文工具名裸调用(宽松升格)", st==200 and obj and obj.get("result",{}).get("isError") is False, str(obj)[:70])
# F4-5 $smart scope 可响应（错误或结果，不挂死）
st,_,obj,_ = req("POST","/mcp/$smart",{"jsonrpc":"2.0","id":42,"method":"initialize",
    "params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"m","version":"1"}}})
check("F4-5 $smart initialize 可响应", st in (200,400,404), f"st={st}")
# F4-6 DELETE 无 session 头 → 400（规范）
st,_,_,_ = raw("DELETE","/mcp")
check("F4-6 DELETE 无头 400", st==400, f"st={st}")
# F4-7 DELETE 存在会话 → 202/200
st,sid,_,_ = req("POST","/mcp",{"jsonrpc":"2.0","id":1,"method":"initialize",
    "params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"m","version":"1"}}})
st2,_,_,_ = raw("DELETE","/mcp",None,{"Mcp-Session-Id":sid})
check("F4-7 DELETE 会话 202/200", st2 in (200,202), f"st={st2}")
# F4-8 RAG builtin 工具经 root 暴露
st,_,obj,_ = req("POST","/mcp",{"jsonrpc":"2.0","id":43,"method":"tools/list","params":{}})
names=[t["name"] for t in obj.get("result",{}).get("tools",[])] if obj else []
check("F4-8 root 工具数>0 且含前缀服务", len(names)>0 and any("-" in n for n in names), f"n={len(names)}")
# F4-9 版本回显：升格响应 id/method 对齐
st,_,obj,_ = req("POST","/mcp",{"jsonrpc":"2.0","id":"str-id-9","method":"tools/list","params":{}})
check("F4-9 升格保留 string id", obj and obj.get("id")=="str-id-9", str(obj)[:70])
# F4-10 params.arguments 非法类型 → 错误响应不挂
st,_,obj,_ = req("POST","/mcp",{"jsonrpc":"2.0","id":44,"method":"tools/call",
    "params":{"name":"nonexistent-x","arguments":"not-an-object"}})
check("F4-10 非法arguments不挂", st==200 or (obj and "error" in obj) or st==400, f"st={st}")

print(f"\n== {len(PASS)}/{len(PASS)+len(FAIL)} passed ==")
if FAIL:
    print("FAILED:", *FAIL, sep="\n  - "); sys.exit(1)
