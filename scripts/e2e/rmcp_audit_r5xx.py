#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
rmcp_audit_r5xx.py — 补充套件（第 5xx 轮审计产出）
N1  trailing-slash route gate：/mcp/$smart/ /mcp/{group}/ /mcp/%24smart/ 三形态
N6  toolDefinitionFields × REST /api/$smart/search 字段一致性
N9  配置卫生：套件首尾快照开关，尾部断言无泄漏
前置：应用运行于 127.0.0.1:23333，openapi IP 服务器已连接，smartRouting.enabled=1
"""
import http.client, json, sqlite3, sys, time, re, ipaddress
from pathlib import Path
from urllib.parse import quote

HOST, PORT = "127.0.0.1", 23333
PASS, FAIL, SKIP = [], [], []

def check(name, cond, detail=""):
    if cond: PASS.append(name); print(f"PASS | {name}")
    else: FAIL.append(name); print(f"FAIL | {name} | {detail}")

def raw_post(path, body, headers=None, timeout=90):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
    if headers: h.update(headers)
    c.request("POST", path, json.dumps(body), h)
    r = c.getresponse(); data = r.read().decode("utf-8", "replace")
    sid = r.getheader("mcp-session-id")
    c.close()
    return r.status, data, sid

def raw_get(path, timeout=60):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    c.request("GET", path, None, {"Accept": "application/json"})
    r = c.getresponse(); data = r.read().decode("utf-8", "replace"); c.close()
    return r.status, data

def raw_delete(path, sid):
    try:
        c = http.client.HTTPConnection(HOST, PORT, timeout=10)
        c.request("DELETE", path, None, {"mcp-session-id": sid})
        c.getresponse().read(); c.close()
    except Exception:
        pass

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

def db_path():
    home = Path.home()
    for cand in [home / "Library/Application Support/app.mcphub.desktop/mcphub.db",
                 home / ".config/app.mcphub.desktop/mcphub.db",
                 home / "AppData/Roaming/app.mcphub.desktop/mcphub.db"]:
        if cand.exists(): return str(cand)
    return None

def cfg_read():
    con = sqlite3.connect(DB)
    row = con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()
    con.close()
    return json.loads(row[0] or "{}") if row else {}

def cfg_write(cfg):
    con = sqlite3.connect(DB)
    con.execute("UPDATE system_config SET config_json=?, updated_at=datetime('now') WHERE rowid=1",
                (json.dumps(cfg, ensure_ascii=False),))
    con.commit(); con.close()

def set_routing(key, val):
    c = cfg_read()
    old = c.get("routing", {}).get(key)
    c.setdefault("routing", {})[key] = bool(val)
    cfg_write(c)
    return old

def group_names():
    con = sqlite3.connect(DB)
    rows = con.execute("SELECT name FROM groups LIMIT 10").fetchall()
    con.close()
    return [r[0] for r in rows]

def main():
    global DB
    DB = db_path()
    if not DB:
        print("SKIP: DB not found"); sys.exit(0)
    def hygiene():
        c = cfg_read()
        return {k: c.get("mcp", {}).get("strictValidation") if k == "strict"
                else c.get("routing", {}).get(k if k != "bearer" else "enableBearerAuth")
                for k in ["strict", "bearer", "enableGlobalRoute", "enableGroupNameRoute"]}
    h0 = hygiene()
    print(f"baseline hygiene: {h0}")

    # ── N1: trailing-slash route gate ────────────────────────────────────────
    print("== N1: trailing-slash route gate ==")
    for p in ["/mcp/$smart/", "/mcp/%24smart/"]:
        st, data, sid = raw_post(p, {"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-11-25", "capabilities": {},
                       "clientInfo": {"name": "r5xx", "version": "1"}}})
        o = sse_obj(data, 1)
        ok = st == 200 and o and "result" in o
        check(f"N1[{p}] 开关全开 initialize 200(全局形态)", ok, f"st={st} {data[:100]}")
        if ok and sid:
            st2, d2, _ = raw_post(p, {"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": {"name": "smart_route_search", "arguments": {"query": "ip", "limit": 3}}},
                {"mcp-session-id": sid})
            check(f"N1[{p}] tools/call 非5xx", st2 < 500, f"st={st2}")
            raw_delete(p, sid)
    old_g = set_routing("enableGlobalRoute", False); time.sleep(0.4)
    try:
        for p in ["/mcp", "/mcp/$smart/", "/mcp/%24smart/"]:
            st, _, _ = raw_post(p, {"jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": "2025-11-25", "capabilities": {},
                           "clientInfo": {"name": "r5xx", "version": "1"}}})
            check(f"N1[{p}] globalRoute=off → 404", st == 404, f"st={st}")
    finally:
        set_routing("enableGlobalRoute", True if old_g is None else old_g); time.sleep(0.4)
    gname = next((g for g in group_names() if g and not g.startswith("$")), None)
    if gname:
        old_gr = set_routing("enableGroupNameRoute", False); time.sleep(0.4)
        try:
            st, _, _ = raw_post(f"/mcp/{quote(gname, safe='')}/", {"jsonrpc": "2.0", "id": 1,
                "method": "initialize", "params": {"protocolVersion": "2025-11-25",
                "capabilities": {}, "clientInfo": {"name": "r5xx", "version": "1"}}})
            check(f"N1[/mcp/{gname}/] groupRoute=off → 404", st == 404, f"st={st}")
        finally:
            set_routing("enableGroupNameRoute", True if old_gr is None else old_gr); time.sleep(0.4)
    else:
        SKIP.append("N1 group form"); print("SKIP | N1 group form（无分组）")

    # ── N6: toolDefinitionFields × REST search ───────────────────────────────
    print("== N6: toolDefinitionFields × REST /api/$smart/search ==")
    st, data = raw_get("/api/$smart/search?query=ip&limit=5")
    if st != 200:
        SKIP.append("N6"); print(f"SKIP | N6 st={st}")
    else:
        def _unwrap(d):
            try:
                o = json.loads(d)
                if isinstance(o, dict) and "content" in o:
                    return json.loads(o["content"][0]["text"])
                return o
            except Exception:
                return {}
        base_o = _unwrap(data)
        base_tools = base_o.get("tools") or []
        fields_hit = [t for t in base_tools if "annotations" in t or "outputSchema" in t]
        check("N6 REST search 200 且有条目", len(base_tools) > 0, f"n={len(base_tools)}")
        c = cfg_read()
        sr = c.get("smartRouting", {})
        saved_fields = sr.get("toolDefinitionFields")
        sr["toolDefinitionFields"] = []
        cfg_write(c); time.sleep(0.6)
        try:
            st2, d2 = raw_get("/api/$smart/search?query=ip&limit=5")
            t2 = _unwrap(d2).get("tools") or []
            bad = [t for t in t2 if any(k in t for k in ("annotations", "outputSchema", "icons", "_meta", "execution"))]
            check("N6 fields=[] REST 条目无 optional 字段", st2 == 200 and not bad, f"bad={len(bad)}")
        finally:
            c = cfg_read()
            if saved_fields is None: c.setdefault("smartRouting", {}).pop("toolDefinitionFields", None)
            else: c["smartRouting"]["toolDefinitionFields"] = saved_fields
            cfg_write(c); time.sleep(0.6)
        c = cfg_read()
        c["smartRouting"]["toolDefinitionFields"] = ["title", "annotations", "outputSchema", "execution", "icons", "_meta"]
        cfg_write(c); time.sleep(0.6)
        try:
            st3, d3 = raw_get("/api/$smart/search?query=ip&limit=5")
            t3 = _unwrap(d3).get("tools") or []
            ann = [t for t in t3 if "annotations" in t]
            if fields_hit:
                check("N6 fields 全开 REST annotations 出现", st3 == 200 and len(ann) > 0, f"ann={len(ann)}")
            else:
                SKIP.append("N6 positive"); print("SKIP | N6 正向（上游无 annotations 样例）")
        finally:
            c = cfg_read()
            if saved_fields is None: c.setdefault("smartRouting", {}).pop("toolDefinitionFields", None)
            else: c["smartRouting"]["toolDefinitionFields"] = saved_fields
            cfg_write(c); time.sleep(0.6)

    # ── N9: hygiene no-leak ──────────────────────────────────────────────────
    h1 = hygiene()
    for k in h0:
        check(f"N9 配置无泄漏({k})", h0[k] == h1[k], f"{h0[k]}→{h1[k]}")

    print(f"\n{'='*50}\nPASS={len(PASS)} FAIL={len(FAIL)} SKIP={len(SKIP)}")
    if FAIL: print("FAILED:", *FAIL, sep="\n  - ")
    sys.exit(1 if FAIL else 0)

if __name__ == "__main__":
    main()
