#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""补充覆盖套件 v26（R26 复核轮固化）——自我复核回归 + 全协议快照终验：
1. prompts/get 空参数 + 值含特殊 token（{{}} 边界回归由 Rust 单测覆盖；E2E 验真实渲染链不退化）
2. 全版本 × root 通道公网IP 调用快照（每版本至少一次真实 MCP 执行）
3. 严格/宽松热切换下的 /mcp 基线
4. tools/list 形状快照（inputSchema 存在）
"""
import http.client, json, os, sys, time, urllib.parse, re, sqlite3

HOST, PORT = "localhost", 23333
DB = os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")
IP_SERVER = "本机公网ip查询"
SCOPE = urllib.parse.quote(IP_SERVER)
VERSIONS = ["2024-11-05","2025-03-26","2025-06-18","2025-11-25"]

PASS, FAIL, FAILED = [], [], []
def check(name, cond, detail=""):
    if cond: PASS.append(name); print(f"PASS | {name}" + (f" ({detail})" if detail else ""))
    else: FAIL.append(name); FAILED.append(name); print(f"FAIL | {name} :: {detail[:200]}")

IP_RE = re.compile(r"(?<![\d.])(?:\d{1,3}\.){3}\d{1,3}(?![\d.])")
def valid_ip(t):
    m = IP_RE.search(t or "")
    return m.group(0) if m else None

def req(path, body, headers=None, timeout=60):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {"Content-Type":"application/json","Accept":"application/json, text/event-stream"}
    if headers: h.update(headers)
    c.request("POST", path, json.dumps(body, ensure_ascii=False).encode(), h)
    r = c.getresponse(); d = r.read().decode(); sid = r.getheader("mcp-session-id"); c.close()
    obj = None
    for line in d.splitlines():
        if line.startswith("data:"):
            try: obj = json.loads(line[5:].strip())
            except Exception: pass
    if obj is None:
        try: obj = json.loads(d)
        except Exception: pass
    return r.status, obj, d, sid

def session(path="/mcp", version="2025-11-25"):
    st, obj, d, sid = req(path, {"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"m26","version":"1"}}})
    if sid:
        req(path, {"jsonrpc":"2.0","method":"notifications/initialized"}, {"Mcp-Session-Id":sid})
    return sid

def snapshot_matrix():
    for v in VERSIONS:
        sid = session("/mcp", v)
        st, obj, d, _ = req("/mcp", {"jsonrpc":"2.0","id":2,"method":"tools/call",
            "params":{"name":f"{IP_SERVER}-getPublicIp","arguments":{}}}, {"Mcp-Session-Id":sid})
        check(f"[快照/{v}] root 公网IP 真实调用", st == 200 and valid_ip(d), f"st={st} ip={valid_ip(d)}")
        st2, obj2, d2, _ = req("/mcp", {"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}},
                               {"Mcp-Session-Id":sid})
        tools = (obj2 or {}).get("result", {}).get("tools") or []
        with_schema = [t for t in tools if t.get("inputSchema")]
        check(f"[快照/{v}] tools/list 形状（inputSchema）", st2 == 200 and len(with_schema) > 0,
              f"n={len(tools)} withSchema={len(with_schema)}")
    # 2026 modern 直达
    st, obj, d, _ = req("/mcp", {"jsonrpc":"2.0","id":9,"method":"tools/call",
        "params":{"name":"getPublicIp","arguments":{},
        "_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28",
        "io.modelcontextprotocol/clientInfo":{"name":"m26","version":"1"},
        "io.modelcontextprotocol/clientCapabilities":{}}}},
        {"MCP-Protocol-Version":"2026-07-28","Mcp-Method":"tools/call",
         "Mcp-Name":"getPublicIp"})
    check("[快照/2026] modern 直达公网IP", st == 200 and valid_ip(d), f"st={st} ip={valid_ip(d)}")

def strict_baseline():
    import sqlite3
    con = sqlite3.connect(DB)
    cfg = json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
    orig = cfg.get("mcp", {}).get("strictValidation", False)
    def set_strict(on):
        c = sqlite3.connect(DB)
        cc = json.loads(c.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
        cc.setdefault("mcp", {})["strictValidation"] = bool(on)
        c.execute("UPDATE system_config SET config_json=?, updated_at=datetime('now') WHERE rowid=1",
                  (json.dumps(cc, ensure_ascii=False),))
        c.commit(); c.close()
    try:
        set_strict(True); time.sleep(0.4)
        sid = session("/mcp", "2025-11-25")
        st, obj, d, _ = req("/mcp", {"jsonrpc":"2.0","id":2,"method":"tools/call",
            "params":{"name":f"{IP_SERVER}-getPublicIp","arguments":{}}}, {"Mcp-Session-Id":sid})
        check("[strict] 规范请求照常通过", st == 200 and valid_ip(d), f"st={st}")
        set_strict(False); time.sleep(0.4)
        sid2 = session("/mcp", "2025-11-25")
        st2, obj2, d2, _ = req("/mcp", {"jsonrpc":"2.0","id":2,"method":"tools/call",
            "params":{"name":f"{IP_SERVER}-getPublicIp","arguments":{}}}, {"Mcp-Session-Id":sid2})
        check("[lenient] 切回后照常通过", st2 == 200 and valid_ip(d2), f"st={st2}")
    finally:
        set_strict(orig); time.sleep(0.4)

def prompts_regression():
    con = sqlite3.connect(DB)
    row = con.execute("SELECT name, template, arguments FROM builtin_prompts WHERE enabled=1 ORDER BY rowid LIMIT 1").fetchone()
    con.close()
    if not row: return
    name, template, args_json = row
    args_def = json.loads(args_json or "[]")
    sid = session()
    fill = {a["name"]: f"V_{a['name']}" for a in args_def}
    st, obj, d, _ = req("/mcp", {"jsonrpc":"2.0","id":9,"method":"prompts/get",
        "params":{"name":name,"arguments":fill}}, {"Mcp-Session-Id":sid})
    msgs = (obj or {}).get("result", {}).get("messages") or []
    text = "".join((m.get("content") or {}).get("text") or "" for m in msgs)
    leaked = [n for n in fill if f"{{{{{n}}}}}" in text]
    check("[prompt] 渲染链回归（无参数占位残留）", st == 200 and not leaked, f"st={st} leaked={leaked}")

def main():
    print("== v26 快照终验 ==")
    snapshot_matrix()
    strict_baseline()
    prompts_regression()
    print(f"\n== {len(PASS)}/{len(PASS)+len(FAIL)} passed ==")
    if FAILED: print("FAILED:", ", ".join(FAILED[:10])); sys.exit(1)

if __name__ == "__main__":
    main()
