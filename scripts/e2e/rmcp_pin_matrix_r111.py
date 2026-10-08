#!/usr/bin/env python3
"""服务器级 pin 特性协议矩阵 E2E（rmcp_pin_matrix_r111.py）。

覆盖：
  A. 服务器级 pin DB 直写（server_tool_config.pinned=1，测毕还原）×
     根 $smart / $smart/Test 组作用域（列表含 pin、前缀直调、bare 拒绝、meta 工具）
  B. 五协议版本矩阵：initialize 回显 + 根 $smart pin 暴露 + 前缀直调
  C. 未 pin 时根 $smart 不暴露该工具
  D. REST /rest/{server}/call 通道独立性（pin 有/无都 200）
  E. 门禁负路径：伪造前缀 {server}-{不存在工具} 被拒
  F. 宽松模式：无 Accept 头裸请求升格后直调 pin 工具
依赖：应用运行于 127.0.0.1:23333；DB 内 Test group 含「本机公网ip查询」。
"""
import json, http.client, sqlite3, os, sys, time, re, ipaddress
from urllib.parse import quote

HOST, PORT = "127.0.0.1", 23333
DB = os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")
PASS, FAIL = [], []
IP_SERVER = "本机公网ip查询"
IP_SERVER_ENC = quote(IP_SERVER, safe="")
IP_TOOL = "getPublicIp"
SEP_TOOL = f"{IP_SERVER}-{IP_TOOL}"
PIN_UUID = "e2e-r111-pin-fixed-00000000-0001"

def req(method, path, body=None, headers=None, timeout=60, with_default_accept=True):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {}
    if with_default_accept:
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

def has_ip(txt):
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

# ── pin DB 直写（先查原状态，测毕还原）──
def _pin_state():
    con = sqlite3.connect(DB)
    row = con.execute(
        "SELECT pinned, enabled FROM server_tool_config WHERE server_name=? AND item_type='tool' AND item_name=?",
        (IP_SERVER, IP_TOOL)).fetchone()
    con.close()
    return row

def set_pin(on):
    con = sqlite3.connect(DB)
    if on:
        con.execute(
            "INSERT OR REPLACE INTO server_tool_config (id, server_name, item_type, item_name, enabled, pinned) "
            "VALUES (?, ?, 'tool', ?, 1, 1)", (PIN_UUID, IP_SERVER, IP_TOOL))
    else:
        con.execute("DELETE FROM server_tool_config WHERE id=?", (PIN_UUID,))
    con.commit(); con.close()

ORIG_PIN = _pin_state()  # None | (pinned, enabled)

def restore_pin():
    if ORIG_PIN is None:
        set_pin(False)
    else:
        con = sqlite3.connect(DB)
        con.execute(
            "UPDATE server_tool_config SET pinned=?, enabled=?, updated_at=datetime('now') "
            "WHERE server_name=? AND item_type='tool' AND item_name=?",
            (ORIG_PIN[0], ORIG_PIN[1], IP_SERVER, IP_TOOL))
        con.commit(); con.close()

def init_smart(path, pv, rid):
    st, body0, hdrs0 = req("POST", path, {"jsonrpc": "2.0", "id": rid, "method": "initialize",
        "params": {"protocolVersion": pv, "capabilities": {}, "clientInfo": {"name": "r111", "version": "1"}}})
    sid = hdrs0.get("mcp-session-id", "")
    obj = sse_obj(body0, rid)
    if sid:
        req("POST", path, {"jsonrpc": "2.0", "method": "notifications/initialized"},
            headers={"mcp-session-id": sid})
    return st, sid, obj, body0

def list_tools(path, sid, rid, pv):
    st, body, _ = req("POST", path, {"jsonrpc": "2.0", "id": rid, "method": "tools/list", "params": {}},
                      headers={"mcp-session-id": sid, "MCP-Protocol-Version": pv})
    obj = sse_obj(body, rid)
    names = [t.get("name") for t in ((obj or {}).get("result") or {}).get("tools", [])]
    return st, names, obj

# ═══════════ A. 服务器级 pin × 作用域 ═══════════
try:
    set_pin(True); time.sleep(0.3)

    # A1–A3 根 $smart（2025-11-25 会话）
    st, sid, obj, _ibody = init_smart("/mcp/$smart", "2025-11-25", 1)
    check("A0 根$smart initialize 200", st == 200 and ((obj or {}).get("result") or {}).get("serverInfo") is not None,
          f"{st} {str(_ibody)[:150]}" if st != 200 else str(obj)[:120])
    st, names, _ = list_tools("/mcp/$smart", sid, 2, "2025-11-25")
    check("A1 根$smart tools/list 含前缀 pin 名", SEP_TOOL in (names or []), str(names)[:300])
    st, body, _ = req("POST", "/mcp/$smart", {"jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": {"name": SEP_TOOL, "arguments": {}}}, headers={"mcp-session-id": sid})
    obj = sse_obj(body, 3)
    txt = json.dumps((obj or {}).get("result", {}), ensure_ascii=False)
    check("A2 根$smart 前缀直调返回真实公网IP", ok_call(obj) and has_ip(txt), f"{st} {body[:250]}")
    st, body, _ = req("POST", "/mcp/$smart", {"jsonrpc": "2.0", "id": 4, "method": "tools/call",
        "params": {"name": IP_TOOL, "arguments": {}}}, headers={"mcp-session-id": sid})
    obj = sse_obj(body, 4)
    check("A3 根$smart bare 名直调被拒（多服务器作用域）", (obj or {}).get("error") is not None, f"{st} {body[:200]}")

    # A4–A5 $smart/Test 组作用域（union：组员 pin ∪ 服务器级 pin）
    st, gsid, obj, _ibody = init_smart("/mcp/$smart/Test", "2025-11-25", 5)
    check("A4a $smart/Test initialize 200", st == 200, f"{st} {str(_ibody)[:150]}")
    st, gnames, _ = list_tools("/mcp/$smart/Test", gsid, 6, "2025-11-25")
    check("A4 $smart/Test 列表含该 pin（union）", SEP_TOOL in (gnames or []), str(gnames)[:300])
    st, body, _ = req("POST", "/mcp/$smart/Test", {"jsonrpc": "2.0", "id": 7, "method": "tools/call",
        "params": {"name": SEP_TOOL, "arguments": {}}}, headers={"mcp-session-id": gsid})
    obj = sse_obj(body, 7)
    txt = json.dumps((obj or {}).get("result", {}), ensure_ascii=False)
    check("A5 $smart/Test 前缀直调成功", ok_call(obj) and has_ip(txt), f"{st} {body[:250]}")

    # A6 meta 工具在两作用域仍正常
    for label, path, s in [("根$smart", "/mcp/$smart", sid), ("$smart/Test", "/mcp/$smart/Test", gsid)]:
        st, body, _ = req("POST", path, {"jsonrpc": "2.0", "id": 8, "method": "tools/call",
            "params": {"name": "smart_route_search", "arguments": {"query": "public ip", "limit": 5}}},
            headers={"mcp-session-id": s})
        obj = sse_obj(body, 8)
        t = json.dumps((obj or {}).get("result", {}), ensure_ascii=False)
        check(f"A6 meta smart_route_search {label} 正常", ok_call(obj) and ("getPublicIp" in t or IP_SERVER in t),
              f"{st} {t[:200]}")
    req("DELETE", "/mcp/$smart", headers={"mcp-session-id": sid})
    req("DELETE", "/mcp/$smart/Test", headers={"mcp-session-id": gsid})

    # ═══════════ B. 版本矩阵（根 $smart）═══════════
    for i, pv in enumerate(["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"]):
        base = 10 + i * 10
        st, vsid, obj, _ibody = init_smart("/mcp/$smart", pv, base)
        echoed = ((obj or {}).get("result") or {}).get("protocolVersion")
        check(f"B1 [{pv}] initialize 回显一致", echoed == pv, f"{st} echoed={echoed}")
        st, vnames, _ = list_tools("/mcp/$smart", vsid, base + 1, pv)
        check(f"B2 [{pv}] 根$smart 列表含 pin", SEP_TOOL in (vnames or []), str(vnames)[:250])
        st, body, _ = req("POST", "/mcp/$smart", {"jsonrpc": "2.0", "id": base + 2, "method": "tools/call",
            "params": {"name": SEP_TOOL, "arguments": {}}}, headers={"mcp-session-id": vsid})
        obj = sse_obj(body, base + 2)
        t = json.dumps((obj or {}).get("result", {}), ensure_ascii=False)
        # 2024-11-05 会话存在 resultType 剥离等版本差异，断言放宽到 isError 字段
        cond = (obj or {}).get("error") is None and (((obj or {}).get("result") or {}).get("isError") in (None, False))
        check(f"B3 [{pv}] 前缀直调成功(isError 放宽)", cond and (pv != "2024-11-05" or has_ip(t)),
              f"{st} {t[:200]}")
        req("DELETE", "/mcp/$smart", headers={"mcp-session-id": vsid})

    # 2026 modern：initialize _meta + server/discover 现代路径 + modern 直调
    H26 = {"Mcp-Method": "tools/list", "Mcp-Name": "tools/list", "MCP-Protocol-Version": "2026-07-28"}
    CLIENT_META = {"io.modelcontextprotocol/clientInfo": {"name": "r111", "version": "1"},
                   "io.modelcontextprotocol/clientCapabilities": {}}
    meta26 = {"io.modelcontextprotocol/protocolVersion": "2026-07-28", **CLIENT_META}
    st, body, hdrs = req("POST", "/mcp/$smart", {"jsonrpc": "2.0", "id": 50, "method": "initialize",
        "params": {"protocolVersion": "2026-07-28", "capabilities": {},
                   "clientInfo": {"name": "r111", "version": "1"}, "_meta": meta26}})
    sid26 = hdrs.get("mcp-session-id", "")
    obj = sse_obj(body, 50)
    neg = ((obj or {}).get("result") or {}).get("protocolVersion")
    check("B1 [2026-07-28] initialize（rmcp 降级协商）", neg in ("2026-07-28", "2025-11-25"), f"{st} neg={neg}")
    req("POST", "/mcp/$smart", {"jsonrpc": "2.0", "method": "notifications/initialized"},
        headers={"mcp-session-id": sid26})
    # modern server/discover 验证能力面
    st, body, _ = req("POST", "/mcp/$smart", {"jsonrpc": "2.0", "id": 51, "method": "server/discover",
        "params": {"_meta": meta26}}, {"MCP-Protocol-Version": "2026-07-28", "Mcp-Method": "server/discover"})
    obj = sse_obj(body, 51)
    d = (obj or {}).get("result") or {}
    check("B2 [2026-07-28] server/discover resultType=complete",
          st == 200 and d.get("resultType") == "complete", f"{st} {json.dumps(d)[:200]}")
    # modern 直调（无状态 _meta 路径；中文前缀名走 body，不经 header）
    # 中文前缀名不可放入 header（latin-1 限制），仅 body 携带；Mcp-Name 省略
    st, body, _ = req("POST", "/mcp/$smart", {"jsonrpc": "2.0", "id": 52, "method": "tools/call",
        "params": {"name": SEP_TOOL, "arguments": {}, "_meta": meta26}},
        {"Mcp-Method": "tools/call", "MCP-Protocol-Version": "2026-07-28"})
    obj = sse_obj(body, 52)
    t = json.dumps((obj or {}).get("result", {}), ensure_ascii=False)
    check("B3 [2026-07-28] modern 前缀直调", ok_call(obj) and has_ip(t), f"{st} {body[:250]}")
    req("DELETE", "/mcp/$smart", headers={"mcp-session-id": sid26})

    # ═══════════ D（有 pin 侧）REST 通道独立 ═══════════
    st, body, _ = req("POST", f"/rest/{IP_SERVER_ENC}/call", {"tool": IP_TOOL, "arguments": {}})
    check("D1 有 pin 时 REST call 200（通道独立）", st == 200 and has_ip(body), f"{st} {body[:200]}")

    # ═══════════ E. 门禁负路径：伪造前缀 ═══════════
    st, sid, _, _ibody = init_smart("/mcp/$smart", "2025-11-25", 60)
    st, body, _ = req("POST", "/mcp/$smart", {"jsonrpc": "2.0", "id": 61, "method": "tools/call",
        "params": {"name": f"{IP_SERVER}-nonexistent_tool_xyz", "arguments": {}}},
        headers={"mcp-session-id": sid})
    obj = sse_obj(body, 61)
    check("E 伪造前缀 {server}-{不存在工具} 被拒", (obj or {}).get("error") is not None, f"{st} {body[:200]}")
    req("DELETE", "/mcp/$smart", headers={"mcp-session-id": sid})

    # ═══════════ F. 宽松模式：无 Accept 头裸请求升格直调 ═══════════
    st, body, _ = req("POST", "/mcp/$smart", {"id": 70, "method": "tools/call",
        "params": {"name": SEP_TOOL, "arguments": {}}}, headers={"mcp-session-id": ""},
        with_default_accept=False)
    obj = sse_obj(body, 70)
    t = json.dumps((obj or {}).get("result", {}), ensure_ascii=False)
    check("F 无 Accept 头裸请求升格直调 pin 工具", ok_call(obj) and has_ip(t), f"{st} {body[:250]}")

finally:
    restore_pin(); time.sleep(0.3)

# ═══════════ C. 未 pin：根 $smart 不暴露 + REST 仍 200 ═══════════
st, sid, obj, _ibody = init_smart("/mcp/$smart", "2025-11-25", 80)
st, cnames, _ = list_tools("/mcp/$smart", sid, 81, "2025-11-25")
check("C1 未 pin 时根$smart 列表不含该工具", SEP_TOOL not in (cnames or []), str(cnames)[:300])
st, body, _ = req("POST", "/mcp/$smart", {"jsonrpc": "2.0", "id": 82, "method": "tools/call",
    "params": {"name": SEP_TOOL, "arguments": {}}}, headers={"mcp-session-id": sid})
obj = sse_obj(body, 82)
check("C2 未 pin 时前缀直调被拒", (obj or {}).get("error") is not None, f"{st} {body[:200]}")
req("DELETE", "/mcp/$smart", headers={"mcp-session-id": sid})
st, body, _ = req("POST", f"/rest/{IP_SERVER_ENC}/call", {"tool": IP_TOOL, "arguments": {}})
check("D2 无 pin 时 REST call 仍 200（通道独立）", st == 200 and has_ip(body), f"{st} {body[:200]}")

# 还原核验
fin = _pin_state()
check("Z1 pin 状态已还原", fin == ORIG_PIN, f"orig={ORIG_PIN} final={fin}")

print(f"\n{'='*50}\nPASS={len(PASS)} FAIL={len(FAIL)}")
if FAIL:
    print("FAILED:", *FAIL, sep="\n  - ")
    sys.exit(1)
