#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""补充覆盖套件 v22（R22 复核轮固化）——HTTP 面边界与方法语义：
1. 方法不匹配：PUT/DELETE/PATCH /rest 与 /api → 405（非 5xx）
2. 未知路径 404（非 5xx）：/rest/None/..., /api/unknown/..., /mcp/unknown-scope 行为
3. 严格模式 × /api × /rest：严格只管 /mcp 协议面，REST 面不受影响
4. Content-Type 缺失/错误 POST：宽松放行或 4xx，绝不 5xx
5. 空 body POST /rest/call → 4xx 语义
6. 并发同会话串行性（同 session 并发 3 ping 全 200）
"""
import http.client, json, sqlite3, os, sys, time, urllib.parse, threading

HOST, PORT = "localhost", 23333
DB = os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")
IP_SERVER = "本机公网ip查询"
SCOPE = urllib.parse.quote(IP_SERVER)
GROUP = "Test"

PASS, FAIL, FAILED = [], [], []
def check(name, cond, detail=""):
    if cond: PASS.append(name); print(f"PASS | {name}" + (f" ({detail})" if detail else ""))
    else: FAIL.append(name); FAILED.append(name); print(f"FAIL | {name} :: {detail[:200]}")

def raw(method, path, body=None, headers=None, timeout=30):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = dict(headers or {})
    c.request(method, path, body=body, headers=h)
    r = c.getresponse(); d = r.read().decode("utf-8", "replace")
    c.close()
    return r.status, d

def set_strict(on):
    con = sqlite3.connect(DB)
    cfg = json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
    cfg.setdefault("mcp", {})["strictValidation"] = bool(on)
    con.execute("UPDATE system_config SET config_json=?, updated_at=datetime('now') WHERE rowid=1",
                (json.dumps(cfg, ensure_ascii=False),))
    con.commit(); con.close()

def method_semantics(tag):
    for m in ("PUT", "DELETE", "PATCH"):
        st, _ = raw(m, f"/rest/{SCOPE}/tools")
        check(f"{tag} {m} /rest/tools → 405 非 5xx", st == 405, f"st={st}")
        st, _ = raw(m, "/api/openapi.json")
        check(f"{tag} {m} /api/openapi.json → 405 非 5xx", st == 405, f"st={st}")
    # 未知路径 404
    st, _ = raw("GET", "/rest/NoSuchServerZZ/tools")
    check(f"{tag} 未知服务器 /rest → 4xx", 400 <= st < 500, f"st={st}")
    st, _ = raw("GET", "/api/unknown-ns-xyz/thing")
    check(f"{tag} 未知 /api 路径 → 4xx", 400 <= st < 500, f"st={st}")
    # Content-Type 缺失 POST /rest/call → 4xx 非 5xx
    st, _ = raw("POST", f"/rest/{SCOPE}/call", json.dumps({"tool":"getPublicIp","arguments":{}}))
    check(f"{tag} 无 Content-Type POST → 非 5xx", st < 500, f"st={st}")
    # 错误 Content-Type
    st, _ = raw("POST", f"/rest/{SCOPE}/call", "not json", {"Content-Type": "text/plain"})
    check(f"{tag} text/plain body → 非 5xx", st < 500, f"st={st}")
    # 空 body POST call → 4xx
    st, _ = raw("POST", f"/rest/{SCOPE}/call", "", {"Content-Type": "application/json"})
    check(f"{tag} 空 body call → 4xx", 400 <= st < 500, f"st={st}")

def strict_unaffected():
    set_strict(True); time.sleep(0.4)
    try:
        # 严格模式只作用于 /mcp；REST 面保持正常
        st, d = raw("GET", f"/rest/{SCOPE}/tools")
        check("[strict] /rest/tools 不受严格模式影响", st == 200, f"st={st}")
        st, d = raw("GET", "/api/openapi.json")
        check("[strict] /api/openapi.json 不受严格模式影响", st == 200, f"st={st}")
        # 严格 /mcp：缺 Accept initialize → 拒绝 4xx
        c = http.client.HTTPConnection(HOST, PORT, timeout=15)
        c.request("POST", "/mcp", json.dumps({"jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"m22","version":"1"}}}),
            {"Content-Type":"application/json"})
        r = c.getresponse(); r.read(); c.close()
        check("[strict] /mcp 缺 Accept → 4xx 拒绝", 400 <= r.status < 500, f"st={r.status}")
    finally:
        set_strict(set_strict(False)); time.sleep(0.4)
    # 宽松恢复：同请求放行（结果可解析）
    st, d = raw("POST", "/mcp", json.dumps({"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"m22","version":"1"}}}),
        {"Content-Type":"application/json"})
    check("[lenient] /mcp 缺 Accept → 放行 2xx", 200 <= st < 300, f"st={st}")

def concurrent_same_session():
    # 建会话
    c = http.client.HTTPConnection(HOST, PORT, timeout=30)
    c.request("POST", "/mcp", json.dumps({"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"m22","version":"1"}}}),
        {"Content-Type":"application/json","Accept":"application/json, text/event-stream"})
    r = c.getresponse(); r.read(); sid = r.getheader("mcp-session-id"); c.close()
    req("POST", "/mcp", {"jsonrpc":"2.0","method":"notifications/initialized"},
        {"Content-Type":"application/json","Accept":"application/json, text/event-stream","Mcp-Session-Id":sid})
    results = []
    def worker():
        try:
            # Legacy-session POST responses are SSE streams that stay open —
            # reading the body would block. Read only the status line.
            c = http.client.HTTPConnection(HOST, PORT, timeout=30)
            c.request("POST", "/mcp", json.dumps({"jsonrpc":"2.0","id":7,"method":"ping","params":{}}),
                      {"Content-Type":"application/json","Accept":"application/json, text/event-stream","Mcp-Session-Id":sid})
            r = c.getresponse()
            results.append(r.status)
            c.close()
        except Exception as e:
            results.append(str(e)[:40])
    ts=[threading.Thread(target=worker) for _ in range(3)]
    for t in ts: t.start()
    for t in ts: t.join(30)
    ok = all(isinstance(s,int) and 200 <= s < 300 for s in results) and len(results)==3
    check("[并发] 同会话 3 并发 ping 全 2xx", ok, f"results={results}")

def req(method, path, body, headers):
    c = http.client.HTTPConnection(HOST, PORT, timeout=30)
    c.request(method, path, json.dumps(body), headers)
    r = c.getresponse(); r.read(); st = r.status; c.close()
    return st, None

def main():
    print("== v22 HTTP 边界与方法语义套件 ==")
    method_semantics("[methods]")
    strict_unaffected()
    concurrent_same_session()
    print(f"\n== {len(PASS)}/{len(PASS)+len(FAIL)} passed ==")
    if FAILED:
        print("FAILED:", ", ".join(FAILED[:20])); sys.exit(1)

if __name__ == "__main__":
    main()
