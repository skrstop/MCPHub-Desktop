#!/usr/bin/env python3
"""复核第十九轮新增 E2E（rmcp_review_r461.py）。

核心（用户指定「必须验证每一个字段」）：toolDefinitionFields 六字段
逐字段单独启停验证 —— 对每个字段 F：
  1. 仅启用 F：search/describe 只可能多出 F（其他五键绝不出现）
  2. 仅排除 F（其余五个开）：F 绝不出现
  3. 字段来源真实性：annotations/outputSchema 来自公网IP openapi 工具
     （无）与 Idea-mcp-server 工具（有，见 DB 实测）—— 用「Idea 工具命中
     带 annotations / outputSchema」作正向样例；title/execution/icons/_meta
     在当前上游均不发送 → 启用后也不出现（origin `!== undefined` 语义）
另外：
  G. 渐进披露模式下 search 不携带 optional fields（origin parity）
  H. smart_rest_call 未知工具 → 400（本轮修复回归）
  I. smart_rest_describe 未知工具 → 200 + error body（origin parity）
依赖：应用运行于 127.0.0.1:23333；smartRouting.enabled=true。
"""
import json, http.client, sqlite3, os, sys, time
from urllib.parse import quote

HOST, PORT = "127.0.0.1", 23333
DB = os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")
PASS, FAIL = [], []
SIX = ["title", "annotations", "outputSchema", "execution", "icons", "_meta"]
FIELD_TO_KEY = {f: f for f in SIX}  # whitelist string == response key name

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

def mcp_session(version="2025-06-18", path="/mcp/$smart"):
    st, data, hd = req("POST", path, {
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": version, "capabilities": {},
                   "clientInfo": {"name": "r461", "version": "1"}}})
    sid = hd.get("mcp-session-id")
    if not sid:
        return None
    req("POST", path, {"jsonrpc": "2.0", "method": "notifications/initialized"},
        headers={"mcp-session-id": sid})
    return sid

def mcp_call(sid, name, args, path="/mcp/$smart", rid="c"):
    st, data, _ = req("POST", path, {
        "jsonrpc": "2.0", "id": rid, "method": "tools/call",
        "params": {"name": name, "arguments": args}},
        headers={"mcp-session-id": sid} if sid else None)
    return st, sse_last(data, rid)

def set_fields(fields):
    con = sqlite3.connect(DB)
    row = con.execute("SELECT config_json FROM system_config WHERE id=1").fetchone()
    cfg = json.loads(row[0])
    sr = cfg.setdefault("smartRouting", {})
    old = sr.get("toolDefinitionFields")
    old_prog = sr.get("progressiveDisclosure")
    sr["toolDefinitionFields"] = fields
    con.execute("UPDATE system_config SET config_json=? WHERE id=1", (json.dumps(cfg, ensure_ascii=False),))
    con.commit(); con.close()
    return old, old_prog

def set_progressive(on):
    con = sqlite3.connect(DB)
    row = con.execute("SELECT config_json FROM system_config WHERE id=1").fetchone()
    cfg = json.loads(row[0])
    sr = cfg.setdefault("smartRouting", {})
    old = sr.get("progressiveDisclosure")
    sr["progressiveDisclosure"] = on
    con.execute("UPDATE system_config SET config_json=? WHERE id=1", (json.dumps(cfg, ensure_ascii=False),))
    con.commit(); con.close()
    return old

def restore(fields, prog):
    con = sqlite3.connect(DB)
    row = con.execute("SELECT config_json FROM system_config WHERE id=1").fetchone()
    cfg = json.loads(row[0])
    sr = cfg.setdefault("smartRouting", {})
    if fields is None: sr.pop("toolDefinitionFields", None)
    else: sr["toolDefinitionFields"] = fields
    if prog is None: sr.pop("progressiveDisclosure", None)
    else: sr["progressiveDisclosure"] = prog
    con.execute("UPDATE system_config SET config_json=? WHERE id=1", (json.dumps(cfg, ensure_ascii=False),))
    con.commit(); con.close()

def check(name, cond, detail=""):
    (PASS if cond else FAIL).append(name)
    print(("PASS" if cond else "FAIL"), name, detail if not cond else "")

BASE = {"name", "description", "serverName", "score"}
KEY_MAP = {"_meta": "_meta", "title": "title", "annotations": "annotations",
           "outputSchema": "outputSchema", "execution": "execution", "icons": "icons"}

saved_fields, saved_prog = set_fields(SIX)
time.sleep(0.3)

# 发现一个带 annotations 或 outputSchema 的真实工具作正向样例
sid = mcp_session()
check("P0 $smart session", sid is not None)
st, r = mcp_call(sid, "smart_route_search", {"query": "code", "limit": 20}, rid="p0")
all_hits = unwrap_text(r).get("tools", [])
probe = mcp_session()
st, r = mcp_call(probe, "smart_route_search", {"query": "a", "limit": 50}, rid="p0b")
all_hits = unwrap_text(r).get("tools", []) or all_hits
src_ann = next((h for h in all_hits if "annotations" in h), None)
src_os = next((h for h in all_hits if "outputSchema" in h), None)
print(f"# probe: hits={len(all_hits)} ann_src={src_ann and src_ann['name']} os_src={src_os and src_os['name']}")

def run_search(sid):
    st, r = mcp_call(sid, "smart_route_search", {"query": "a", "limit": 50}, rid="s")
    return unwrap_text(r).get("tools", [])

def run_describe(sid, tool_name):
    st, r = mcp_call(sid, "smart_route_describe", {"toolName": tool_name}, rid="d")
    return unwrap_text(r).get("tool", {})

# ── 逐字段验证 ──────────────────────────────────────────────────────────
for field in SIX:
    key = KEY_MAP[field]
    others = [k for k in SIX if k != field]

    # 1) 仅启用该字段
    set_fields([field]); time.sleep(0.3)
    sid = mcp_session()
    hits = run_search(sid)
    extra_in_hits = set()
    for h in hits:
        extra_in_hits |= (set(h.keys()) - BASE - {"inputSchema"})
    only_field = extra_in_hits <= {key}
    check(f"F1[{field}] enable-only: search extra keys ⊆ {{{key}}}", only_field,
          f"extra={sorted(extra_in_hits)}")
    # 正向样例：若库里有带该字段样例（annotations/outputSchema 有；其余上游不发 → 无样例）
    if field == "annotations":
        check(f"F1[{field}] positive sample appears", src_ann is not None,
              "no ann sample found in library")
    if field == "outputSchema":
        check(f"F1[{field}] positive sample appears", src_os is not None,
              "no outputSchema sample found in library")
    # describe 正向（有样例时）
    if field == "annotations" and src_ann:
        tool = run_describe(sid, src_ann["name"])
        check(f"F1[{field}] describe carries field", key in tool, f"keys={sorted(tool.keys())}")
    if field == "outputSchema" and src_os:
        tool = run_describe(sid, src_os["name"])
        check(f"F1[{field}] describe carries field", key in tool, f"keys={sorted(tool.keys())}")

    # 2) 排除该字段（其余五个开）
    set_fields(others); time.sleep(0.3)
    sid = mcp_session()
    hits = run_search(sid)
    leaked = set()
    for h in hits:
        leaked |= (set(h.keys()) - BASE - {"inputSchema"})
    check(f"F2[{field}] excluded: field never appears", key not in leaked,
          f"leaked={sorted(leaked)}")
    if field == "annotations" and src_ann:
        tool = run_describe(sid, src_ann["name"])
        check(f"F2[{field}] describe excludes field", key not in tool)

set_fields(SIX); time.sleep(0.3)

# ── G. 渐进披露：search 不携带 optional fields ─────────────────────────
set_progressive(True); time.sleep(0.3)
_G_OK = False
try:
    sid = mcp_session()
    hits = run_search(sid)
    extras = set()
    for h in hits:
        extras |= (set(h.keys()) - BASE)
    check("G progressive search carries no optional fields", extras <= {"inputSchema"} or not extras,
          f"extras={sorted(extras)}")
    tools_list_ok = False
    st, r = mcp_call(sid, "smart_route_search", {"query": "ip", "limit": 5}, rid="g2")
    # 渐进模式下元工具目录应为 3 个（search/describe/call）
    st, data, _ = req("POST", "/mcp/$smart", {"jsonrpc":"2.0","id":"tl","method":"tools/list"},
                      headers={"mcp-session-id": sid})
    tl = sse_last(data, "tl")
    names = [t["name"] for t in tl.get("result", {}).get("tools", [])]
    meta_named = [n for n in names if n.startswith("smart_route_")]
    check("G2 progressive lists 3 meta tools", len(meta_named) == 3, f"meta={meta_named}")
    _G_OK = True
finally:
    # Restore even when a G-group assertion crashes — leaked SIX+progressive
    # would poison every later suite.
    restore(saved_fields, saved_prog); time.sleep(0.3)
    if not _G_OK:
        check("G section completed", False, "G group crashed mid-section (config restored)")

# ── H/I. REST 状态码回归 ────────────────────────────────────────────────
st, data, _ = req("POST", "/api/$smart/call", {"toolName": "definitely-not-a-tool", "arguments": {}})
err_code = None
try: err_code = json.loads(data).get("error") and json.loads(data).get("status")
except Exception: pass
check("H smart_rest_call unknown tool → 400", st == 400, f"st={st} body={data[:120]}")
st, data, _ = req("POST", "/api/$smart/describe", {"toolName": "definitely-not-a-tool"})
check("I smart_rest_describe unknown tool → 200 + error body", st == 200 and "not found" in data,
      f"st={st} body={data[:120]}")

print(f"\n== {len(PASS)}/{len(PASS)+len(FAIL)} passed, {len(FAIL)} failed ==")
if FAIL:
    print("FAILED:", *FAIL, sep="\n  ")
sys.exit(1 if FAIL else 0)
