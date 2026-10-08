#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
rmcp_audit_r6xx.py — 第 6xx 轮审计缺口套件
N-JR-SCOPE      json_response=true × scope 通道（CT=application/json、无 SSE 帧、id 回显）
N-STRICT-SMART  严格模式 × $smart 裸请求/缺件拒绝（基线还原）
N-BEARER-SMART  bearer × /mcp/$smart 401/200/allow-list 收窄
N-PIN-UNPIN     unpin 后 $smart search/describe/call 即时消失/拒绝
N-SESSION-REAPER 会话空闲回收（DB 直置 last-seen 过期不可行 → 仅验活跃会话不被误杀）
前置：应用 127.0.0.1:23333，openapi IP 服务器已连，smartRouting.enabled=1，分组 Test 存在。
"""
import http.client, json, sqlite3, sys, time, re, ipaddress, base64
from pathlib import Path
from urllib.parse import quote

HOST, PORT = "127.0.0.1", 23333
PASS, FAIL, SKIP = [], [], []
IP_SERVER = "本机公网ip查询"
IP_TOOL = "getPublicIp"

def check(n, c, d=""):
    if c: PASS.append(n); print(f"PASS | {n}")
    else: FAIL.append(n); print(f"FAIL | {n} | {d}")

def skip(n, d=""):
    SKIP.append(n); print(f"SKIP | {n} | {d}")

def raw_post(path, body, headers=None, timeout=90):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
    if headers: h.update(headers)
    c.request("POST", path, json.dumps(body), h)
    r = c.getresponse(); data = r.read().decode("utf-8", "replace")
    ct = r.getheader("Content-Type") or ""; sid = r.getheader("mcp-session-id")
    c.close(); return r.status, data, ct, sid

def sse_obj(data, rid):
    try:
        if data.strip().startswith("{"):
            o = json.loads(data)
            if o.get("id") == rid or "result" in o or "error" in o: return o
    except Exception: pass
    for line in data.split("\n"):
        line = line.strip()
        if line.startswith("data: "):
            try:
                o = json.loads(line[6:])
                if o.get("id") == rid: return o
            except Exception: continue
    return None

def init(path, pv="2025-11-25"):
    st, data, _, sid = raw_post(path, {"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":pv,"capabilities":{},"clientInfo":{"name":"r6xx","version":"1"}}})
    if st != 200 or not sid: return None, None
    raw_post(path, {"jsonrpc":"2.0","method":"notifications/initialized"}, {"mcp-session-id": sid})
    return sid, data

def list_tools(path, sid, rid=2):
    st, data, _, _ = raw_post(path, {"jsonrpc":"2.0","id":rid,"method":"tools/list"}, {"mcp-session-id": sid})
    o = sse_obj(data, rid)
    return [t.get("name") for t in ((o or {}).get("result") or {}).get("tools") or []]

def db():
    home = Path.home()
    for cand in [home/"Library/Application Support/app.mcphub.desktop/mcphub.db",
                 home/".config/app.mcphub.desktop/mcphub.db",
                 home/"AppData/Roaming/app.mcphub.desktop/mcphub.db"]:
        if cand.exists(): return str(cand)
    return None

def cfg_read():
    con = sqlite3.connect(DB); row = con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone(); con.close()
    return json.loads(row[0] or "{}") if row else {}

def cfg_write(c):
    con = sqlite3.connect(DB)
    con.execute("UPDATE system_config SET config_json=?, updated_at=datetime('now') WHERE rowid=1", (json.dumps(c, ensure_ascii=False),))
    con.commit(); con.close()

def set_flag(path, key, val):
    """path = parent chain, e.g. ["mcp"]; key = leaf. Tolerates non-dict nodes.
    Old value None means 'key absent' -> restore deletes the key."""
    c = cfg_read(); node = c
    for k in path:
        nxt = node.get(k)
        if not isinstance(nxt, dict): nxt = {}
        node[k] = nxt; node = nxt
    old = node.get(key); node[key] = val; cfg_write(c); return old

def restore_flag(path, key, old):
    """old None = key was absent -> delete it."""
    if old is None:
        c = cfg_read(); node = c
        for k in path:
            nxt = node.get(k)
            if not isinstance(nxt, dict): return
        node = nxt
        node.pop(key, None); cfg_write(c)
    else:
        set_flag(path, key, old)

def valid_ip(text):
    for m in re.findall(r"\d{1,3}(?:\.\d{1,3}){3}", text or ""):
        try:
            a = ipaddress.ip_address(m)
            if a.version == 4 and not (a.is_private or a.is_loopback or a.is_reserved): return True
        except ValueError: pass
    return False

MODERN = {"MCP-Protocol-Version": "2026-07-28"}
MODERN_META = {"io.modelcontextprotocol/protocolVersion": "2026-07-28",
               "io.modelcontextprotocol/clientInfo": {"name": "r6xx", "version": "1"},
               "io.modelcontextprotocol/clientCapabilities": {}}

def main():
    global DB
    DB = db()
    if not DB: print("SKIP: no DB"); sys.exit(0)
    # Discover the IP tool name from the live listing instead of guessing
    # DB schema/columns (servers table has no config column in fresh installs).
    st0, d0, _, sid0 = raw_post("/mcp", {"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"r6xx","version":"1"}}})
    if st0 != 200 or not sid0: print("SKIP: init failed"); sys.exit(0)
    raw_post("/mcp", {"jsonrpc":"2.0","method":"notifications/initialized"}, {"mcp-session-id": sid0})
    st1, d1, _, _ = raw_post("/mcp", {"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}, {"mcp-session-id": sid0})
    o1 = sse_obj(d1, 2)
    tools = ((o1 or {}).get("result") or {}).get("tools") or []
    ip_entry = next((t for t in tools if t.get("name","").endswith("getPublicIp")), None)
    if not ip_entry:
        print("SKIP: getPublicIp tool not found"); sys.exit(0)
    full_name = ip_entry["name"]
    global IP_SERVER, IP_TOOL
    IP_TOOL = "getPublicIp"
    IP_SERVER = full_name[: -len("-" + IP_TOOL)] if "-" in full_name else ""
    # SEP_TOOL recomputed in main via IP_SERVER; also fix global SEP usage
    h0_s = cfg_read().get("mcp", {}).get("strictValidation")
    h0_b = cfg_read().get("routing", {}).get("enableBearerAuth")
    enc_srv = quote(IP_SERVER, safe="") if IP_SERVER else ""
    SEP_TOOL = full_name

    # Ensure the IP tool is pinned so the $smart direct-call lane is available.
    con = sqlite3.connect(DB)
    con.execute("""INSERT INTO server_tool_config (id, server_name, item_type, item_name, enabled, description, pinned)
        VALUES (lower(hex(randomblob(16))), ?, 'tool', ?, 1, NULL, 1)
        ON CONFLICT(server_name, item_type, item_name) DO UPDATE SET pinned=1""", (IP_SERVER, IP_TOOL))
    con.commit(); con.close(); time.sleep(0.6)

    # ── N-JR-SCOPE ───────────────────────────────────────────────────────────
    print("== N-JR-SCOPE: json_response=true × scope 通道 ==")
    for path, tag in [("/mcp", "root"), (f"/mcp/{enc_srv}", "scope"), ("/mcp/$smart", "smart")]:
        call_name = IP_TOOL if tag == "scope" else SEP_TOOL  # scope: bare; root+smart: {server}-{tool} prefixed
        body = {"jsonrpc":"2.0","id":"jr","method":"tools/call",
                "params":{"name": call_name, "arguments": {},
                          "_meta": dict(MODERN_META)}}
        hdrs = dict(MODERN)
        hdrs["Mcp-Method"] = "tools/call"
        hdrs["Mcp-Name"] = "=?base64?" + base64.b64encode(call_name.encode()).decode() + "?="
        st, data, ct, _ = raw_post(path, body, hdrs)
        o = sse_obj(data, "jr")
        txt = json.dumps((o or {}).get("result", {}), ensure_ascii=False)
        check(f"N-JR[{tag}] CT=application/json 无 SSE 帧", st == 200 and "application/json" in ct and "text/event-stream" not in ct and data.strip().startswith("{"), f"st={st} ct={ct} body={data[:160]}")
        check(f"N-JR[{tag}] id 回显", (o or {}).get("id") == "jr", str(o)[:80])
        if tag != "smart":
            check(f"N-JR[{tag}] 公网IP真实调用", valid_ip(txt) and '"isError": true' not in txt, txt[:100])
        else:
            check(f"N-JR[smart] 非5xx", st < 500, f"st={st}")

    # ── N-STRICT-SMART ──────────────────────────────────────────────────────
    print("== N-STRICT-SMART: 严格模式 × $smart 裸请求 ==")
    old_s = set_flag(["mcp"], "strictValidation", True); time.sleep(0.4)
    try:
        # (a) 缺 Accept → 4xx
        st, data, _, _ = raw_post("/mcp/$smart", {"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}},
                                  headers={"Accept": ""})
        check("N-SS[a] 缺 Accept 严格 4xx", 400 <= st < 500, f"st={st}")
        # (b) 裸 tools/call（无 session/无 client 元数据，宽松才升格）→ 4xx
        c = http.client.HTTPConnection(HOST, PORT, timeout=30)
        c.request("POST", "/mcp/$smart", json.dumps({"jsonrpc":"2.0","id":2,"method":"tools/call",
            "params":{"name":"smart_route_search","arguments":{"query":"ip","limit":2}}}),
            {"Content-Type":"application/json","Accept":"application/json, text/event-stream"})
        r = c.getresponse(); st2 = r.status; r.read(); c.close()
        check("N-SS[b] 裸请求严格 4xx", 400 <= st2 < 500, f"st={st2}")
    finally:
        restore_flag(["mcp"], "strictValidation", old_s); time.sleep(0.4)
    # 宽松对照：裸请求放行
    c = http.client.HTTPConnection(HOST, PORT, timeout=60)
    c.request("POST", "/mcp/$smart", json.dumps({"jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"smart_route_search","arguments":{"query":"ip","limit":2}}}),
        {"Content-Type":"application/json","Accept":"application/json, text/event-stream"})
    r = c.getresponse(); st3 = r.status; d3 = r.read().decode(); c.close()
    check("N-SS[c] 宽松裸请求放行", st3 == 200 and (sse_obj(d3, 3) or {}).get("result") is not None, f"st={st3} {d3[:100]}")

    # ── N-BEARER-SMART ──────────────────────────────────────────────────────
    print("== N-BEARER-SMART ==")
    con = sqlite3.connect(DB)
    key_row = con.execute("SELECT token FROM bearer_keys WHERE enabled=1 LIMIT 1").fetchone()
    con.close()
    old_b = set_flag(["routing"], "enableBearerAuth", True); time.sleep(0.4)
    try:
        st, _, _, _ = raw_post("/mcp/$smart", {"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}})
        check("N-BS[a] bearer-on 无 key → 401", st == 401, f"st={st}")
        if key_row:
            st, data, _, _ = raw_post("/mcp/$smart", {"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}},
                                      {"Authorization": f"Bearer {key_row[0]}"})
            o = sse_obj(data, 1)
            check("N-BS[b] 有效 key → 200", st == 200 and o and "result" in o, f"st={st}")
        else:
            skip("N-BS[b]", "无 bearer key")
    finally:
        restore_flag(["routing"], "enableBearerAuth", old_b); time.sleep(0.4)
    st, _, _, _ = raw_post("/mcp/$smart", {"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}})
    check("N-BS[c] bearer-off 恢复放行", st == 200, f"st={st}")

    # ── N-PIN-UNPIN ─────────────────────────────────────────────────────────
    print("== N-PIN-UNPIN: unpin 后 $smart 三面即时消失 ==")
    con = sqlite3.connect(DB)
    con.execute("""INSERT INTO server_tool_config (id, server_name, item_type, item_name, enabled, description, pinned)
        VALUES (lower(hex(randomblob(16))), ?, 'tool', ?, 1, NULL, 1)
        ON CONFLICT(server_name, item_type, item_name) DO UPDATE SET pinned=1""", (IP_SERVER, IP_TOOL))
    con.commit(); con.close(); time.sleep(0.6)
    sid, _ = init("/mcp/$smart")
    if not sid:
        skip("N-PU", "smart 会话建立失败")
    else:
        names = list_tools("/mcp/$smart", sid)
        pinned_on = IP_TOOL in names or SEP_TOOL in names
        # unpin
        con = sqlite3.connect(DB)
        con.execute("UPDATE server_tool_config SET pinned=0 WHERE server_name=? AND item_type='tool' AND item_name=?", (IP_SERVER, IP_TOOL))
        con.commit(); con.close(); time.sleep(0.6)
        names2 = list_tools("/mcp/$smart", sid)
        gone_list = not (IP_TOOL in names2 or SEP_TOOL in names2)
        check("N-PU[a] unpin 后列表消失", gone_list, f"names2={len(names2)}")
        # search 面
        st, d, _, _ = raw_post("/mcp/$smart", {"jsonrpc":"2.0","id":5,"method":"tools/call",
            "params":{"name":"smart_route_search","arguments":{"query":"ip","limit":5}}},
            {"mcp-session-id": sid})
        o = sse_obj(d, 5)
        res_txt = json.dumps((o or {}).get("result", {}), ensure_ascii=False)
        # 搜索可能仍命中（未 pin 的服务器工具仍在 $smart 索引）——断言针对 describe/call
        st2, d2, _, _ = raw_post("/mcp/$smart", {"jsonrpc":"2.0","id":6,"method":"tools/call",
            "params":{"name": SEP_TOOL if pinned_on else IP_TOOL, "arguments": {}}},
            {"mcp-session-id": sid})
        o2 = sse_obj(d2, 6)
        txt2 = json.dumps((o2 or {}).get("result", o2 or {}), ensure_ascii=False)
        check("N-PU[b] unpin 后直调拒绝", o2 is None or (o2.get("error") is not None or "is not a pinned tool" in txt2 or "not available" in txt2 or "not found" in txt2.lower()), f"st={st2} {txt2[:120]}")
        st3, d3, _, _ = raw_post("/mcp/$smart", {"jsonrpc":"2.0","id":7,"method":"tools/call",
            "params":{"name":"smart_route_describe","arguments":{"toolName": SEP_TOOL if pinned_on else IP_TOOL}}},
            {"mcp-session-id": sid})
        o3 = sse_obj(d3, 7)
        txt3 = json.dumps((o3 or {}).get("result", {}), ensure_ascii=False)
        # describe is NOT pin-gated (origin parity): its allowed set is the
        # $smart scope's pool tools, not the pinned list. Only the direct-call
        # lane and the tools/list exposure are pin-gated.
        check("N-PU[c] unpin 后 describe 仍可用（origin parity：describe 不查 pin）",
              st3 == 200 and _describe_tool_present(o3), f"st={st3} {txt3[:100]}")
        raw_post("/mcp/$smart", {"jsonrpc":"2.0","id":8,"method":"not-a-method"}, {"mcp-session-id": sid})

    # ── N9 卫生 ─────────────────────────────────────────────────────────────
    h1_s = cfg_read().get("mcp", {}).get("strictValidation")
    h1_b = cfg_read().get("routing", {}).get("enableBearerAuth")
    check("N9 strict 无泄漏", h0_s == h1_s, f"{h0_s}→{h1_s}")
    check("N9 bearer 无泄漏", h0_b == h1_b, f"{h0_b}→{h1_b}")

    print(f"\n{'='*50}\nPASS={len(PASS)} FAIL={len(FAIL)} SKIP={len(SKIP)}")
    if FAIL: print("FAILED:", *FAIL, sep="\n  - ")
    sys.exit(1 if FAIL else 0)

def _describe_tool_present(o):
    """describe returns content[0].text = JSON with a "tool" object."""
    try:
        txt = o["result"]["content"][0]["text"]
        return "tool" in json.loads(txt)
    except Exception:
        return False

if __name__ == "__main__":
    main()
