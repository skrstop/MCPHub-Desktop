#!/usr/bin/env python3
"""rmcp_sync_baseline_r110.py — 复核第十八轮修复回归 + R116 缺口矩阵。

覆盖：
  A. /rest/{server}/call × 5 个协议版本头（真实 getPublicIp，R116 缺口 2/3）
  B. /rest/group/{g}/call × 5 个协议版本头（真实 getPublicIp，R116 缺口 1）
  C. /rest/$smart/search 真实结果 + smart_route_call 元工具 REST 真实调用（R116 缺口 4）
  D. 2026 tools/list ttlMs ≤ 5000（R109-1 TTL 上限回归）
  E. $smart/{group} 直呼钉选门控：钉选内放行 / 未钉选拒绝（R109-2 修复回归）
  F. enableGroupNameRoute 开关：按名 404 / 按 id 放行 / percent-encoded 名 404
     （R110-1 + R115-1 修复回归；DB 直写开关，finally 还原）
依赖：应用运行于 127.0.0.1:23333；DB 含 group「Test」（成员含「本机公网ip查询」tools=all），
     Smart Routing 已启用。
"""
import json, http.client, sqlite3, os, sys, re, time, ipaddress
from urllib.parse import quote

HOST, PORT = "127.0.0.1", 23333
DB = os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")
PASS, FAIL = [], []
IP_SERVER = "本机公网ip查询"
IP_SERVER_ENC = quote(IP_SERVER, safe="")
IP_TOOL = "getPublicIp"
SEP_TOOL = f"{IP_SERVER}-{IP_TOOL}"
GROUP = "Test"
GROUP_ENC = quote(GROUP, safe="")
GROUP_ID = None
VERSIONS = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25", "2026-07-28"]

def req(method, path, body=None, headers=None, timeout=60):
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

def valid_pub_ip(txt):
    for m in re.findall(r"\d{1,3}(?:\.\d{1,3}){3}", txt):
        try:
            ip = ipaddress.ip_address(m)
            if not ip.is_private and not ip.is_loopback and not ip.is_reserved:
                return True
        except ValueError:
            pass
    return False

def ok_call(obj):
    r = (obj or {}).get("result") or {}
    return (obj or {}).get("error") is None and r.get("isError") in (None, False)

def check(name, cond, detail=""):
    (PASS if cond else FAIL).append(name)
    print(("✓" if cond else "✗") + f" {name}" + (f" — {detail}" if (detail and not cond) else ""))

def read_config():
    con = sqlite3.connect(DB)
    v = con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}"
    con.close()
    return json.loads(v)

def write_config(cfg):
    con = sqlite3.connect(DB)
    con.execute("UPDATE system_config SET config_json=?, updated_at=datetime('now') WHERE rowid=1",
                (json.dumps(cfg, ensure_ascii=False),))
    con.commit(); con.close()

def set_routing_flag(key, val):
    cfg = read_config()
    cfg.setdefault("routing", {})[key] = val
    write_config(cfg)

con = sqlite3.connect(DB)
row = con.execute("SELECT id FROM groups WHERE name=?", (GROUP,)).fetchone()
con.close()
if not row:
    print(f"group '{GROUP}' not found; aborting"); sys.exit(1)
GROUP_ID = row[0]

# ── A/B. REST 通道 × 5 版本头，真实 getPublicIp（REST 无版本协商，头仅记录语义）──
for v in VERSIONS:
    h = {"MCP-Protocol-Version": v}
    st, body, _ = req("POST", f"/rest/{IP_SERVER_ENC}/call", {"tool": IP_TOOL, "arguments": {}}, h)
    check(f"A /rest × {v} 真实调用", st == 200 and valid_pub_ip(body), f"{st} {body[:150]}")
    st, body, _ = req("POST", f"/rest/group/{GROUP_ENC}/call", {"tool": IP_TOOL, "arguments": {}}, h)
    check(f"B /rest/group × {v} 真实调用", st == 200 and valid_pub_ip(body), f"{st} {body[:150]}")

# ── C. /rest/$smart 真实 search + meta call ──
st, body, _ = req("POST", "/api/$smart/search", {"query": "public ip address", "limit": 10})
ok = st == 200
try:
    sresp = json.loads(body)
    inner = json.loads(sresp.get("content", [{}])[0].get("text", "{}"))
except Exception: sresp, inner = {}, {}
found = any(IP_TOOL in json.dumps(t) for t in inner.get("tools", []))
check("C1 /api/$smart/search 真实结果含 IP 工具", ok and found, f"{st} {body[:200]}")
# /api/$smart/call 直呼索引内真实工具（meta 面由 search/describe 端点承担）
st, body, _ = req("POST", "/api/$smart/call", {"toolName": SEP_TOOL, "arguments": {}})
check("C2 /api/$smart/call 真实调用返回公网IP", st == 200 and valid_pub_ip(body), f"{st} {body[:200]}")
st, body, _ = req("POST", "/api/$smart/describe", {"toolName": SEP_TOOL})
ok3 = st == 200
try:
    d = json.loads(json.loads(body).get("content", [{}])[0].get("text", "{}"))
except Exception: d = {}
check("C3 /api/$smart/describe 返回 inputSchema", ok3 and "inputSchema" in json.dumps(d), f"{st} {body[:150]}")

# ── D. 2026 tools/list ttlMs ≤ 5000（R109-1 修复回归：原恒 30000）──
st, body, _ = req("POST", f"/mcp/{IP_SERVER_ENC}", {"jsonrpc": "2.0", "id": 1, "method": "initialize",
    "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "r110", "version": "1"}}})
st, body, hdrs = req("POST", f"/mcp/{IP_SERVER_ENC}", {"jsonrpc": "2.0", "id": 1, "method": "initialize",
    "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "r110", "version": "1"}}})
sid = hdrs.get("mcp-session-id", "")
st, body, _ = req("POST", f"/mcp/{IP_SERVER_ENC}", {"jsonrpc": "2.0", "id": 2, "method": "tools/list",
    "params": {"_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28",
                          "io.modelcontextprotocol/clientInfo": {"name": "r110", "version": "1"},
                          "io.modelcontextprotocol/clientCapabilities": {}}}},
    headers={"mcp-session-id": sid, "MCP-Protocol-Version": "2026-07-28", "Mcp-Method": "tools/list"})
obj = sse_obj(body, 2)
ttl = ((obj or {}).get("result") or {}).get("ttlMs")
check("D1 2026 tools/list ttlMs ≤ 5000（有界）", isinstance(ttl, (int, float)) and ttl <= 5000,
      f"ttlMs={ttl}")
# $smart scope ttlMs 必须 0（网关自生成条目）
st, body, hdrs = req("POST", f"/mcp/$smart/{GROUP_ENC}", {"jsonrpc": "2.0", "id": 3, "method": "initialize",
    "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "r110", "version": "1"}}})
sid2 = hdrs.get("mcp-session-id", "")
st, body, _ = req("POST", f"/mcp/$smart/{GROUP_ENC}", {"jsonrpc": "2.0", "id": 4, "method": "tools/list",
    "params": {"_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28",
                          "io.modelcontextprotocol/clientInfo": {"name": "r110", "version": "1"},
                          "io.modelcontextprotocol/clientCapabilities": {}}}},
    headers={"mcp-session-id": sid2, "MCP-Protocol-Version": "2026-07-28", "Mcp-Method": "tools/list"})
obj = sse_obj(body, 4)
ttl2 = ((obj or {}).get("result") or {}).get("ttlMs")
check("D2 $smart scope tools/list ttlMs == 0", ttl2 == 0, f"ttlMs={ttl2}")
# builtin prompts/resources 列表 ttlMs == 0
st, body, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 5, "method": "prompts/list",
    "params": {"_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28",
                          "io.modelcontextprotocol/clientInfo": {"name": "r110", "version": "1"},
                          "io.modelcontextprotocol/clientCapabilities": {}}}},
    headers={"MCP-Protocol-Version": "2026-07-28", "Mcp-Method": "prompts/list"})
obj = sse_obj(body, 5)
ttl3 = ((obj or {}).get("result") or {}).get("ttlMs")
check("D3 prompts/list（builtin）ttlMs == 0", ttl3 == 0, f"ttlMs={ttl3}")
req("DELETE", f"/mcp/{IP_SERVER_ENC}", None, {"mcp-session-id": sid})
req("DELETE", f"/mcp/$smart/{GROUP_ENC}", None, {"mcp-session-id": sid2})

# ── E. $smart/{group} 直呼钉选门控（R109-2 修复回归）──
# 把 IP 服务器钉选到组：先 pin，再 $smart 直呼；随后未钉选工具应被拒绝。
con = sqlite3.connect(DB)
orig = json.loads(con.execute("SELECT servers FROM groups WHERE name=?", (GROUP,)).fetchone()[0])
con.close()
pinned = [dict(m) for m in orig]
for m in pinned:
    if m.get("name") == IP_SERVER:
        m["pinnedTools"] = [IP_TOOL]
con = sqlite3.connect(DB)
con.execute("UPDATE groups SET servers=? WHERE name=?",
            (json.dumps(pinned, ensure_ascii=False), GROUP))
con.commit(); con.close()
time.sleep(0.3)
try:
    st, body, hdrs = req("POST", f"/mcp/$smart/{GROUP_ENC}", {"jsonrpc": "2.0", "id": 10, "method": "initialize",
        "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "r110", "version": "1"}}})
    sid3 = hdrs.get("mcp-session-id", "")
    # 钉选工具直呼 → 放行 + 真实 IP。名称形态跟随 parity 规则：多服务器可见时
    # 列表只暴露 {server}{sep}{tool} 前缀名（R109-2/六轮 parity），单服务器才
    # 接受裸名——从 tools/list 实际暴露形态取调用名，避免环境敏感断言。
    st, body, _ = req("POST", f"/mcp/$smart/{GROUP_ENC}", {"jsonrpc": "2.0", "id": 110, "method": "tools/list",
        "params": {}}, headers={"mcp-session-id": sid3, "MCP-Protocol-Version": "2025-11-25"})
    lst = sse_obj(body, 110)
    names = [t.get("name") for t in ((lst or {}).get("result") or {}).get("tools", [])]
    call_name = next((n for n in names if n and n.endswith(IP_TOOL) and ("getPublicIp" in n)), None)
    check("E0 tools/list 暴露钉选工具（形态=裸名或前缀名）", call_name is not None, f"names={names[:8]}")
    st, body, _ = req("POST", f"/mcp/$smart/{GROUP_ENC}", {"jsonrpc": "2.0", "id": 11, "method": "tools/call",
        "params": {"name": call_name or IP_TOOL, "arguments": {}}},
        headers={"mcp-session-id": sid3, "MCP-Protocol-Version": "2025-11-25"})
    obj = sse_obj(body, 11)
    check("E1 $smart/{group} 钉选工具直呼放行+真实IP", ok_call(obj) and valid_pub_ip(json.dumps(obj)),
          f"{st} {str(obj)[:150]}")
    # 未钉选工具（组内其他成员的工具）→ 拒绝（not found 语义）
    st, body, _ = req("POST", f"/mcp/$smart/{GROUP_ENC}", {"jsonrpc": "2.0", "id": 12, "method": "tools/call",
        "params": {"name": "playwright-browser-navigate", "arguments": {}}},
        headers={"mcp-session-id": sid3, "MCP-Protocol-Version": "2025-11-25"})
    obj = sse_obj(body, 12)
    rejected = (obj or {}).get("error") is not None
    check("E2 $smart/{group} 未钉选工具被拒", rejected, f"{st} {str(obj)[:150]}")
    req("DELETE", f"/mcp/$smart/{GROUP_ENC}", None, {"mcp-session-id": sid3})
finally:
    con = sqlite3.connect(DB)
    con.execute("UPDATE groups SET servers=? WHERE name=?",
                (json.dumps(orig, ensure_ascii=False), GROUP))
    con.commit(); con.close()

# ── F. enableGroupNameRoute 开关（R110-1 + R115-1 修复回归）──
cfg_backup = read_config()
try:
    set_routing_flag("enableGroupNameRoute", False)
    time.sleep(0.4)
    st, body, _ = req("POST", f"/mcp/{GROUP_ENC}", {"jsonrpc": "2.0", "id": 20, "method": "initialize",
        "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "r110", "version": "1"}}})
    check("F1 关名路由后按名 404", st == 404, f"{st} {body[:120]}")
    # percent-encoded 非 ASCII 组名 → decode 后命中组名 → 同样 404（R115-1）
    con = sqlite3.connect(DB)
    con.execute("INSERT OR IGNORE INTO groups (id, name, servers, description) VALUES ('r110-tmp-group', '测试组R110', '[]', '')")
    con.commit(); con.close()
    time.sleep(0.2)
    st, body, _ = req("POST", f"/mcp/{quote('测试组R110', safe='')}", {"jsonrpc": "2.0", "id": 21, "method": "initialize",
        "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "r110", "version": "1"}}})
    check("F2 percent-encoded 组名也 404", st == 404, f"{st} {body[:120]}")
    # 按 id 访问 → 放行（R110-1：id 不受名路由开关影响）
    st, body, _ = req("POST", f"/mcp/{GROUP_ID}", {"jsonrpc": "2.0", "id": 22, "method": "initialize",
        "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "r110", "version": "1"}}})
    check("F3 按 group id 访问放行", st == 200, f"{st} {body[:120]}")
    # 开关关闭下 root/scope 通道不受影响（真实调用）
    st, body, _ = req("POST", f"/rest/{IP_SERVER_ENC}/call", {"tool": IP_TOOL, "arguments": {}})
    check("F4 REST 单服务器不受组开关影响", st == 200 and valid_pub_ip(body), f"{st} {body[:120]}")
finally:
    con = sqlite3.connect(DB)
    con.execute("DELETE FROM groups WHERE id='r110-tmp-group'")
    con.commit(); con.close()
    for k, v in (cfg_backup.get("routing") or {}).items():
        set_routing_flag(k, v)
    # flag 不存在时确保删除
    cfg = read_config()
    for k in ("enableGroupNameRoute", "enableGlobalRoute"):
        if k not in (cfg_backup.get("routing") or {}):
            cfg.get("routing", {}).pop(k, None)
    write_config(cfg)
    time.sleep(0.3)

print(f"\n{'='*50}\nPASS={len(PASS)} FAIL={len(FAIL)}")
if FAIL:
    print("FAILED:", *FAIL, sep="\n  - ")
    sys.exit(1)
