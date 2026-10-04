#!/usr/bin/env python3
# 50轮复核三轮套件 D：REST 端点 / $smart / GET SSE / legacy core tasks / bearer 矩阵
# 用法: python3 rmcp_round50d.py   （bearer 段自动开关 routing.enableBearerAuth 并还原）
import json, http.client, sys, sqlite3, os, time

HOST, PORT = "localhost", 23333
IP_SERVER = "本机公网ip查询"
IP_TOOL = "getPublicIp"
PASS, FAIL = [], []
def check(name, ok, detail=""):
    (PASS if ok else FAIL).append(name)
    print(("PASS" if ok else "FAIL"), "|", name, ("| " + str(detail)[:130] if detail and not ok else ""))

def http_req(method, path, payload=None, headers=None, timeout=90, raw_body=None):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {"Content-Type": "application/json"}
    if headers: h.update(headers)
    body = raw_body if raw_body is not None else (json.dumps(payload, ensure_ascii=False).encode("utf-8") if payload is not None else None)
    c.request(method, path, body=body, headers=h)
    r = c.getresponse(); data = r.read().decode("utf-8", "replace")
    st = r.status; ct = r.getheader("content-type") or ""
    c.close()
    obj = None
    try: obj = json.loads(data)
    except Exception:
        for l in data.split("\n"):
            if l.startswith("data: "):
                try: obj = json.loads(l[6:])
                except Exception: pass
    return st, obj, ct, data

META = {"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28",
    "io.modelcontextprotocol/clientInfo":{"name":"round50d","version":"1"},
    "io.modelcontextprotocol/clientCapabilities":{}}}

def mcp_session(version="2025-11-25", base="/mcp"):
    st, obj, ct, data = http_req("POST", base, {"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"d","version":"1"}}})
    c = http.client.HTTPConnection(HOST, PORT, timeout=90)
    c.request("POST", base, body=json.dumps({"jsonrpc":"2.0","id":0,"method":"initialize",
        "params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"d","version":"1"}}}).encode(),
        headers={"Content-Type":"application/json","Accept":"application/json, text/event-stream"})
    r = c.getresponse(); r.read(); sid = r.getheader("mcp-session-id"); c.close()
    return sid

def mcp_call(sid, name, base="/mcp", version="2025-11-25", extra=None):
    h = {"MCP-Protocol-Version": version}
    if sid: h["mcp-session-id"] = sid
    if extra: h.update(extra)
    st, obj, ct, data = http_req("POST", base, {"jsonrpc":"2.0","id":9,"method":"tools/call",
        "params":{"name":name,"arguments":{}}}, h)
    return st, obj

def rest_ok(st, obj):
    """REST call 返回裸 content 数组"""
    if st != 200: return False
    if isinstance(obj, list):
        txt = "".join(c.get("text","") for c in obj if isinstance(c, dict))
        return len(txt.strip()) > 3
    if isinstance(obj, dict):
        arr = obj.get("content") or obj.get("result")
        if isinstance(arr, list):
            txt = "".join(c.get("text","") for c in arr if isinstance(c, dict))
            return len(txt.strip()) > 3
    return False

def call_ok(st, obj):
    if not (st==200 and obj and "result" in obj): return False
    res = obj["result"]
    if res.get("isError"): return False
    txt = "".join(c.get("text","") for c in res.get("content",[]) if isinstance(c,dict))
    return len(txt.strip()) > 3

# ============ D1: REST 端点真实调用 ============
print("== D1 REST 端点 ==")
from urllib.parse import quote
srv = quote(IP_SERVER)
st, obj, ct, data = http_req("GET", f"/rest/{srv}/tools")
tools = obj if isinstance(obj, list) else (obj or {}).get("tools", [])
check("D1.1 REST /rest/{server}/tools 列表含 ip 工具",
      st==200 and any(isinstance(t,dict) and t.get("name")==IP_TOOL for t in tools), f"st={st} n={len(tools)}")
st, obj, ct, data = http_req("POST", f"/rest/{srv}/call", {"tool":IP_TOOL,"arguments":{}})
check("D1.2 REST /rest/{server}/call 真实公网IP调用", rest_ok(st, obj), f"st={st} {str(obj)[:120]}")
st, obj, ct, data = http_req("GET", f"/rest/group/Test/tools")
gt = obj if isinstance(obj, list) else (obj or {}).get("tools", [])
check("D1.3 REST 分组 tools", st==200 and len(gt)>0, f"st={st} n={len(gt)}")
st, obj, ct, data = http_req("POST", "/rest/group/Test/call", {"tool":f"{IP_SERVER}-{IP_TOOL}","arguments":{}})
if not rest_ok(st, obj):
    # 分组内可能按裸名解析
    st, obj, ct, data = http_req("POST", "/rest/group/Test/call", {"tool":IP_TOOL,"arguments":{}})
check("D1.4 REST 分组 call 真实调用", rest_ok(st, obj), f"st={st} {str(obj)[:120]}")
# REST 未知工具 → 明确错误非 5xx
st, obj, ct, data = http_req("POST", f"/rest/{srv}/call", {"tool":"no_such_tool","arguments":{}})
check("D1.5 REST 未知工具错误非 5xx", st < 500 and st >= 400, f"st={st}")

# ============ D2: $smart 通道 ============
print("== D2 $smart 通道 ==")
st, obj, ct, data = http_req("POST", "/mcp/$smart", {"jsonrpc":"2.0","id":1,"method":"tools/list","params":META})
tools2 = (obj or {}).get("result", {}).get("tools", [])
check("D2.1 $smart tools/list 可响应（3 个 meta 工具或未启用提示）",
      st==200 and (len(tools2)>0 or "未开启" in str(obj) or "not enabled" in str(obj).lower()), f"st={st} {str(obj)[:100]}")
if tools2:
    names = [t["name"] for t in tools2]
    check("D2.2 $smart meta 工具形态（progressive=3 / 标准=2）",
          names == ["smart_route_search","smart_route_describe","smart_route_call"]
          or names == ["smart_route_search","smart_route_call"], str(names))
    st2, obj2 = mcp_call(None, "smart_route_search", base="/mcp/$smart", version="2025-11-25")  # sid=None → 无 session 头
    # 未启用 smart routing 时应返回明确提示（isError content 或 error），不 5xx
    check("D2.3 smart_route_search 调用不 5xx（未启用给明确提示）", st2 < 500, f"st={st2} {str(obj2)[:100]}")

# ============ D3: GET /mcp SSE（rmcp 语义：需 session）============
print("== D3 GET SSE（session 绑定流） ==")
sid3 = mcp_session("2024-11-05")
c = http.client.HTTPConnection(HOST, PORT, timeout=15)
c.request("GET", "/mcp", headers={"Accept":"text/event-stream", "mcp-session-id": sid3})
r = c.getresponse()
st = r.status; ct = r.getheader("content-type") or ""
c.close()
check("D3.1 GET /mcp + session → SSE 流打开", st==200 and "text/event-stream" in ct, f"st={st} ct={ct}")
st, obj, ct, data = http_req("GET", "/mcp", headers={"Accept":"text/event-stream"})
check("D3.2 GET /mcp 无 session → 400（rmcp 语义，退役无会话拉取）", st==400, f"st={st}")
st, obj, ct, data = http_req("POST", "/mcp/message?sessionId=x", {"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}})
check("D3.3 /mcp/message 退役端点非 5xx（宽松升格或 422/400）", st < 500, f"st={st}")

# ============ D4: 2025-11-25 legacy core tasks ============
print("== D4 legacy core tasks ==")
sid = mcp_session("2025-11-25")
st, obj, ct, data = http_req("POST", "/mcp", {"jsonrpc":"2.0","id":2,"method":"tasks/get",
    "params":{"taskId":"no-such"}}, {"mcp-session-id":sid,"MCP-Protocol-Version":"2025-11-25"})
err = (obj or {}).get("error", {})
check("D4.1 2025-11 tasks/get 核心方法可达（未知 id 报业务错非 -32601）",
      st==200 and err.get("code") not in (None, -32601), f"st={st} err={err.get('code')} {err.get('message','')[:60]}")
st, obj, ct, data = http_req("POST", "/mcp", {"jsonrpc":"2.0","id":3,"method":"tasks/list",
    "params":{}}, {"mcp-session-id":sid,"MCP-Protocol-Version":"2025-11-25"})
check("D4.2 2025-11 tasks/list 可达", st==200 and "error" not in (obj or {}), f"st={st} {str(obj)[:80]}")

# ============ D5: bearer 矩阵（自动还原）============
print("== D5 bearer 矩阵 ==")
DB = os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")
def set_bearer(on):
    con = sqlite3.connect(DB)
    cfg = json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
    cfg.setdefault("routing", {})["enableBearerAuth"] = on
    con.execute("UPDATE system_config SET config_json=?, updated_at=datetime('now') WHERE rowid=1",(json.dumps(cfg),))
    con.commit(); con.close()
def get_key():
    con = sqlite3.connect(DB)
    row = con.execute("SELECT token FROM bearer_keys LIMIT 1").fetchone()
    con.close()
    return row[0] if row else None
KEY = get_key()
try:
    set_bearer(True); time.sleep(0.3)
    st, obj, ct, data = http_req("POST", "/mcp", {"jsonrpc":"2.0","id":1,"method":"tools/list","params":META})
    check("D5.1 无 key POST → 401", st==401, f"st={st}")
    hdr = {"Authorization": f"Bearer {KEY}"}
    st, obj, ct, data = http_req("POST", "/mcp", {"jsonrpc":"2.0","id":1,"method":"tools/list","params":META}, hdr)
    check("D5.2 带 key tools/list → 200", st==200 and (obj or {}).get("result",{}).get("tools"), f"st={st}")
    st, obj, ct, data = http_req("POST", f"/rest/{srv}/call", {"tool":IP_TOOL,"arguments":{}}, hdr)
    check("D5.3 带 key REST 真实调用", rest_ok(st, obj), f"st={st} {str(obj)[:100]}")
    st, obj, ct, data = http_req("POST", "/mcp", {"jsonrpc":"2.0","id":2,"method":"server/discover","params":{}})
    check("D5.4 2026 discover 无 key → 401", st==401, f"st={st}")
finally:
    set_bearer(False); time.sleep(0.3)
st, obj, ct, data = http_req("POST", "/mcp", {"jsonrpc":"2.0","id":1,"method":"tools/list","params":META})
check("D5.5 还原后无 key 恢复 200", st==200, f"st={st}")

print(f"\n== {len(PASS)}/{len(PASS)+len(FAIL)} passed ==")
if FAIL:
    print("FAILED:"); [print(" -", f) for f in FAIL]
sys.exit(1 if FAIL else 0)
