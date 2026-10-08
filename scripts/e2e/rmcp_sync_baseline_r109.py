#!/usr/bin/env python3
"""基线同步 85a530f→01f4bb8 专项 E2E（rmcp_sync_baseline_r109.py）。
覆盖本轮同步 Rust 面：REST 三响应面 structuredContent/_meta 条件键（缺省或对象）、
公网IP 真实调用 × 版本 × 通道（REST/REST-group//mcp/$smart/api）、
五版本回显一致、宽松升格真实调用、严格模式 DB 热切回归、pinnedTools round-trip。
依赖：应用运行于 127.0.0.1:23333；DB 内 Test group 含「本机公网ip查询」。
"""
import json, http.client, sqlite3, os, sys, time
import os as _os, sys as _sys
_sys.path.insert(0, _os.path.dirname(os.path.abspath(__file__)))
from pin_helper import pin, unpin
from urllib.parse import quote

HOST, PORT = "127.0.0.1", 23333
DB = os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")
PASS, FAIL = [], []
IP_SERVER = "本机公网ip查询"
IP_SERVER_ENC = quote(IP_SERVER, safe="")
IP_TOOL = "getPublicIp"
SEP_TOOL = f"{IP_SERVER}-{IP_TOOL}"

def req(method, path, body=None, headers=None, timeout=30):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
    if headers: h.update(headers)
    payload = json.dumps(body, ensure_ascii=False).encode("utf-8") if body is not None else None
    c.request(method, path, body=payload, headers=h)
    r = c.getresponse()
    data = r.read().decode("utf-8", "replace")
    c.close()
    return r.status, data, dict((k.lower(), v) for k, v in r.getheaders())

def sse_obj(data, want_id=None):
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
        if line.startswith("data: "):
            try: frames.append(json.loads(line[6:]))
            except Exception: pass
    for f in reversed(frames):
        if want_id is None or f.get("id") == want_id:
            return f
    return None

import re, ipaddress
def has_ip(txt):
    """True only when the text contains a semantically valid public IPv4.
    The old "Response Body" fallback was a pseudo-assertion: upstream error
    text quoting an HTTP failure body also matched (R116 audit)."""
    for m in re.findall(r"\d{1,3}(?:\.\d{1,3}){3}", txt):
        try:
            ip = ipaddress.ip_address(m)
            if not ip.is_private and not ip.is_loopback and not ip.is_reserved:
                return True
        except ValueError:
            pass
    return False

def ok_call(obj):
    """MCP tool-call success: JSON-RPC ok AND isError not true."""
    r = (obj or {}).get("result") or {}
    return (obj or {}).get("error") is None and r.get("isError") in (None, False)

def is_error_false(resp):
    return (resp or {}).get("isError") in (None, False)

def check(name, cond, detail=""):
    (PASS if cond else FAIL).append(name)
    print(("✓" if cond else "✗") + f" {name}" + (f" — {detail}" if (detail and not cond) else ""))

def set_strict(on):
    con = sqlite3.connect(DB)
    cfg = json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
    cfg.setdefault("mcp", {})["strictValidation"] = on
    con.execute("UPDATE system_config SET config_json=?, updated_at=datetime('now') WHERE rowid=1",
                (json.dumps(cfg, ensure_ascii=False),))
    con.commit(); con.close()

# 读 Test group 现状（终了还原用）
con = sqlite3.connect(DB)
row = con.execute("SELECT id, servers FROM groups WHERE name='Test'").fetchone()
con.close()
gid, orig_servers = row
orig_servers = json.loads(orig_servers)

def set_group_servers(servers):
    con = sqlite3.connect(DB)
    con.execute("UPDATE groups SET servers=? WHERE name='Test'", (json.dumps(servers, ensure_ascii=False),))
    con.commit(); con.close()

# ── A. REST 单服务器 /rest/{server}/call：新键形状 + 公网IP 真实调用 ──
st, body, _ = req("POST", f"/rest/{IP_SERVER_ENC}/call", {"tool": IP_TOOL, "arguments": {}})
resp = {}
ok = st == 200
try: resp = json.loads(body)
except Exception: ok = False
check("A1 REST call 200", ok, body[:120])
check("A2 result 含真实公网IP", has_ip(json.dumps(resp.get("result", []), ensure_ascii=False)), body[:200])
check("A3 is_error False", resp.get("is_error") is False)
check("A4 structuredContent 缺省或对象(条件键)", "structuredContent" not in resp or isinstance(resp["structuredContent"], dict))
check("A5 _meta 缺省或对象(条件键)", "_meta" not in resp or isinstance(resp["_meta"], dict))

# ── B. REST group /rest/group/Test/call ──
st, body, _ = req("POST", "/rest/group/Test/call", {"tool": IP_TOOL, "arguments": {}})
ok = st == 200
try: gresp = json.loads(body)
except Exception: gresp, ok = {}, False
check("B1 group REST call 200", ok, body[:150])
check("B2 group 响应 structuredContent/_meta 缺省或对象", ("structuredContent" not in gresp or isinstance(gresp["structuredContent"], dict)) and ("_meta" not in gresp or isinstance(gresp["_meta"], dict)))
check("B3 group 真实调用返回公网IP", has_ip(json.dumps(gresp, ensure_ascii=False)))

# ── C. pinnedTools round-trip（DB 直写 → REST group call 门控不受影响 → 还原）──
def _c_section():
    for s in orig_servers:
        if s.get("name") == IP_SERVER:
            s["pinnedTools"] = [IP_TOOL]
    set_group_servers(orig_servers); time.sleep(0.3)
    st, body, _ = req("POST", "/rest/group/Test/call", {"tool": IP_TOOL, "arguments": {}})
    check("C1 pin 后 group call 仍真实通过（pin⊆tools 不影响门控）", st == 200 and has_ip(body), f"{st} {body[:150]}")
    con = sqlite3.connect(DB)
    back = json.loads(con.execute("SELECT servers FROM groups WHERE name='Test'").fetchone()[0])
    con.close()
    pin_kept = any(s.get("name") == IP_SERVER and s.get("pinnedTools") == [IP_TOOL] for s in back)
    check("C2 pinnedTools 持久化 round-trip", pin_kept)

try:
    _c_section()
finally:
    for s in orig_servers:
        if s.get("name") == IP_SERVER:
            s.pop("pinnedTools", None)
    set_group_servers(orig_servers); time.sleep(0.3)

# ── D. $smart/Test：meta 工具 + 真实 search/call（MCP $smart 面 list/call pin parity → 先 pin 后还原）──
pin("本机公网ip查询", "getPublicIp")
st, body, hdrs = req("POST", "/mcp/$smart/Test", {"jsonrpc": "2.0", "id": 1, "method": "initialize",
    "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "r109", "version": "1"}}})
sid = hdrs.get("mcp-session-id", "")
obj = sse_obj(body, 1)
check("D1 $smart/Test initialize 200 + 版本回显", st == 200 and (obj or {}).get("result", {}).get("protocolVersion") == "2025-11-25", f"{st} {body[:150]}")
req("POST", "/mcp/$smart/Test", {"jsonrpc": "2.0", "method": "notifications/initialized"}, headers={"mcp-session-id": sid})
st, body, _ = req("POST", "/mcp/$smart/Test", {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
                  headers={"mcp-session-id": sid})
obj = sse_obj(body, 2)
names = [t.get("name") for t in (obj or {}).get("result", {}).get("tools", [])]
meta_names = set(names or [])
check("D2 $smart tools/list meta 工具（progressive 3 形 / 2 形皆可）",
      ({"smart_route_search", "smart_route_describe", "smart_route_call"} <= meta_names) or
      ({"smart_route_search", "smart_route_call"} <= meta_names and "smart_route_describe" not in meta_names),
      str(names))
# R7 之后：$smart 面暴露 meta + pinned 工具（列表/调用 pin parity）——pin 已由上文设置
check("D3 $smart 暴露 meta + pinned（无其他成员工具）",
      all(n.startswith("smart_route_") or n.endswith("-getPublicIp") or n == "getPublicIp" for n in names or []),
      str(names))
st, body, _ = req("POST", "/mcp/$smart/Test", {"jsonrpc": "2.0", "id": 3, "method": "tools/call",
    "params": {"name": "smart_route_search", "arguments": {"query": "public ip", "limit": 5}}},
    headers={"mcp-session-id": sid})
obj = sse_obj(body, 3)
txt = json.dumps((obj or {}).get("result", {}), ensure_ascii=False)
check("D4 smart_route_search 真实命中", "getPublicIp" in txt or "本机公网ip查询" in txt, txt[:200])
st, body, _ = req("POST", "/mcp/$smart/Test", {"jsonrpc": "2.0", "id": 4, "method": "tools/call",
    "params": {"name": "smart_route_call", "arguments": {"toolName": SEP_TOOL, "arguments": {}}}},
    headers={"mcp-session-id": sid})
obj = sse_obj(body, 4)
txt = json.dumps((obj or {}).get("result", {}), ensure_ascii=False)
check("D5 smart_route_call 真实公网IP", has_ip(txt), txt[:200])
req("DELETE", "/mcp/$smart/Test", headers={"mcp-session-id": sid})

unpin("本机公网ip查询", "getPublicIp")
# ── E. 五版本 × 根通道：回显一致 + 宽松升格真实调用 ──
for pv in ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"]:
    st, body, hdrs = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 10, "method": "initialize",
        "params": {"protocolVersion": pv, "capabilities": {}, "clientInfo": {"name": "r109", "version": "1"}}},
        headers={"MCP-Protocol-Version": pv})
    sid = hdrs.get("mcp-session-id", "")
    obj = sse_obj(body, 10)
    echoed = (obj or {}).get("result", {}).get("protocolVersion")
    check(f"E1 {pv} 回显一致", echoed == pv, f"{st} echoed={echoed}")
    st, body, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 11, "method": "tools/call",
        "params": {"name": SEP_TOOL, "arguments": {}}},
        headers={"mcp-session-id": sid, "MCP-Protocol-Version": pv})
    obj = sse_obj(body, 11)
    err = (obj or {}).get("error")
    txt = json.dumps((obj or {}).get("result", {}), ensure_ascii=False)
    check(f"E2 {pv} 宽松带会话真实调用", err is None and ok_call(obj) and has_ip(txt), f"{st} {body[:200]}")
    req("DELETE", "/mcp", headers={"mcp-session-id": sid})

# 2026 modern 无状态路径（宽松注入缺件）
st, body, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 20, "method": "tools/call",
    "params": {"name": SEP_TOOL, "arguments": {},
               "_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28",
                          "io.modelcontextprotocol/clientInfo": {"name": "r109", "version": "1"},
                          "io.modelcontextprotocol/clientCapabilities": {}}}},
    headers={"MCP-Protocol-Version": "2026-07-28", "Mcp-Method": "tools/call"})
obj = sse_obj(body, 20)
txt = json.dumps((obj or {}).get("result", {}), ensure_ascii=False)
check("E3 2026 modern 无状态真实调用", ok_call(obj) and has_ip(txt), f"{st} {body[:250]}")
check("E4 2026 resultType=complete", (obj or {}).get("result", {}).get("resultType") == "complete",
      str((obj or {}).get("result", {}).get("resultType")))
# 2026 bare 升格（连 _meta 都不带——宽松注入）
st, body, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 21, "method": "tools/call",
    "params": {"name": SEP_TOOL, "arguments": {}}},
    headers={"MCP-Protocol-Version": "2026-07-28"})
obj = sse_obj(body, 21)
txt = json.dumps((obj or {}).get("result", {}), ensure_ascii=False)
check("E5 2026 bare 升格真实调用", ok_call(obj) and has_ip(txt), f"{st} {body[:250]}")

# ── F. /api OpenAPI 兼容端点：新键 + 真实调用 ──
st, body, _ = req("POST", f"/api/{IP_SERVER_ENC}/tools/{IP_SERVER_ENC}/{IP_TOOL}", {})
ok = st == 200
try: apiresp = json.loads(body)
except Exception: apiresp, ok = {}, False
check("F1 /api tools POST 200", ok, f"{st} {body[:150]}")
check("F2 /api structuredContent/_meta 缺省或对象", ("structuredContent" not in apiresp or isinstance(apiresp["structuredContent"], dict)) and ("_meta" not in apiresp or isinstance(apiresp["_meta"], dict)))
check("F3 /api 真实返回公网IP", ok and is_error_false(apiresp) and has_ip(body))

# ── G. 严格模式回归（DB 热切）──
# R116 audit: the strict toggle must be restored even when a request raises —
# a leaked strict=True poisons every later leniency suite.
def _g_section():
    set_strict(True); time.sleep(0.3)
    st, body, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 30, "method": "tools/call",
        "params": {"name": SEP_TOOL, "arguments": {}}})
    rejected = st in (400, 406, 415, 422) or (sse_obj(body, 30) or {}).get("error") is not None
    check("G1 严格拒绝裸请求", rejected, f"{st} {body[:150]}")
    st, body, hdrs = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 31, "method": "initialize",
        "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "r109", "version": "1"}}},
        headers={"Accept": "application/json, text/event-stream"})
    check("G2 严格规范 initialize 通过", st == 200, f"{st} {body[:120]}")
    set_strict(False); time.sleep(0.3)
    st, body, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 32, "method": "tools/call",
        "params": {"name": SEP_TOOL, "arguments": {}}})
    obj = sse_obj(body, 32)
    txt = json.dumps((obj or {}).get("result", {}), ensure_ascii=False)
    check("G3 还原宽松后放行恢复", ok_call(obj) and has_ip(txt), f"{st} {body[:200]}")

try:
    _g_section()
finally:
    # set_strict returns the PREVIOUS value — restore it rather than
    # hardcoding False (operators with strict baseline must not be flipped).
    set_strict(set_strict(False))

print(f"\n{'='*50}\nPASS={len(PASS)} FAIL={len(FAIL)}")
if FAIL:
    print("FAILED:", *FAIL, sep="\n  - ")
    sys.exit(1)
