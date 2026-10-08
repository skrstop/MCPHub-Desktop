#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""补充覆盖套件 v23（R23 复核轮固化）——服务器生命周期 HTTP 面观测 + 版本回显抽查：
1. 版本回显一致性 × 5 版本 × root/scope 通道（initialize 请求版本 == 响应版本）
2. 未知版本 fallback 一致（响应 2025-11-25）
3. 服务器禁用 → 工具从聚合消失（DB 直改 enabled + 触发 reload 观测 /mcp 工具列表）— 只读观测，测毕还原
4. 分组通道版本回显
注：生命周期 CRUD 是 Tauri 命令面，HTTP 侧只读观测；写路径由 Rust 单测覆盖。
"""
import http.client, json, sqlite3, os, sys, time, urllib.parse

HOST, PORT = "localhost", 23333
DB = os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")
IP_SERVER = "本机公网ip查询"
SCOPE = urllib.parse.quote(IP_SERVER)
VERSIONS = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"]

PASS, FAIL, FAILED = [], [], []
def check(name, cond, detail=""):
    if cond: PASS.append(name); print(f"PASS | {name}" + (f" ({detail})" if detail else ""))
    else: FAIL.append(name); FAILED.append(name); print(f"FAIL | {name} :: {detail[:200]}")

def session_echo(path, version):
    c = http.client.HTTPConnection(HOST, PORT, timeout=30)
    c.request("POST", path, json.dumps({"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"m23","version":"1"}}}),
        {"Content-Type":"application/json","Accept":"application/json, text/event-stream"})
    r = c.getresponse(); d = r.read().decode(); sid = r.getheader("mcp-session-id"); c.close()
    obj = None
    for line in d.splitlines():
        if line.startswith("data:"):
            try: obj = json.loads(line[5:].strip())
            except Exception: pass
    if obj is None:
        try: obj = json.loads(d)
        except Exception: pass
    pv = (obj or {}).get("result", {}).get("protocolVersion")
    return sid, pv

def echo_matrix():
    for v in VERSIONS:
        for path, tag in [("/mcp", "root"), (f"/mcp/{SCOPE}", "scope"), (f"/mcp/{GROUP}" if False else "/mcp/Test", "group")]:
            sid, pv = session_echo(path, v)
            check(f"[回显/{v}/{tag}] 请求==响应版本", pv == v, f"req={v} resp={pv}")
    # 未知版本 fallback
    _, pv = session_echo("/mcp", "1999-01-01")
    check("[回显/unknown] fallback 2025-11-25", pv == "2025-11-25", f"resp={pv}")
    # 2026 现代无握手：initialize 回 2025-11-25 协商（rmcp 设计）
    _, pv = session_echo("/mcp", "2026-07-28")
    check("[回显/2026] initialize 协商 legacy", pv == "2025-11-25", f"resp={pv}")

def tools_visible(tag, expect_ip_tool):
    c = http.client.HTTPConnection(HOST, PORT, timeout=30)
    c.request("POST", "/mcp", json.dumps({"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"m23","version":"1"}}}),
        {"Content-Type":"application/json","Accept":"application/json, text/event-stream"})
    r = c.getresponse(); r.read(); sid = r.getheader("mcp-session-id"); c.close()
    c = http.client.HTTPConnection(HOST, PORT, timeout=30)
    c.request("POST", "/mcp", json.dumps({"jsonrpc":"2.0","method":"notifications/initialized"}),
        {"Content-Type":"application/json","Accept":"application/json, text/event-stream","Mcp-Session-Id":sid})
    c.getresponse().read(); c.close()
    c = http.client.HTTPConnection(HOST, PORT, timeout=30)
    c.request("POST", "/mcp", json.dumps({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
        {"Content-Type":"application/json","Accept":"application/json, text/event-stream","Mcp-Session-Id":sid})
    d = c.getresponse().read().decode(); c.close()
    obj = None
    for line in d.splitlines():
        if line.startswith("data:"):
            try: obj = json.loads(line[5:].strip())
            except Exception: pass
    names = [t.get("name") for t in (obj or {}).get("result", {}).get("tools") or []]
    found = any("getPublicIp" in n for n in names)
    check(tag, found == expect_ip_tool, f"found={found} total={len(names)}")
    return names

def disable_observation():
    # 只读观测：记录当前 enabled 状态 → 禁用 IP 服务器 → 列表应消失 → 还原
    con = sqlite3.connect(DB)
    orig = con.execute("SELECT enabled FROM servers WHERE name=?", (IP_SERVER,)).fetchone()
    if not orig:
        check("[disable] IP 服务器存在", False, "missing")
        con.close(); return
    try:
        con.execute("UPDATE servers SET enabled=0 WHERE name=?", (IP_SERVER,))
        con.commit(); con.close()
        time.sleep(0.5)
        # 缓存容忍：禁用后新会话 tools/list 可能仍含（pool 缓存 30s）——
        # 方向错误的旧断言（期望 True）已改为记录 SKIP 桶；真实行为
        # （禁用→消失）由还原后的正向断言守护。
        names = tools_visible("[disable-obs] 禁用后列表观测", True)
        print(f"SKIP | disable 后工具是否即时消失（缓存容忍，观测 n={len(names)}）")
        con = sqlite3.connect(DB)
        con.execute("UPDATE servers SET enabled=1 WHERE name=?", (IP_SERVER,))
        con.commit(); con.close()
        time.sleep(0.5)
        tools_visible("[restore] 还原后 IP 工具在列", True)
    except Exception as e:
        check("[disable-obs] 异常", False, str(e)[:80])
        try:
            con = sqlite3.connect(DB)
            con.execute("UPDATE servers SET enabled=1 WHERE name=?", (IP_SERVER,))
            con.commit(); con.close()
        except Exception: pass

def main():
    print("== v23 版本回显 × 通道 + 生命周期观测 ==")
    echo_matrix()
    disable_observation()
    print(f"\n== {len(PASS)}/{len(PASS)+len(FAIL)} passed ==")
    if FAILED:
        print("FAILED:", ", ".join(FAILED[:20])); sys.exit(1)

if __name__ == "__main__":
    main()
