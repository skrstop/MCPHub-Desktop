#!/usr/bin/env python3
# 宽松模式 v2：本轮复核修复的放行用例（D1/D2/D3/D4）+ 严格模式对应拒绝 + 回归
import json, http.client, sqlite3, os, sys, time

HOST, PORT = "localhost", 23333
DB = os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")
PASS, FAIL = [], []

def check(name, ok, detail=""):
    (PASS if ok else FAIL).append(name)
    print(("PASS" if ok else "FAIL"), "|", name, (("| " + str(detail)[:120]) if (detail and not ok) else ""))

def req(payload, headers=None, method="POST", path="/mcp", raw_body=None, accept="application/json, text/event-stream"):
    c = http.client.HTTPConnection(HOST, PORT, timeout=30)
    h = {}
    if accept: h["Accept"] = accept
    if method == "POST": h["Content-Type"] = "application/json"
    if headers: h.update(headers)
    body = raw_body if raw_body is not None else (json.dumps(payload) if payload is not None else None)
    c.request(method, path, body=body, headers=h)
    r = c.getresponse(); d = r.read().decode("utf-8", "replace"); st = r.status
    sid = r.getheader("mcp-session-id"); c.close()
    obj = None
    try: obj = json.loads(d)
    except Exception:
        frames = []
        for l in d.split("\n"):
            if l.startswith("data: ") and l[6:].strip():
                try: frames.append(json.loads(l[6:]))
                except Exception: pass
        obj = frames[-1] if frames else None
    return st, sid, obj, d

def init_session(version="2025-06-18"):
    st, sid, obj, _ = req({"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"c","version":"1"}}})
    return sid

def set_strict(on):
    con = sqlite3.connect(DB)
    cfg = json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
    cfg.setdefault("mcp", {})["strictValidation"] = on
    con.execute("UPDATE system_config SET config_json=?, updated_at=datetime('now') WHERE rowid=1", (json.dumps(cfg, ensure_ascii=False),))
    con.commit(); con.close()

def run_lenient():
    print("== 宽松模式 ==")
    set_strict(False); time.sleep(0.2)
    # D1 params 缺失（裸请求升格）
    st, _, obj, _ = req({"jsonrpc":"2.0","id":1,"method":"tools/list"})
    check("D1 params缺失 裸tools/list 放行", st == 200 and obj and "result" in obj, f"st={st}")
    # D2 jsonrpc 缺失 / 1.0 / id null
    st, _, obj, _ = req({"id":1,"method":"tools/list","params":{}})
    check("D2 jsonrpc缺失 放行+id回显", st == 200 and obj and obj.get("id") == 1, f"st={st}")
    st, _, obj, _ = req({"jsonrpc":"1.0","id":2,"method":"tools/list","params":{}})
    check("D2 jsonrpc1.0 放行", st == 200 and obj and "result" in obj, f"st={st}")
    # 真实工具调用（jsonrpc 缺失形态）
    st, _, obj, _ = req({"id":3,"method":"tools/call","params":{"name":"本机公网ip查询-getPublicIp","arguments":{}}})
    txt = "".join(str(c.get("text","")) for c in obj.get("result",{}).get("content",[])) if obj else ""
    check("D2 jsonrpc缺失 公网IP真实调用", st == 200 and obj and obj.get("result",{}).get("isError") is False and len(txt) > 7, f"st={st} txt={txt[:40]}")
    # D4 非法版本头带 session
    sid = init_session()
    for hv in ["1999-01-01", "garbage", "9999-01-01"]:
        st, _, obj, _ = req({"jsonrpc":"2.0","id":10,"method":"tools/list","params":{}},
                            {"Mcp-Session-Id": sid, "MCP-Protocol-Version": hv})
        check(f"D4 非法版本头[{hv}] 放行", st == 200 and obj and "result" in obj, f"st={st}")
    # 正常头不误伤
    st, _, obj, _ = req({"jsonrpc":"2.0","id":11,"method":"tools/list","params":{}},
                        {"Mcp-Session-Id": sid, "MCP-Protocol-Version": "2025-06-18"})
    check("正常版本头不误伤", st == 200 and obj and "result" in obj, f"st={st}")
    # D3 GET 缺 Accept → SSE
    c = http.client.HTTPConnection(HOST, PORT, timeout=30)
    c.request("GET", "/mcp", headers={"Mcp-Session-Id": sid})
    r = c.getresponse(); r.read(50); st = r.status; ct = r.getheader("content-type") or ""; c.close()
    check("D3 GET缺Accept SSE打开", st == 200 and "text/event-stream" in ct, f"st={st} ct={ct}")
    req(None, {"Mcp-Session-Id": sid}, method="DELETE")
    # unknown _meta pv 仍报结构化 -32022（不能被宽松吞掉）
    st, _, obj, _ = req({"jsonrpc":"2.0","id":20,"method":"tools/list",
        "params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2099-01-01"}}})
    e = obj.get("error", {}) if obj else {}
    check("unknown _meta pv 结构化-32022", e.get("code") == -32022, f"st={st} e={str(e)[:80]}")

def run_strict():
    print("== 严格模式（对应拒绝）==")
    set_strict(True); time.sleep(0.2)
    st, _, obj, _ = req({"jsonrpc":"2.0","id":1,"method":"tools/list"})
    check("严格 params缺失 拒绝", st in (400, 406, 415, 422), f"st={st}")
    st, _, obj, _ = req({"id":1,"method":"tools/list","params":{}})
    check("严格 jsonrpc缺失 拒绝", st in (400, 415, 422), f"st={st}")
    sid = init_session()
    st, _, obj, _ = req({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}},
                        {"Mcp-Session-Id": sid, "MCP-Protocol-Version": "1999-01-01"})
    check("严格 非法版本头 拒绝", st == 400, f"st={st}")
    c = http.client.HTTPConnection(HOST, PORT, timeout=30)
    c.request("GET", "/mcp", headers={"Mcp-Session-Id": sid})
    r = c.getresponse(); r.read(50); st = r.status; c.close()
    check("严格 GET缺Accept 拒绝(406)", st == 406, f"st={st}")
    # 规范请求不受影响
    st, sid2, obj, _ = req({"jsonrpc":"2.0","id":3,"method":"initialize",
        "params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"c","version":"1"}}})
    check("严格 规范 initialize 通过", st == 200 and obj and "result" in obj, f"st={st}")
    if sid2: req(None, {"Mcp-Session-Id": sid2}, method="DELETE")
    if sid: req(None, {"Mcp-Session-Id": sid}, method="DELETE")

run_lenient()
try:
    run_strict()
finally:
    # 还原必须在 finally：strict 中途崩溃时残留 True 会毒化后续所有套件
    # （review round 9，与第 8 轮连环失败同型）。
    set_strict(False)
    time.sleep(0.2)
print(f"\n== {len(PASS)}/{len(PASS)+len(FAIL)} passed ==")
if FAIL:
    print("FAILED:", *FAIL, sep="\n  - "); sys.exit(1)
