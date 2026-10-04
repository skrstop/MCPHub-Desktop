#!/usr/bin/env python3
# 严格/宽松校验矩阵：验证宽松模式（默认）放行不完整请求；严格模式按规范拒绝
import json, http.client, subprocess, sys, sqlite3, os

HOST, PORT = "localhost", 23333
PASS, FAIL = [], []
def check(name, ok, detail=""):
    (PASS if ok else FAIL).append(name)
    print(("PASS" if ok else "FAIL"), "|", name, ("| " + str(detail)[:100] if detail and not ok else ""))

def req(method, path, payload=None, headers=None, timeout=60):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {"Content-Type": "application/json"}
    if headers: h.update(headers)
    body = json.dumps(payload) if payload is not None else None
    c.request(method, path, body=body, headers=h)
    r = c.getresponse(); data = r.read().decode("utf-8","replace")
    st = r.status; ct = r.getheader("content-type") or ""
    c.close()
    obj=None
    if data.strip().startswith("{"):
        try: obj=json.loads(data)
        except Exception: pass
    if obj is None:
        fr=[json.loads(l[6:]) for l in data.split("\n") if l.startswith("data: ") and l[6:].strip()]
        obj=fr[-1] if fr else None
    return st, obj, ct

DB = os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")
def set_strict(on):
    con=sqlite3.connect(DB)
    cfg=json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
    cfg.setdefault("mcp",{})["strictValidation"]=on
    con.execute("UPDATE system_config SET config_json=?, updated_at=datetime('now') WHERE rowid=1",(json.dumps(cfg,ensure_ascii=False),))
    con.commit(); con.close()

def get_strict():
    con=sqlite3.connect(DB)
    cfg=json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
    con.close()
    return cfg.get("mcp",{}).get("strictValidation", False)

CLIENT_META = {"io.modelcontextprotocol/clientInfo":{"name":"m","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}}
PV = "io.modelcontextprotocol/protocolVersion"

def scenario(name, headers, body_meta, expect_ok):
    """带缺陷的 2026 modern tools/list；宽松应放行，严格应拒绝"""
    st, obj, ct = req("POST","/mcp",{"jsonrpc":"2.0","id":7,"method":"tools/list","params":{"_meta":body_meta}},headers)
    if expect_ok:
        check(name+" [宽松放行]", st==200 and obj and "result" in obj and len(obj["result"].get("tools",[]))>0, f"st={st} {str(obj)[:80]}")
    else:
        check(name+" [严格拒绝]", st in (400,406,415,422) or (st==200 and obj and "error" in obj), f"st={st}")

def scenario_init(name, headers, expect_ok):
    """带缺陷的 initialize（免 SEP-2243 豁免）；测 Accept 宽放/严拒"""
    st, obj, ct = req("POST","/mcp",{"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"m","version":"1"}}},headers)
    if expect_ok:
        check(name+" [宽松放行]", st==200 and obj and "result" in obj, f"st={st}")
    else:
        check(name+" [严格拒绝]", st in (400,406,415,422) or (st==200 and obj and "error" in obj), f"st={st}")

def run(expect_ok):
    # 场景1: 版本头 2026 + _meta 只有 protocolVersion（缺 clientInfo/clientCapabilities）
    scenario("S1 缺 client 元数据", {"MCP-Protocol-Version":"2026-07-28","Accept":"application/json, text/event-stream"},
             {PV:"2026-07-28"}, expect_ok)
    # 场景2/3: Accept 缺陷用 initialize 测（免会话语义干扰）
    scenario_init("S2 Accept 单报", {"MCP-Protocol-Version":"2025-11-25","Accept":"application/json"}, expect_ok)
    scenario_init("S3 Accept 缺失", {"MCP-Protocol-Version":"2025-11-25"}, expect_ok)
    # 场景4: _meta 有 protocolVersion 但无版本头
    st, obj, ct = req("POST","/mcp",
        {"jsonrpc":"2.0","id":8,"method":"tools/list","params":{"_meta":{PV:"2026-07-28",**CLIENT_META}}},
        {"Accept":"application/json, text/event-stream"})
    if expect_ok:
        check("S4 meta 有版本无头 [宽松放行]", st==200 and obj and "result" in obj and len(obj.get("result",{}).get("tools",[]))>0, f"st={st} {str(obj)[:80]}")
    else:
        check("S4 meta 有版本无头 [严格拒绝]", st!=200 or (obj and "error" in obj), f"st={st}")

# ── 宽松模式（默认关）──
print("== 宽松模式（strictValidation=false，默认）==")
run(expect_ok=True)
# 宽松下完整规范请求仍正常
st, obj, ct = req("POST","/mcp",{"jsonrpc":"2.0","id":1,"method":"initialize",
    "params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"m","version":"1"}}},
    {"Accept":"application/json, text/event-stream"})
check("宽松: 正常请求不受影响", st==200 and obj and "result" in obj)

# ── 严格模式 ──
print("== 严格模式（strictValidation=true）==")
set_strict(True)
try:
    # 配置热读：中间件每请求读 config（与 is_skip_auth_enabled 同模式），无需重启
    run(expect_ok=False)
    # 严格下完整规范请求仍正常
    st, obj, ct = req("POST","/mcp",{"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"m","version":"1"}}},
        {"Accept":"application/json, text/event-stream"})
    check("严格: 正常请求不受影响", st==200 and obj and "result" in obj)
finally:
    set_strict(False)  # 还原默认关

print(f"\n== {len(PASS)}/{len(PASS)+len(FAIL)} passed ==")
if FAIL:
    print("FAILED:", *FAIL, sep="\n  - "); sys.exit(1)
