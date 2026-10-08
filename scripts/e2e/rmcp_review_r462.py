#!/usr/bin/env python3
"""复核第十九轮新增 E2E（rmcp_review_r462.py）。

每协议版本 × 每上游传输方式（stdio=codegraph / streamable-http=Idea-mcp-server /
openapi=本机公网ip查询）× 单服务器通道：真实 tools/call + 版本回显一致性。
openapi 通道走公网IP 查询（真实外呼）。宽松模式默认开启。
依赖：应用运行于 127.0.0.1:23333；playwright/codegraph/本机公网ip查询/Idea-mcp-server 已启用。
"""
import json, http.client, sys
from urllib.parse import quote

HOST, PORT = "127.0.0.1", 23333
PASS, FAIL = [], []
VERSIONS = ["2025-03-26", "2025-06-18", "2025-11-25", "2026-07-28"]
UPSTREAMS = [
    # (server, tool, args, checker)
    ("codegraph", "codegraph_status", {}, "any"),   # stdio 上游：无参真实调用
    ("Idea-mcp-server", "xdebug_get_threads", {}, "any"),
    ("本机公网ip查询", "getPublicIp", {}, "ip"),
]
import re
# No \b word-boundaries: inside json.dumps output a newline is the literal
# two chars backslash+n, and "n" is a word char — \b before a digit following
# it never matches (carpet R200 harness lesson).
IP_RE = re.compile(r"\d{1,3}(?:\.\d{1,3}){3}")

def req(method, path, body=None, headers=None, timeout=90, default_accept=True):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {}
    if default_accept:
        h["Accept"] = "application/json, text/event-stream"
    if body is not None:
        h["Content-Type"] = "application/json"
    if headers: h.update(headers)
    payload = json.dumps(body, ensure_ascii=False).encode("utf-8") if body is not None else None
    c.request(method, path, body=payload, headers=h)
    r = c.getresponse()
    data = r.read().decode("utf-8", "replace")
    c.close()
    return r.status, data, dict((k.lower(), v) for k, v in r.getheaders())

def sse_last(data, want_id=None):
    body = data.strip()
    if body.startswith("{"):
        try:
            o = json.loads(body)
            if want_id is None or o.get("id") == want_id:
                return o
        except Exception:
            pass
    frames = []
    for line in data.split("\n"):
        if line.startswith("data: ") and line[6:].strip():
            try: frames.append(json.loads(line[6:]))
            except Exception: pass
    for f in reversed(frames):
        if want_id is None or f.get("id") == want_id:
            return f
    return None

def unwrap_text(resp):
    v = resp
    for _ in range(4):
        if isinstance(v, dict) and "result" in v and isinstance(v["result"], dict):
            v = v["result"]
        if isinstance(v, dict) and isinstance(v.get("content"), list) and v["content"]:
            try: v = json.loads(v["content"][0]["text"])
            except Exception: break
        else: break
    return v

def check(name, cond, detail=""):
    (PASS if cond else FAIL).append(name)
    print(("PASS" if cond else "FAIL"), name, detail if not cond else "")

def first_tool(server):
    enc = quote(server, safe="")
    st, data, hd = req("POST", f"/mcp/{enc}", {
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "r462", "version": "1"}}})
    o = sse_last(data, 1)
    sid = hd.get("mcp-session-id")
    if not o or not sid:
        return None, None
    req("POST", f"/mcp/{enc}", {"jsonrpc": "2.0", "method": "notifications/initialized"},
        headers={"mcp-session-id": sid})
    st, data, _ = req("POST", f"/mcp/{enc}", {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
                      headers={"mcp-session-id": sid})
    o = sse_last(data, 2)
    names = [t["name"] for t in o.get("result", {}).get("tools", [])]
    req("DELETE", f"/mcp/{enc}", headers={"mcp-session-id": sid})
    return (names[0] if names else None), names

# 预取各上游首个工具名
tool_for = {}
for server, tool, args, kind in UPSTREAMS:
    if tool is None:
        t, names = first_tool(server)
        tool_for[server] = t
        print(f"# {server} tools={len(names or [])} first={t}")
    else:
        tool_for[server] = tool

for v in VERSIONS:
    for server, tool, args, kind in UPSTREAMS:
        tname = tool or tool_for[server]
        enc = quote(server, safe="")
        st, data, hd = req("POST", f"/mcp/{enc}", {
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": v, "capabilities": {},
                       "clientInfo": {"name": "r462", "version": "1"}}})
        o = sse_last(data, 1)
        sid = hd.get("mcp-session-id")
        if not o or not sid:
            check(f"M-{v}-{server} initialize", False, f"st={st}")
            continue
        echoed = o.get("result", {}).get("protocolVersion")
        check(f"M-{v}-{server} version echo", echoed in (v, "2025-11-25"), f"echo={echoed}")
        req("POST", f"/mcp/{enc}", {"jsonrpc": "2.0", "method": "notifications/initialized"},
            headers={"mcp-session-id": sid})
        st, data, _ = req("POST", f"/mcp/{enc}", {
            "jsonrpc": "2.0", "id": "call", "method": "tools/call",
            "params": {"name": tname, "arguments": args}},
            headers={"mcp-session-id": sid}, timeout=90)
        o = sse_last(data, "call")
        txt = json.dumps(unwrap_text(o), ensure_ascii=False) if o else ""
        ok = False
        if kind == "ip":
            import ipaddress as _ip
            _ipok = False
            for m in IP_RE.findall(txt):
                try:
                    a = _ip.ip_address(m)
                    if a.version == 4 and not (a.is_private or a.is_loopback or a.is_reserved):
                        _ipok = True; break
                except ValueError:
                    pass
            o_err = bool(o and isinstance(o, dict) and (o.get("result", {}) or {}).get("isError"))
            ok = _ipok and not o_err
        elif kind == "list":
            ok = st == 200 and o is not None and "isError\": true" not in txt and len(txt) > 10
        else:
            ok = st == 200 and o is not None and not txt.startswith('{"jsonrpc"')
        check(f"M-{v}-{server} real tools/call via {kind}", ok, f"st={st} o={str(o)[:80]} txt={txt[:400]}")
        req("DELETE", f"/mcp/{enc}", headers={"mcp-session-id": sid})

print(f"\n== {len(PASS)}/{len(PASS)+len(FAIL)} passed, {len(FAIL)} failed ==")
if FAIL:
    print("FAILED:", *FAIL, sep="\n  ")
sys.exit(1 if FAIL else 0)
