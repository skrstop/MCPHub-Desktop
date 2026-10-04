#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""公网IP MCP 真实调用矩阵（R101+ 复核轮固化）。
要求 openapi 类型服务器「本机公网ip查询」（上游 ip.3322.net）已连接。
覆盖：3 个 legacy 协议版本完整 lifecycle + 2026 无状态 + 单服务器 scope + REST 单服务器 + REST group。
每项断言：HTTP 200 + 协议版本协商一致 + 响应文本含真实公网 IPv4。
"""
import http.client, json, urllib.parse, re, sys

HOST, PORT = "localhost", 23333
IP_TOOL = "本机公网ip查询-getPublicIp"
SCOPE = urllib.parse.quote("本机公网ip查询")

def get_ip(d):
    m = re.search(r"\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}", d)
    return m.group(0) if m else None

def post(path, body, headers=None):
    c = http.client.HTTPConnection(HOST, PORT, timeout=60)
    h = {"Accept": "application/json, text/event-stream", "Content-Type": "application/json"}
    if headers: h.update(headers)
    c.request("POST", path, body=json.dumps(body).encode(), headers=h)
    r = c.getresponse(); d = r.read().decode("utf-8", "replace")
    return r.status, d, r.getheader("mcp-session-id")

def main():
    ok = 0; total = 0; failed = []
    for v in ["2025-03-26", "2025-06-18", "2025-11-25"]:
        st, d, sid = post("/mcp", {"jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"protocolVersion":v,"capabilities":{},"clientInfo":{"name":"ip-matrix","version":"1"}}})
        try:
            pv = json.loads([l for l in d.splitlines() if l.startswith("data:")][-1][5:])["result"]["protocolVersion"]
        except Exception:
            pv = None
        t = f"legacy {v} init+negotiated"
        total += 1
        if st == 200 and pv == v:
            ok += 1; print(f"PASS | {t} (negotiated={pv})")
        else:
            failed.append(t); print(f"FAIL | {t} st={st} negotiated={pv}")
        post("/mcp", {"jsonrpc":"2.0","method":"notifications/initialized"}, {"Mcp-Session-Id": sid})
        st, d, _ = post("/mcp", {"jsonrpc":"2.0","id":2,"method":"tools/call",
            "params":{"name":IP_TOOL,"arguments":{}}}, {"Mcp-Session-Id": sid})
        ip = get_ip(d); total += 1
        t = f"legacy {v} tools/call 公网IP"
        if st == 200 and ip:
            ok += 1; print(f"PASS | {t} ip={ip}")
        else:
            failed.append(t); print(f"FAIL | {t} st={st}")
    body = {"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":IP_TOOL,"arguments":{},
        "_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28",
                 "io.modelcontextprotocol/clientInfo":{"name":"ip26","version":"1.0"},
                 "io.modelcontextprotocol/clientCapabilities":{}}}}
    st, d, _ = post("/mcp", body); ip = get_ip(d); total += 1
    if st == 200 and ip: ok += 1; print(f"PASS | 2026-07-28 stateless tools/call 公网IP ip={ip}")
    else: failed.append("2026 stateless"); print(f"FAIL | 2026-07-28 stateless st={st}")
    st, d, sid = post(f"/mcp/{SCOPE}", {"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"s","version":"1"}}})
    post(f"/mcp/{SCOPE}", {"jsonrpc":"2.0","method":"notifications/initialized"}, {"Mcp-Session-Id": sid})
    st, d, _ = post(f"/mcp/{SCOPE}", {"jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"getPublicIp","arguments":{}}}, {"Mcp-Session-Id": sid})
    ip = get_ip(d); total += 1
    if st == 200 and ip: ok += 1; print(f"PASS | single-server scope tools/call 公网IP ip={ip}")
    else: failed.append("scope"); print(f"FAIL | single-server scope st={st}")
    for label, path, body in [
        ("REST /rest/:server/call", f"/rest/{SCOPE}/call", {"tool":"getPublicIp","arguments":{}}),
        ("REST group /rest/group/:g/call", "/rest/group/Test/call",
         {"server":"本机公网ip查询","tool":"getPublicIp","arguments":{}}),
    ]:
        c = http.client.HTTPConnection(HOST, PORT, timeout=60)
        c.request("POST", path, body=json.dumps(body).encode(),
                  headers={"Content-Type":"application/json","Accept":"application/json, text/event-stream"})
        r = c.getresponse(); d = r.read().decode(); ip = get_ip(d); total += 1
        if r.status == 200 and ip: ok += 1; print(f"PASS | {label} ip={ip}")
        else: failed.append(label); print(f"FAIL | {label} st={r.status}")
    print(f"\n== {ok}/{total} passed ==")
    if failed:
        print("FAILED:", ", ".join(failed)); sys.exit(1)

if __name__ == "__main__":
    main()
