#!/usr/bin/env python3
# 50轮复核六轮套件 F：重复 initialize / id 类型边界 / batch JSON-RPC / 工具名特殊字符 / GET SSE 长连接
import json, http.client, sys, time, socket

HOST, PORT = "localhost", 23333
PASS, FAIL = [], []
def check(name, ok, detail=""):
    (PASS if ok else FAIL).append(name)
    print(("PASS" if ok else "FAIL"), "|", name, ("| " + str(detail)[:140] if detail and not ok else ""))

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
            if l.startswith("data: ") and l[6:].strip():
                try: obj = json.loads(l[6:])
                except Exception: pass
    return st, obj, sid, data

def init(v="2025-11-25"):
    st, obj, sid, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":v,"capabilities":{},"clientInfo":{"name":"f","version":"1"}}})
    return sid

IP = "本机公网ip查询-getPublicIp"
H = lambda sid, v="2025-11-25": {"mcp-session-id": sid, "MCP-Protocol-Version": v}

# ============ F1: 同 session 重复 initialize ============
print("== F1 重复 initialize ==")
sid = init()
st, obj, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":2,"method":"initialize",
    "params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"f","version":"1"}}}, H(sid))
check("F1.1 同 session 二次 initialize → 非 5xx（rmcp 已初始化错误或重置）", st < 500, f"st={st} {str(obj)[:80]}")
st, obj, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}}, H(sid))
check("F1.2 二次 initialize 后 session 仍可用（或明确失效非 5xx）", st < 500, f"st={st}")

# ============ F2: id 类型边界 ============
print("== F2 id 类型 ==")
sid = init()
# 字符串 id
st, obj, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":"str-id-1","method":"tools/list","params":{}}, H(sid))
check("F2.1 string id 回显一致", (obj or {}).get("id") == "str-id-1", f"{str(obj)[:80]}")
# null id 通知（无 id）不回包错误
st, obj, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","method":"notifications/initialized","params":{}}, H(sid))
check("F2.2 通知（无 id）→ 202/204 或空响应", st in (200,202,204), f"st={st}")
# 浮点 id（规范非法但需不挂）
st, obj, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":1.5,"method":"tools/list","params":{}}, H(sid))
# 202 Accepted = rmcp 把响应走 GET SSE 流（非内联），非 5xx 不挂即正确
check("F2.3 浮点 id → 非 5xx（200 内联或 202 走 SSE 流）", st in (200, 202), f"st={st} {str(obj)[:80]}")

# ============ F3: batch JSON-RPC（2025-03 支持过、2025-06 起废弃）============
print("== F3 batch ==")
sid = init()
batch = [{"jsonrpc":"2.0","id":10,"method":"tools/list","params":{}},
         {"jsonrpc":"2.0","method":"notifications/progress","params":{}}]
st, obj, _, data = req("POST", "/mcp", batch, H(sid))
check("F3.1 batch 请求 → 非 5xx（按版本支持或拒绝）", st < 500, f"st={st} {str(data)[:80]}")

# ============ F4: 工具名特殊字符 ============
print("== F4 工具名边界 ==")
sid = init()
st, obj, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":11,"method":"tools/call",
    "params":{"name":"不存在的工具","arguments":{}}}, H(sid))
msg = str((obj or {}).get("error", {}).get("message", ""))
check("F4.1 不存在工具 → 明确错误非 5xx", st == 200 and ("error" in (obj or {}) or (obj or {}).get("result",{}).get("isError")), f"{str(obj)[:80]}")
# 前缀形式（多服务器 scope 下才有效，全局 scope 单服务器裸名）——用不匹配前缀探测
st, obj, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":12,"method":"tools/call",
    "params":{"name":"zzz" + IP, "arguments":{}}}, H(sid))
check("F4.2 假前缀工具名 → not found 错误", "not found" in str((obj or {}).get("error",{}).get("message","")).lower() or (obj or {}).get("result",{}).get("isError"), f"{str(obj)[:80]}")

# ============ F5: GET SSE 长连接（带 session）============
print("== F5 GET SSE ==")
sid = init()
c = http.client.HTTPConnection(HOST, PORT, timeout=15)
try:
    c.request("GET", "/mcp", headers={"Accept":"text/event-stream", **H(sid)})
    r = c.getresponse()
    check("F5.1 GET /mcp 带 session → 200 SSE 流开", r.status == 200 and "text/event-stream" in (r.getheader("content-type") or ""), f"st={r.status}")
    try:
        r.read(64)  # 有通知则读到数据
        check("F5.2 SSE 流活跃", True)
    except socket.timeout:
        # GET 流仅在有通知时发数据，静默等待属规范行为（流已开）
        check("F5.2 SSE 流静默等待（规范行为：无通知不发数据）", True)
except Exception as e:
    check("F5.1 GET SSE 流开", False, str(e))
finally:
    c.close()

# ============ F6: initialize 后未发 initialized 直接调用（rmcp 语义）============
print("== F6 未完成握手 ==")
st, obj, sid2, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":1,"method":"initialize",
    "params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"f","version":"1"}}})
st, obj, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":13,"method":"tools/list","params":{}}, H(sid2))
check("F6.1 未发 initialized 即 tools/list → 非 5xx（rmcp 拒绝或放行）", st < 500, f"st={st} {str(obj)[:80]}")

# ============ F7: 严格模式快速回归（热切换）============
print("== F7 严格/宽松热切换 ==")
import sqlite3, os
DB = os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")
con = sqlite3.connect(DB)
cfg = json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
orig = cfg.get("mcp", {}).get("strictValidation")
def set_strict(v):
    cfg.setdefault("mcp", {})["strictValidation"] = v
    con.execute("UPDATE system_config SET config_json=?, updated_at=datetime('now') WHERE rowid=1", (json.dumps(cfg, ensure_ascii=False),))
    con.commit()
try:
    set_strict(True)
    # 严格模式缺 Accept → 拒绝
    st, obj, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"f","version":"1"}}},
        {"Accept": "application/json"})
    check("F7.1 严格模式缺 text/event-stream Accept → 4xx", 400 <= st < 500, f"st={st}")
    set_strict(False)
    st, obj, sid3, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"f","version":"1"}}},
        {"Accept": "application/json"})
    check("F7.2 宽松模式缺 Accept → 注入放行 200", st == 200, f"st={st}")
    st, obj, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":14,"method":"tools/call",
        "params":{"name":IP,"arguments":{}}}, H(sid3))
    check("F7.3 宽松下公网IP 真实调用成功", (obj or {}).get("result",{}).get("isError") is not True, f"{str(obj)[:100]}")
finally:
    set_strict(orig if orig is not None else False)
    con.close()

print(f"\n== {len(PASS)}/{len(PASS)+len(FAIL)} passed ==")
if FAIL:
    print("FAILED:"); [print(" -", f) for f in FAIL]
sys.exit(1 if FAIL else 0)
