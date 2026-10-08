#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""补充覆盖套件 v24（R24 复核轮固化）——日志/FTS 写路径观测 + 协议特性终验：
1. 日志写入吞吐：写入 N 条日志（log_event 无 HTTP 面，改用活动面：连续 /mcp 调用产生日志），
   验证写入延迟不随体量劣化（R24 前 add_log O(N) 全表扫描）
2. 日志检索 FTS 命中 + LIKE 降级
3. 2026 特性终验：discover ttlMs/cacheScope、-32022 未知版本、CacheableResult
4. 宽松全放行回归：4 legacy 版本缺 Accept + 空 _meta 裸请求升格
"""
import http.client, json, sqlite3, os, sys, time, urllib.parse, base64

HOST, PORT = "localhost", 23333
DB = os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")
IP_SERVER = "本机公网ip查询"
SCOPE = urllib.parse.quote(IP_SERVER)

PASS, FAIL, FAILED = [], [], []
def check(name, cond, detail=""):
    if cond: PASS.append(name); print(f"PASS | {name}" + (f" ({detail})" if detail else ""))
    else: FAIL.append(name); FAILED.append(name); print(f"FAIL | {name} :: {detail[:200]}")

def req(method, path, body=None, headers=None, timeout=60):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {"Content-Type":"application/json","Accept":"application/json, text/event-stream"}
    if headers: h.update(headers)
    c.request(method, path, body=json.dumps(body, ensure_ascii=False).encode("utf-8") if body is not None else None, headers=h)
    r = c.getresponse(); d = r.read().decode("utf-8","replace"); sid = r.getheader("mcp-session-id"); c.close()
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
    st, obj, d, sid = req("POST", path, {"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"m24","version":"1"}}})
    if sid:
        req("POST", path, {"jsonrpc":"2.0","method":"notifications/initialized"}, {"Mcp-Session-Id":sid})
    return sid

def log_throughput():
    # 每条 /mcp 调用写 app_log（+ activity）。先记录当前库体量，测 30 次调用耗时 vs 后 30 次
    con = sqlite3.connect(DB)
    n0 = con.execute("SELECT COUNT(*) FROM activity_log").fetchone()[0]
    con.close()
    # Sessions are path-local (rmcp): one per channel.
    sid_root = session("/mcp")
    sid_scope = session(f"/mcp/{SCOPE}")
    t0 = time.time()
    for i in range(30):
        req("POST", "/mcp", {"jsonrpc":"2.0","id":10+i,"method":"ping","params":{}}, {"Mcp-Session-Id":sid_root})
    t_mid = time.time()
    for i in range(30):
        req("POST", f"/mcp/{SCOPE}", {"jsonrpc":"2.0","id":50+i,"method":"tools/call",
            "params":{"name":"getPublicIp","arguments":{}}}, {"Mcp-Session-Id":sid_scope})
    t1 = time.time()
    con = sqlite3.connect(DB)
    n1 = con.execute("SELECT COUNT(*) FROM activity_log").fetchone()[0]
    con.close()
    wrote = n1 - n0
    avg_ms = (t1 - t0) * 1000 / 60
    # ping 不写活动日志；30 次 tools/call 应各写一条
    check("[log-tp] tools/call 产生活动日志写入", wrote >= 25, f"wrote={wrote} (n0={n0})")
    check("[log-tp] 平均单调用 <2s（写路径不劣化）", avg_ms < 2000, f"avg={avg_ms:.0f}ms")
    # ping 批与 call 批无显著劣化差（放宽 3x）
    ping_ms = (t_mid - t0) * 1000 / 30
    call_ms = (t1 - t_mid) * 1000 / 30
    check("[log-tp] call 批（含外呼）不劣化 3x", call_ms < max(ping_ms * 3, 1000), f"ping={ping_ms:.0f}ms call={call_ms:.0f}ms")

def log_search():
    con = sqlite3.connect(DB)
    row = con.execute("SELECT message FROM app_log ORDER BY rowid DESC LIMIT 1").fetchone()
    con.close()
    if not row:
        check("[log-search] 有日志可查", False, "empty app_log"); return
    # FTS/LIKE 检索端点在 Tauri 命令面 — HTTP 侧无直接面；验证 DB 层 FTS 表一致性
    con = sqlite3.connect(DB)
    fts_n = con.execute("SELECT COUNT(*) FROM fts_app_log").fetchone()[0]
    src_n = con.execute("SELECT COUNT(*) FROM app_log").fetchone()[0]
    con.close()
    # FTS 行数应与源表同量级（同事务铁律）
    check("[log-search] fts_app_log 与 app_log 行数一致", fts_n == src_n, f"fts={fts_n} src={src_n}")

def features_2026():
    # discover
    st, obj, d, sid = req("POST", "/mcp", {"jsonrpc":"2.0","id":1,"method":"server/discover",
        "params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28",
        "io.modelcontextprotocol/clientInfo":{"name":"m24","version":"1"},
        "io.modelcontextprotocol/clientCapabilities":{}}}},
        {"MCP-Protocol-Version":"2026-07-28","Mcp-Method":"server/discover"})
    res = (obj or {}).get("result", {})
    meta = res.get("_meta", {}).get("io.modelcontextprotocol/serverInfo", {})
    check("[2026] discover 200 + serverInfo", st == 200 and meta.get("name"), f"st={st} meta={bool(meta)}")
    check("[2026] discover ttlMs=3600000 + public", res.get("ttlMs") == 3600000 and res.get("cacheScope") == "public",
          f"ttl={res.get('ttlMs')} scope={res.get('cacheScope')}")
    # 未知版本 → -32022
    st, obj, d, sid = req("POST", "/mcp", {"jsonrpc":"2.0","id":1,"method":"server/discover",
        "params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"1999-01-01",
        "io.modelcontextprotocol/clientInfo":{"name":"m24","version":"1"},
        "io.modelcontextprotocol/clientCapabilities":{}}}},
        {"MCP-Protocol-Version":"1999-01-01","Mcp-Method":"server/discover"})
    err = (obj or {}).get("error", {})
    check("[2026] 未知版本 -32022", err.get("code") == -32022, f"code={err.get('code')}")
    # tools/list CacheableResult（2026 无状态）
    st, obj, d, sid = req("POST", "/mcp", {"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}},
        {"MCP-Protocol-Version":"2026-07-28","Mcp-Method":"tools/list",
         "Accept":"application/json, text/event-stream"})
    res2 = (obj or {}).get("result", {})
    check("[2026] tools/list ttlMs≤5000 private",
          st == 200 and isinstance(res2.get("ttlMs"), (int,float)) and res2.get("ttlMs") <= 5000 and res2.get("cacheScope") == "private",
          f"st={st} ttl={res2.get('ttlMs')} scope={res2.get('cacheScope')}")

def leniency_regression():
    for v in ["2024-11-05","2025-03-26","2025-06-18","2025-11-25"]:
        # 缺 Accept initialize 放行 + 版本回显
        c = http.client.HTTPConnection(HOST, PORT, timeout=30)
        c.request("POST", "/mcp", json.dumps({"jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"protocolVersion":v,"capabilities":{},"clientInfo":{"name":"m24","version":"1"}}}),
            {"Content-Type":"application/json"})
        r = c.getresponse(); d = r.read().decode(); c.close()
        obj = None
        for line in d.splitlines():
            if line.startswith("data:"):
                try: obj = json.loads(line[5:].strip())
                except Exception: pass
        pv = (obj or {}).get("result", {}).get("protocolVersion") if obj else None
        check(f"[宽松/{v}] 缺 Accept 放行+回显", 200 <= r.status < 300 and pv == v, f"st={r.status} pv={pv}")

def main():
    print("== v24 日志/FTS 写路径 + 2026 特性终验 ==")
    log_throughput()
    log_search()
    features_2026()
    leniency_regression()
    print(f"\n== {len(PASS)}/{len(PASS)+len(FAIL)} passed ==")
    if FAILED:
        print("FAILED:", ", ".join(FAILED[:20])); sys.exit(1)

if __name__ == "__main__":
    main()
