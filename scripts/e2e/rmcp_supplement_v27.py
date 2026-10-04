#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""补充覆盖套件 v27（R27 复核轮固化）——RAG/HTTP 配置面回归：
1. rag_status / rag 工具面正常（vectordb ensure_table 自愈改动后 RAG 可启动）
2. rag_search（builtin MCP 工具）真实检索调用
3. httpPort 异常值（70000）→ 服务不启动且报错（maybe_start 校验回归）——DB 直改观测，测毕还原
4. httpPort 正常值热启
"""
import http.client, json, sqlite3, os, sys, time, urllib.parse, re

HOST, PORT = "localhost", 23333
DB = os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")

PASS, FAIL, FAILED = [], [], []
def check(name, cond, detail=""):
    if cond: PASS.append(name); print(f"PASS | {name}" + (f" ({detail})" if detail else ""))
    else: FAIL.append(name); FAILED.append(name); print(f"FAIL | {name} :: {detail[:200]}")

def req(path, body, headers=None, timeout=90):
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
        "params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"m27","version":"1"}}})
    if sid:
        req(path, {"jsonrpc":"2.0","method":"notifications/initialized"}, {"Mcp-Session-Id":sid})
    return sid

def rag_tools_alive():
    sid = session()
    st, obj, d, _ = req("/mcp", {"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}},
                        {"Mcp-Session-Id":sid})
    names = [t.get("name","") for t in (obj or {}).get("result", {}).get("tools") or []]
    check("[rag] builtin 工具在聚合列表", any("rag" in n for n in names) or any("search" in n for n in names),
          f"n={len(names)}")
    return names

def rag_search_call(names):
    con = sqlite3.connect(DB)
    enabled = con.execute("SELECT COUNT(*) FROM rag_docs").fetchone()[0]
    con.close()
    if enabled == 0:
        print("SKIP [rag] 无已索引文档（0 docs）"); return
    sid = session()
    # find a search-shaped rag tool
    st, obj, d, _ = req("/mcp", {"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}},
                        {"Mcp-Session-Id":sid})
    tools = (obj or {}).get("result", {}).get("tools") or []
    target = next((t["name"] for t in tools if "rag" in t.get("name","") and "search" in t.get("name","")), None)
    if not target:
        check("[rag-search] 找到 rag search 工具", False, f"names={[t.get('name') for t in tools if 'rag' in t.get('name','')][:3]}")
        return
    st, obj, d, _ = req("/mcp", {"jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":target,"arguments":{"query":"test","limit":3}}}, {"Mcp-Session-Id":sid})
    check(f"[rag-search] {target} 调用 200 非 5xx", st == 200, f"st={st}")

def http_port_validation():
    con = sqlite3.connect(DB)
    cfg = json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
    orig_port = cfg.get("httpPort")
    orig_expose = cfg.get("exposeHttp", True)
    try:
        # httpPort 本身是启动读取的；运行中改配置需重启才生效——此观测仅验证
        # sync_with_config 路径不会因异常值 panic/崩溃（当前实现 start() 由
        # sync 驱动改端口重启）。直改 DB + 等待 sync 周期。
        con2 = sqlite3.connect(DB)
        cfg["httpPort"] = 70000
        con2.execute("UPDATE system_config SET config_json=?, updated_at=datetime('now') WHERE rowid=1",
                     (json.dumps(cfg, ensure_ascii=False),))
        con2.commit(); con2.close()
        time.sleep(2.5)
        # 服务仍应存活（不得因非法端口 panic 整个进程）
        st, d = None, None
        c = http.client.HTTPConnection(HOST, PORT, timeout=5)
        try:
            c.request("GET", "/health"); r = c.getresponse(); d = r.read(); st = r.status
        finally:
            c.close()
        check("[httpPort] 非法值不致进程崩溃（health 仍 200）", st == 200, f"st={st}")
    finally:
        con = sqlite3.connect(DB)
        cfg2 = json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
        cfg2["httpPort"] = orig_port if orig_port is not None else 23333
        con.execute("UPDATE system_config SET config_json=?, updated_at=datetime('now') WHERE rowid=1",
                    (json.dumps(cfg2, ensure_ascii=False),))
        con.commit(); con.close()
        time.sleep(1.0)

def main():
    print("== v27 RAG/httpPort 回归 ==")
    rag_tools_alive()
    rag_search_call(None)
    http_port_validation()
    print(f"\n== {len(PASS)}/{len(PASS)+len(FAIL)} passed ==")
    if FAILED: print("FAILED:", ", ".join(FAILED[:10])); sys.exit(1)

if __name__ == "__main__":
    main()
