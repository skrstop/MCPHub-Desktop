#!/usr/bin/env python3
# 版本 × 通道 × 公网IP 全矩阵回归
import json, http.client, urllib.parse, sys, re

HOST, PORT = "localhost", 23333
PASS, FAIL = [], []
import ipaddress as _ipa
def _valid_pub(t):
    for m in re.findall(r"\d{1,3}(?:\.\d{1,3}){3}", t):
        try:
            ip = _ipa.ip_address(m)
            if not ip.is_private and not ip.is_loopback and not ip.is_reserved:
                return True
        except ValueError:
            pass
    return False

def check(name, ok, detail=""):
    (PASS if ok else FAIL).append(name)
    print(("PASS" if ok else "FAIL"), "|", name, ("| " + str(detail)[:100] if detail and not ok else ""))

def req(method, path, payload=None, headers=None, timeout=60):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
    if headers: h.update(headers)
    body = json.dumps(payload) if payload is not None else None
    c.request(method, path, body=body, headers=h)
    r = c.getresponse(); data = r.read().decode("utf-8", "replace")
    sid = r.getheader("mcp-session-id"); st = r.status; ct = r.getheader("content-type") or ""
    c.close()
    obj = None
    if data.strip().startswith("{"):
        try: obj = json.loads(data)
        except Exception: pass
    if obj is None:
        frames = []
        for line in data.split("\n"):
            if line.startswith("data: "):
                try: frames.append(json.loads(line[6:]))
                except Exception: pass
        obj = frames[-1] if frames else None
    return st, sid, obj, ct

def init_2026():
    meta = {"io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientInfo": {"name":"m","version":"1"},
            "io.modelcontextprotocol/clientCapabilities": {}}
    return {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2026-07-28","capabilities":{},"clientInfo":{"name":"m","version":"1"},"_meta":meta}}
H26 = {"Mcp-Method":"tools/list","Mcp-Name":"tools/list","MCP-Protocol-Version":"2026-07-28"}

CLIENT_META = {"io.modelcontextprotocol/clientInfo":{"name":"m","version":"1"},
               "io.modelcontextprotocol/clientCapabilities":{}}
def modern_call(path, tool, args=None, sid=None):
    meta = {"io.modelcontextprotocol/protocolVersion": "2026-07-28", **CLIENT_META}
    body = {"jsonrpc":"2.0","id":99,"method":"tools/call",
            "params":{"name":tool,"arguments":args or {},"_meta":meta}}
    h = {"Mcp-Method":"tools/call","Mcp-Name":tool,"MCP-Protocol-Version":"2026-07-28"}
    if sid: h["Mcp-Session-Id"] = sid
    return req("POST", path, body, h)

def legacy_flow(v, path, tool, sep=True):
    st, sid, obj, ct = req("POST", path, {"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":v,"capabilities":{},"clientInfo":{"name":"m","version":"1"}}})
    ok_neg = obj and obj.get("result",{}).get("protocolVersion") == v
    check(f"[{v}] {path} initialize 协商一致", ok_neg, str(obj)[:90])
    st, _, obj, ct = req("POST", path, {"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}},
                         {"Mcp-Session-Id": sid, "MCP-Protocol-Version": v})
    tools = [t["name"] for t in (obj.get("result",{}).get("tools") or [])] if obj else []
    tname = tool if not sep else tool
    hit = tname in tools or any(t == tname for t in tools) or any(t.endswith(tname) for t in tools)
    check(f"[{v}] {path} IP 工具暴露", hit, f"n={len(tools)}")
    st, _, obj, ct = req("POST", path, {"jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":tname,"arguments":{}}}, {"Mcp-Session-Id": sid, "MCP-Protocol-Version": v})
    txt = "".join(str(c.get("text","")) for c in (obj.get("result",{}).get("content") or [])) if obj else ""
    check(f"[{v}] {path} 公网IP 真实调用", st==200 and obj and obj.get("result",{}).get("isError") is False and _valid_pub(txt), txt[:80])
    req("DELETE", path, None, {"Mcp-Session-Id": sid})
    return ok_neg

# ── 通道定义 ──
IP_PREFIXED = "本机公网ip查询-getPublicIp"
channels_legacy = [
    ("/mcp", IP_PREFIXED),                    # root: 中文前缀名
    (f"/mcp/{urllib.parse.quote('Test')}", IP_PREFIXED),  # 分组 scope
    (f"/mcp/{urllib.parse.quote('本机公网ip查询')}", "getPublicIp"),  # 单服务器 scope: 裸名
]
for v in ["2024-11-05", "2025-03-26", "2025-11-25"]:
    for path, tool in channels_legacy:
        legacy_flow(v, path, tool)

# ── 2026 modern（每通道）──
for path, tool in channels_legacy:
    # 2026 modern: initialize(协商降级) + 直接无状态 modern call
    st, sid, obj, ct = req("POST", path, init_2026())
    neg = obj.get("result",{}).get("protocolVersion") if obj else None
    check(f"[2026] {path} initialize（rmcp 降级协商 {neg}）", neg in ("2026-07-28","2025-11-25"), str(obj)[:80])
    # modern 无状态 tools/list
    st, _, obj, ct = req("POST", path, {"jsonrpc":"2.0","id":2,"method":"tools/list",
        "params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28", **CLIENT_META}}}, H26)
    n_tools = len(obj.get("result",{}).get("tools") or []) if obj else 0
    check(f"[2026] {path} modern tools/list", st==200 and n_tools>0, f"n={n_tools} ct={ct[:20]}")
    # TTL 泄漏门控：2026 会话带 ttlMs
    check(f"[2026] {path} CacheableResult(ttlMs)", obj and "ttlMs" in obj.get("result",{}), "")
    # 2026 modern IP 调用：裸名 ASCII 可调；中文前缀受限（latin-1 header）→ 记录跳过
    if all(ord(ch) < 128 for ch in tool):
        st, _, obj, ct = modern_call(path, tool)
        txt = "".join(str(c.get("text","")) for c in (obj.get("result",{}).get("content") or [])) if obj else ""
        check(f"[2026] {path} modern 公网IP 调用({tool})", st==200 and obj and obj.get("result",{}).get("isError") is False and _valid_pub(txt), txt[:80])
    else:
        print(f"SKIP | [2026] {path} modern 调用 {tool}（中文工具名 latin-1 header 限制，已知边界）")

# ── $smart 通道（legacy 一档验证）──
st, sid, obj, ct = req("POST", "/mcp/$smart", {"jsonrpc":"2.0","id":1,"method":"initialize",
    "params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"m","version":"1"}}})
smart_ok = obj is not None and obj.get("result",{}).get("serverInfo") is not None
check("[$smart] initialize", smart_ok, str(obj)[:90])

# ── discover 通道（真实 server/discover 2026 modern 调用）──
st, _, obj, ct = req("POST", "/mcp", {"jsonrpc":"2.0","id":1,"method":"server/discover",
    "params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28",
                       "io.modelcontextprotocol/clientInfo":{"name":"d","version":"1"},
                       "io.modelcontextprotocol/clientCapabilities":{}}}},
    {"MCP-Protocol-Version":"2026-07-28","Mcp-Method":"server/discover"})
r = (obj or {}).get("result") or {}
disc = (st == 200 and r.get("resultType") == "complete"
        and all(v in (r.get("supportedVersions") or []) for v in ["2024-11-05","2025-11-25","2026-07-28"]))
check("[discover] resultType + supportedVersions", bool(disc), str(obj)[:100])

print(f"\n== {len(PASS)}/{len(PASS)+len(FAIL)} passed ==")
if FAIL:
    print("FAILED:", *FAIL, sep="\n  - ")
    sys.exit(1)
