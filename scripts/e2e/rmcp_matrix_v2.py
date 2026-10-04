#!/usr/bin/env python3
# 全矩阵 E2E v2：5 协议版本 × 4 通道 × 宽松/严格 × 新特性，每版本至少一次公网IP真实调用
# 依赖: 服务器运行于 :23333，已配置「本机公网ip查询」(openapi) 与 group "Test"
import json, http.client, urllib.parse, sqlite3, os, sys, time

HOST, PORT = "localhost", 23333
DB = os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")
PASS, FAIL = [], []
VERSIONS = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25", "2026-07-28"]
IP_SERVER = "本机公网ip查询"
CHANNELS = [
    ("root", "/mcp", ""),
    ("group", "/mcp/Test", ""),
    ("scope", "/mcp/" + urllib.parse.quote(IP_SERVER), IP_SERVER + "-"),
    ("smart", "/mcp/$smart", ""),
]
PV = "io.modelcontextprotocol/protocolVersion"
CLIENT_META = {"io.modelcontextprotocol/clientInfo": {"name": "m", "version": "1"},
               "io.modelcontextprotocol/clientCapabilities": {}}

def check(name, ok, detail=""):
    (PASS if ok else FAIL).append(name)
    print(("PASS" if ok else "FAIL"), "|", name, (("| " + str(detail)[:120]) if (detail and not ok) else ""))

def req(method, path, payload=None, headers=None, timeout=90):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
    if headers: h.update(headers)
    body = json.dumps(payload) if payload is not None else None
    c.request(method, path, body=body, headers=h)
    r = c.getresponse(); data = r.read().decode("utf-8", "replace")
    st = r.status; sid = r.getheader("mcp-session-id"); ct = r.getheader("content-type") or ""
    c.close()
    obj = None
    if data.strip().startswith("{"):
        try: obj = json.loads(data)
        except Exception: pass
    if obj is None:
        frames = []
        for l in data.split("\n"):
            if l.startswith("data: ") and l[6:].strip():
                try: frames.append(json.loads(l[6:]))
                except Exception: pass
        obj = frames[-1] if frames else None
    return st, sid, obj, ct

def legacy_lifecycle(ch_name, base, prefix, version):
    """initialize→tools/list→真实IP调用→DELETE；断言版本回显一致"""
    tag = f"[{version}|{ch_name}]"
    st, sid, obj, _ = req("POST", base, {"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": version, "capabilities": {}, "clientInfo": {"name": "c", "version": "1"}}})
    check(f"{tag} initialize 200", st == 200 and obj and "result" in obj, f"st={st} {str(obj)[:80]}")
    if not (obj and "result" in obj):
        return
    check(f"{tag} 版本回显一致", obj["result"].get("protocolVersion") == version, str(obj["result"].get("protocolVersion")))
    check(f"{tag} session minted", bool(sid))
    hdrs = {"Mcp-Session-Id": sid, "MCP-Protocol-Version": version}
    st, _, obj, _ = req("POST", base, {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}, hdrs)
    tools = obj.get("result", {}).get("tools", []) if obj else []
    tname = next((t["name"] for t in tools if t["name"].endswith("getPublicIp")), None)
    if ch_name == "smart":
        check(f"{tag} tools/list meta工具暴露", st == 200 and len(tools) > 0, f"st={st} n={len(tools)}")
    else:
        check(f"{tag} tools/list + IP工具暴露", st == 200 and tname, f"st={st} n={len(tools)}")
    check(f"{tag} legacy 无 ttlMs/resultType", obj and "ttlMs" not in obj.get("result", {}) and "resultType" not in obj.get("result", {}))
    if tname:
        st, _, obj, _ = req("POST", base, {"jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {"name": tname, "arguments": {}}}, hdrs)
        txt = "".join(str(c.get("text", "")) for c in obj.get("result", {}).get("content", [])) if obj else ""
        check(f"{tag} 公网IP真实调用", st == 200 and obj.get("result", {}).get("isError") is False and len(txt) > 7, f"st={st} txt={txt[:40]}")
        if version == "2024-11-05":
            check(f"{tag} 2024 structuredContent 剥离", "structuredContent" not in obj.get("result", {}), str(obj)[:100])
    req("DELETE", base, None, {"Mcp-Session-Id": sid})

def modern_channel(ch_name, base, prefix):
    """2026-07-28 无状态: tools/list + 真实IP调用 + discover + -32022"""
    tag = f"[2026|{ch_name}]"
    meta = {"_meta": {PV: "2026-07-28", **CLIENT_META}}
    st, sid, obj, _ = req("POST", base, {"jsonrpc": "2.0", "id": 1, "method": "tools/list",
        "params": {"_meta": {PV: "2026-07-28"}}}, {"MCP-Protocol-Version": "2026-07-28"})
    tools = obj.get("result", {}).get("tools", []) if obj else []
    tname = next((t["name"] for t in tools if t["name"].endswith("getPublicIp")), None)
    if ch_name == "smart":
        check(f"{tag} tools/list meta工具暴露", st == 200 and len(tools) > 0, f"st={st} n={len(tools)}")
    else:
        check(f"{tag} tools/list 无状态放行", st == 200 and tname, f"st={st} n={len(tools)}")
    check(f"{tag} CacheableResult(ttlMs+resultType)", obj and obj.get("result", {}).get("resultType") == "complete" and "ttlMs" in obj.get("result", {}), str(obj)[:80])
    if tname:
        st, _, obj, _ = req("POST", base, {"jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": tname, "arguments": {}, **meta}}, {"MCP-Protocol-Version": "2026-07-28"})
        txt = "".join(str(c.get("text", "")) for c in (obj.get("result", {}) or {}).get("content", [])) if obj else ""
        ok = st == 200 and obj and (obj.get("result", {}) or {}).get("isError") is False and len(txt) > 7
        check(f"{tag} 公网IP真实调用(无状态)", ok, f"st={st} txt={txt[:40]}")
    st, _, obj, _ = req("POST", base, {"jsonrpc": "2.0", "id": 3, "method": "server/discover", "params": {"_meta": {PV: "2026-07-28"}}}, {"MCP-Protocol-Version": "2026-07-28"})
    r = obj.get("result", {}) if obj else {}
    check(f"{tag} server/discover", st == 200 and r.get("resultType") == "complete" and "supportedVersions" in r, str(r)[:80])
    st, _, obj, _ = req("POST", base, {"jsonrpc": "2.0", "id": 4, "method": "tools/list",
        "params": {"_meta": {PV: "2099-01-01"}}})
    e = obj.get("error", {}) if obj else {}
    check(f"{tag} 未知版本 -32022", e.get("code") == -32022 and "requested" in json.dumps(e), str(e)[:80])

def set_strict(on):
    con = sqlite3.connect(DB)
    cfg = json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
    cfg.setdefault("mcp", {})["strictValidation"] = on
    con.execute("UPDATE system_config SET config_json=?, updated_at=datetime('now') WHERE rowid=1", (json.dumps(cfg, ensure_ascii=False),))
    con.commit(); con.close()

# ════ 阶段 1: 宽松模式（默认）全版本×全通道 ════
print("== 阶段1: 宽松模式 5版本×4通道 ==")
set_strict(False)
for v in VERSIONS:
    for ch_name, base, prefix in CHANNELS:
        if v == "2026-07-28":
            modern_channel(ch_name, base, prefix)
        else:
            legacy_lifecycle(ch_name, base, prefix, v)

# ════ 阶段 2: 严格模式：规范请求全通过 + 缺陷请求全拒绝 ════
print("== 阶段2: 严格模式 ==")
set_strict(True)
try:
    time.sleep(0.3)
    for v in VERSIONS:
        for ch_name, base, prefix in CHANNELS:
            tag = f"[严格|{v}|{ch_name}]"
            if v == "2026-07-28":
                st, sid, obj, _ = req("POST", base, {"jsonrpc": "2.0", "id": 1, "method": "tools/list",
                    "params": {"_meta": {PV: v, **CLIENT_META}}},
                    {"MCP-Protocol-Version": v, "Mcp-Method": "tools/list"})
                ok = st == 200 and obj and "result" in obj
                check(f"{tag} 规范请求通过", ok, f"st={st} {str(obj)[:80]}")
            else:
                st, sid, obj, _ = req("POST", base, {"jsonrpc": "2.0", "id": 1, "method": "initialize",
                    "params": {"protocolVersion": v, "capabilities": {}, "clientInfo": {"name": "c", "version": "1"}}})
                ok = st == 200 and obj and "result" in obj and obj["result"].get("protocolVersion") == v
                check(f"{tag} 规范 initialize 通过+回显", ok, f"st={st} {str(obj)[:80]}")
                if ok and sid:
                    st, _, obj, _ = req("POST", base, {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
                        {"Mcp-Session-Id": sid, "MCP-Protocol-Version": v})
                    check(f"{tag} 规范 tools/list 通过", st == 200 and obj and "result" in obj, f"st={st}")
                    req("DELETE", base, None, {"Mcp-Session-Id": sid})
    st, _, obj, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}})
    check("[严格] 缺Accept 拒绝(406/422)", st in (406, 422), f"st={st}")
    st, _, obj, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 1, "method": "tools/list",
        "params": {"_meta": {PV: "2026-07-28"}}})
    check("[严格] 2026 缺 client 元数据拒绝", st in (400, 406, 422), f"st={st}")
finally:
    set_strict(False)

print(f"\n== {len(PASS)}/{len(PASS)+len(FAIL)} passed ==")
if FAIL:
    print("FAILED:", *FAIL, sep="\n  - ")
    sys.exit(1)
