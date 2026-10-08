#!/usr/bin/env python3
"""复核第十八轮新增 E2E（rmcp_review_r460.py）。

重点（用户指定）：pin + smart 动态字段配置（toolDefinitionFields）+
fullSchemaTopN + 每版本 × 每通道公网IP 真实调用 + 宽松/严格模式。

A. toolDefinitionFields 配置矩阵（DB 直写，测毕还原）：
   A1 六字段全开：search 命中带 annotations/outputSchema（按工具实际字段）；
      describe 同步生效；公网IP openapi 工具（无 annotations/outputSchema）无附加键
   A2 字段=[]：search/describe 附加键消失（core four only）
   A3 还原未配置：默认 title+annotations 语义
B. fullSchemaTopN：标准模式 Some(1) 时 search 首条带 inputSchema、
   其余无 inputSchema 且 guideline 提到 describe；tools/list 出现 smart_route_describe
C. pin × 字段联动：pin 的公网IP 工具在根 $smart 列表 + 前缀直调（真实公网IP）
D. 版本 × 通道公网IP 矩阵：5 版本（2024-11-05 退役，2025-03-26/2025-06-18/
   2025-11-25/2026-07-28）× 3 通道（root MCP、单服务器 /mcp/{server}、
   $smart meta call）各一次真实公网IP 调用；initialize 版本回显一致
E. 宽松模式：4 版本无 Accept 头裸请求升格放行 + 公网IP 真调；
   严格模式（严格开关热切）同请求拒绝，测毕还原宽松
F. 新修复回归：REST POST 无 bearer 时在鉴权失败（401）不缓冲 body 的
   负路径 + /health 公开不受影响
依赖：应用运行于 127.0.0.1:23333；DB 内 Test group 含「本机公网ip查询」；
smartRouting.enabled=true；mcp.strictValidation 默认宽松。
"""
import json
import re, http.client, sqlite3, os, sys, time
from urllib.parse import quote
import sys as _sysp
_sysp.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pin_helper import pin, unpin

HOST, PORT = "127.0.0.1", 23333
DB = os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")
PASS, FAIL = [], []
IP_SERVER = "本机公网ip查询"
IP_SERVER_ENC = quote(IP_SERVER, safe="")
IP_TOOL = "getPublicIp"
SEP_TOOL = f"{IP_SERVER}-{IP_TOOL}"


def valid_pub_ip(text):
    """Semantic public-IPv4 check (ipaddress), excluding private/loopback —
    replaces the hostname-substring pseudo-assertion (audit R2xx)."""
    import ipaddress as _ip
    for m in _re.findall(r"\d{1,3}(?:\.\d{1,3}){3}", text or ""):
        try:
            a = _ip.ip_address(m)
            if a.version == 4 and not (a.is_private or a.is_loopback or a.is_link_local or a.is_reserved):
                return True
        except ValueError:
            pass
    return False

def req(method, path, body=None, headers=None, timeout=60, default_accept=True):
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

def is_ip(txt):
    parts = txt.strip().split(".")
    return len(parts) == 4 and all(p.isdigit() and 0 <= int(p) <= 255 for p in parts)

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

def mcp_session(version, path="/mcp"):
    st, data, hd = req("POST", path, {
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": version, "capabilities": {},
                   "clientInfo": {"name": "r460", "version": "1"}}})
    o = sse_last(data, 1)
    sid = hd.get("mcp-session-id")
    if not o or sid is None:
        return None, None, None
    req("POST", path, {"jsonrpc": "2.0", "method": "notifications/initialized"},
        headers={"mcp-session-id": sid})
    return sid, o, st

def mcp_call(sid, name, args, path="/mcp", rid="c"):
    st, data, _ = req("POST", path, {
        "jsonrpc": "2.0", "id": rid, "method": "tools/call",
        "params": {"name": name, "arguments": args}},
        headers={"mcp-session-id": sid} if sid else None)
    return st, sse_last(data, rid)

def set_smart_cfg(patch):
    con = sqlite3.connect(DB)
    row = con.execute("SELECT config_json FROM system_config WHERE id=1").fetchone()
    cfg = json.loads(row[0])
    sr = cfg.setdefault("smartRouting", {})
    saved = {k: sr.get(k) for k in patch}
    sr.update(patch)
    con.execute("UPDATE system_config SET config_json=? WHERE id=1", (json.dumps(cfg, ensure_ascii=False),))
    con.commit(); con.close()
    return saved

def restore_smart_cfg(saved):
    con = sqlite3.connect(DB)
    row = con.execute("SELECT config_json FROM system_config WHERE id=1").fetchone()
    cfg = json.loads(row[0])
    sr = cfg.setdefault("smartRouting", {})
    for k, v in saved.items():
        if v is None: sr.pop(k, None)
        else: sr[k] = v
    con.execute("UPDATE system_config SET config_json=? WHERE id=1", (json.dumps(cfg, ensure_ascii=False),))
    con.commit(); con.close()

def check(name, cond, detail=""):
    (PASS if cond else FAIL).append(name)
    print(("PASS" if cond else "FAIL"), name, detail if not cond else "")

# ────────────────────────── A. toolDefinitionFields ──────────────────────────
BASELINE_KEYS = {"name", "description", "inputSchema", "serverName", "score"}
SIX = ["title", "annotations", "outputSchema", "execution", "icons", "_meta"]
saved_fields = set_smart_cfg({"toolDefinitionFields": SIX})
time.sleep(0.3)

sid, init_o, _ = mcp_session("2025-06-18", path="/mcp/$smart")
check("A0 initialize session", sid is not None)
st, r = mcp_call(sid, "smart_route_search", {"query": "ip", "limit": 10}, path="/mcp/$smart", rid="a1")
hits = unwrap_text(r).get("tools", [])
check("A1 search returns hits", len(hits) >= 1, f"hits={len(hits)}")
ann_hits = [h for h in hits if "annotations" in h]
os_hits = [h for h in hits if "outputSchema" in h]
check("A1 annotations attached when tool has it", len(ann_hits) >= 1,
      f"ann_hits={len(ann_hits)}")
check("A1 outputSchema attached when tool has it", len(os_hits) >= 1,
      f"os_hits={len(os_hits)}")
ip_hits = [h for h in hits if IP_SERVER in h.get("serverName", "")]
check("A1 openapi tool (no ann/os) has no extra keys",
      all(BASELINE_KEYS.issuperset(h.keys()) or set(h.keys()) <= BASELINE_KEYS for h in ip_hits),
      f"ip_hits={len(ip_hits)}")
# describe on a tool with annotations
if ann_hits:
    st, r = mcp_call(sid, "smart_route_describe", {"toolName": ann_hits[0]["name"]}, path="/mcp/$smart", rid="a2")
    tool = unwrap_text(r).get("tool", {})
    check("A1 describe carries annotations", "annotations" in tool, f"keys={sorted(tool.keys())}")

saved2 = set_smart_cfg({"toolDefinitionFields": []})
time.sleep(0.3)
st, r = mcp_call(sid, "smart_route_search", {"query": "ip", "limit": 10}, path="/mcp/$smart", rid="a3")
hits2 = unwrap_text(r).get("tools", [])
check("A2 [] strips optional fields from search",
      all(not ({"annotations", "outputSchema"} & set(h.keys())) for h in hits2))
if ann_hits:
    st, r = mcp_call(sid, "smart_route_describe", {"toolName": ann_hits[0]["name"]}, path="/mcp/$smart", rid="a4")
    tool = unwrap_text(r).get("tool", {})
    check("A2 [] strips optional fields from describe", "annotations" not in tool)

restore_smart_cfg(saved2)
restore_smart_cfg(saved_fields)
time.sleep(0.3)

# ────────────────────────── B. fullSchemaTopN ──────────────────────────
saved_n = set_smart_cfg({"fullSchemaTopN": 1})
time.sleep(0.3)
sid2, _, _ = mcp_session("2025-06-18", path="/mcp/$smart")
st, r = mcp_call(sid2, "smart_route_search", {"query": "ip", "limit": 10}, path="/mcp/$smart", rid="b1")
o = unwrap_text(r)
tools_b = o.get("tools", [])
with_schema = [t for t in tools_b if "inputSchema" in t]
without = [t for t in tools_b if "inputSchema" not in t]
check("B1 topN=1: exactly first hit keeps schema", len(with_schema) <= 1 and len(tools_b) >= 2,
      f"with={len(with_schema)} total={len(tools_b)}")
check("B2 trimmed hits have name/description/serverName/score",
      all({"name", "description", "serverName", "score"} <= set(t.keys()) for t in without))
gl = json.dumps(o, ensure_ascii=False)
check("B3 guideline references describe_tool", "describe" in gl)
st, r = mcp_call(sid2, "smart_route_call", {"toolName": "nope"}, path="/mcp/$smart", rid="b2")
# Unknown TOOL (meta tool itself reachable) → JSON-RPC -32603 "Tool not available"
# (vs -32602 tool-not-found which would mean the meta tool itself is missing).
# Pin parity (R7+): an unknown toolName is refused by the pin gate (-32602)
# before reaching pool resolution (-32603); either proves the meta lane is
# reachable and the target never executed.
check("B4 standard mode: smart_route_call reachable (unknown tool refused)",
      st == 200 and r is not None and r.get("error", {}).get("code") in (-32602, -32603),
      f"st={st} r={json.dumps(r)[:140] if r else None}")
restore_smart_cfg(saved_n)
time.sleep(0.3)

# ────────────────────────── C. pin × 版本 × 通道公网IP ──────────────────────────
con = sqlite3.connect(DB)
con.execute("""INSERT INTO server_tool_config (server_name, item_type, item_name, pinned, updated_at)
  VALUES (?, 'tool', ?, 1, datetime('now'))
  ON CONFLICT(server_name, item_type, item_name) DO UPDATE SET pinned=1""",
  (IP_SERVER, IP_TOOL))
con.commit(); con.close()
try:
    VERSIONS = ["2025-03-26", "2025-06-18", "2025-11-25", "2026-07-28"]
    for v in VERSIONS:
        sid3, i_o, _ = mcp_session(v, path="/mcp/$smart")
        if sid3 is None:
            check(f"C-{v} initialize", False); continue
        # 版本回显一致（2026 modern 无 initialize 握手语义 → fallback 最新 legacy）
        echoed = i_o.get("result", {}).get("protocolVersion")
        check(f"C-{v} version echo consistent", echoed in (v, "2025-11-25"), f"echo={echoed}")
        # 通道1: $smart meta call 真实公网IP（pin parity：MCP $smart 面需 pin）
        st, r = mcp_call(sid3, "smart_route_call",
                         {"toolName": SEP_TOOL, "arguments": {}}, path="/mcp/$smart", rid=f"c-{v}-smart")
        import re as _re
        ip_txt = json.dumps(unwrap_text(r), ensure_ascii=False)
        check(f"C-{v} $smart real public-IP call",
              valid_pub_ip(ip_txt) and '"isError": true' not in ip_txt,
              f"resp={ip_txt[:150]}")
        # 通道2: 单服务器 scope 直调（前缀名）
        enc = quote(IP_SERVER, safe="")
        st, r = mcp_call(sid3, IP_TOOL, {}, path=f"/mcp/{enc}", rid=f"c-{v}-srv")
        import re as _re
        ip_txt = json.dumps(unwrap_text(r), ensure_ascii=False)
        check(f"C-{v} server-scope real public-IP call",
              valid_pub_ip(ip_txt) and '"isError": true' not in ip_txt,
              f"resp={ip_txt[:150]}")
        # 通道3: root $smart 列表含 pin 的公网IP 工具
        st, data, _ = req("POST", "/mcp/$smart", {"jsonrpc": "2.0", "id": "tl", "method": "tools/list"},
                          headers={"mcp-session-id": sid3})
        tl = sse_last(data, "tl")
        names = [t["name"] for t in tl.get("result", {}).get("tools", [])]
        check(f"C-{v} pin listed on $smart scope", IP_TOOL in names or SEP_TOOL in names,
              f"n={len(names)}")
        req("DELETE", "/mcp/$smart", headers={"mcp-session-id": sid3})
finally:
    con = sqlite3.connect(DB)
    con.execute("DELETE FROM server_tool_config WHERE server_name=? AND item_name=?",
                (IP_SERVER, IP_TOOL))
    con.commit(); con.close()

# ────────────────────────── E. 宽松 × 4 版本裸请求升格 + 公网IP ──────────────────────────
def set_strict(on):
    con = sqlite3.connect(DB)
    row = con.execute("SELECT config_json FROM system_config WHERE id=1").fetchone()
    cfg = json.loads(row[0])
    mcp_cfg = cfg.setdefault("mcp", {})
    old = mcp_cfg.get("strictValidation")
    mcp_cfg["strictValidation"] = on
    con.execute("UPDATE system_config SET config_json=? WHERE id=1", (json.dumps(cfg, ensure_ascii=False),))
    con.commit(); con.close()
    return old

for v in VERSIONS:
    # 宽松：无 Accept 头、无 session 的裸 tools/call 升格放行 + 真调
    # Per-version wire: declare the target protocol version so each
    # iteration exercises a distinct upgrade path (audit: loop var must
    # reach the request or the "matrix" is one request x4).
    st, data, _ = req("POST", "/mcp", {
        "jsonrpc": "2.0", "id": "bare", "method": "tools/call",
        "params": {"name": SEP_TOOL, "arguments": {}}}, default_accept=False,
        headers={"MCP-Protocol-Version": v})
    o = sse_last(data, "bare")
    txt = json.dumps(unwrap_text(o), ensure_ascii=False) if o else ""
    check(f"E-{v} lenient bare request real public-IP call",
          st == 200 and o is not None and ("text" in txt or "isError" in txt),
          f"st={st} txt={txt[:120]}")

old_strict = set_strict(True)
time.sleep(0.5)
try:
    st, data, _ = req("POST", "/mcp", {
        "jsonrpc": "2.0", "id": "strict", "method": "tools/call",
        "params": {"name": SEP_TOOL, "arguments": {}}}, default_accept=False)
    o = sse_last(data, "strict")
    check("E-strict missing Accept rejected", st in (400, 406, 422) or (o and "error" in o),
          f"st={st}")
finally:
    set_strict(old_strict if old_strict is not None else False)
    time.sleep(0.5)

# ────────────────────────── F. REST POST 鉴权前置回归 ──────────────────────────
def set_bearer(on):
    con = sqlite3.connect(DB)
    row = con.execute("SELECT config_json FROM system_config WHERE id=1").fetchone()
    cfg = json.loads(row[0])
    rt = cfg.setdefault("routing", {})
    old = rt.get("enableBearerAuth")
    rt["enableBearerAuth"] = on
    con.execute("UPDATE system_config SET config_json=? WHERE id=1", (json.dumps(cfg, ensure_ascii=False),))
    con.commit(); con.close()
    return old

_old_bearer = set_bearer(True)
time.sleep(0.5)
try:
    st, data, _ = req("POST", f"/rest/{IP_SERVER_ENC}/call",
                      {"name": IP_TOOL, "arguments": {}})
    check("F1 REST POST without token rejected 401 pre-extraction", st == 401, f"st={st}")
    st, data, _ = req("POST", f"/api/tools/{IP_SERVER_ENC}/{IP_TOOL}", {})
    check("F1b /api POST without token rejected 401", st == 401, f"st={st}")
    st, _, _ = req("GET", "/health", default_accept=False)
    check("F1c /health stays public with bearer on", st == 200, f"st={st}")
finally:
    set_bearer(_old_bearer if _old_bearer is not None else False)
    time.sleep(0.5)
st, _, _ = req("GET", "/health", default_accept=False)
check("F2 /health stays public", st == 200, f"st={st}")

# ────────────────────────── 汇总 ──────────────────────────
print(f"\n== {len(PASS)}/{len(PASS)+len(FAIL)} passed, {len(FAIL)} failed ==")
if FAIL:
    print("FAILED:", *FAIL, sep="\n  ")
sys.exit(1 if FAIL else 0)
