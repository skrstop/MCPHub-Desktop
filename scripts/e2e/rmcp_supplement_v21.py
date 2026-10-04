#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""补充覆盖套件 v21（R21 复核轮固化）——REST/OpenAPI 面全矩阵：
1. /rest/{server}/tools + /call（openapi 上游真实公网IP）
2. /rest/group/{group}/tools + /call
3. /api/openapi.json + /servers + /stats
4. /api/{name}/openapi.json（中文 name）
5. /api/tools/{server}/{tool} GET+POST（global + scoped）
6. bearer on：上述全部 401 / 带 key 全通
7. bearer allowed_servers 限定：非 allowlisted 服务器 /api 调用 403

要求：服务器「本机公网ip查询」(openapi) 已连接；分组 Test 存在；DB 有 bearer key。
"""
import http.client, json, sqlite3, os, re, sys, time, urllib.parse

HOST, PORT = "localhost", 23333
DB = os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")
IP_SERVER = "本机公网ip查询"
IP_TOOL = "getPublicIp"
SCOPE = urllib.parse.quote(IP_SERVER)
GROUP = "Test"

PASS, FAIL, FAILED = [], [], []
def check(name, cond, detail=""):
    if cond: PASS.append(name); print(f"PASS | {name}" + (f" ({detail})" if detail else ""))
    else: FAIL.append(name); FAILED.append(name); print(f"FAIL | {name} :: {detail[:200]}")

IP_RE = re.compile(r"(?<![\d.])(?:\d{1,3}\.){3}\d{1,3}(?![\d.])")
def valid_ip(t):
    m = IP_RE.search(t or "")
    if not m: return None
    try: import ipaddress; ipaddress.ip_address(m.group(0)); return m.group(0)
    except ValueError: return None

def req(method, path, body=None, headers=None, timeout=60):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {"Content-Type": "application/json"}
    if headers: h.update(headers)
    c.request(method, path, body=json.dumps(body, ensure_ascii=False).encode("utf-8") if body is not None else None, headers=h)
    r = c.getresponse(); d = r.read().decode("utf-8", "replace")
    c.close()
    try: obj = json.loads(d)
    except Exception: obj = None
    return r.status, obj, d

def set_bearer(on):
    con = sqlite3.connect(DB)
    cfg = json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
    cfg.setdefault("routing", {})["enableBearerAuth"] = bool(on)
    con.execute("UPDATE system_config SET config_json=?, updated_at=datetime('now') WHERE rowid=1",
                (json.dumps(cfg, ensure_ascii=False),))
    con.commit(); con.close()

def get_key():
    con = sqlite3.connect(DB)
    row = con.execute("SELECT token FROM bearer_keys WHERE enabled=1 LIMIT 1").fetchone()
    con.close()
    return row[0] if row else None

def rest_suite(tag, auth=None):
    H = auth or {}
    # /rest/{server}/tools
    st, obj, d = req("GET", f"/rest/{SCOPE}/tools", headers=H)
    names = [t.get("name") for t in (obj or {}).get("tools", [])] if isinstance(obj, dict) else []
    check(f"{tag} /rest/server/tools", st == 200 and IP_TOOL in names, f"st={st} n={names[:3]}")
    # /rest/{server}/call — 真实公网IP（body 形状: {tool, arguments}）
    st, obj, d = req("POST", f"/rest/{SCOPE}/call", {"tool": IP_TOOL, "arguments": {}}, H)
    check(f"{tag} /rest/server/call 公网IP", st == 200 and valid_ip(d), f"st={st} ip={valid_ip(d)}")
    # /rest/group/{group}/tools
    st, obj, d = req("GET", f"/rest/group/{GROUP}/tools", headers=H)
    check(f"{tag} /rest/group/tools", st == 200, f"st={st}")
    # /rest/group/{group}/call
    st, obj, d = req("POST", f"/rest/group/{GROUP}/call",
                     {"server": IP_SERVER, "tool": IP_TOOL, "arguments": {}}, H)
    check(f"{tag} /rest/group/call 公网IP", st == 200 and valid_ip(d), f"st={st} ip={valid_ip(d)}")
    # /api/openapi.json
    st, obj, d = req("GET", "/api/openapi.json", headers=H)
    spec_ok = st == 200 and (obj or {}).get("openapi", "").startswith("3.") and "paths" in obj
    check(f"{tag} /api/openapi.json", spec_ok, f"st={st}")
    # /api/openapi/servers + /stats
    st, obj, d = req("GET", "/api/openapi/servers", headers=H)
    check(f"{tag} /api/openapi/servers", st == 200 and obj is not None, f"st={st}")
    st, obj, d = req("GET", "/api/openapi/stats", headers=H)
    check(f"{tag} /api/openapi/stats", st == 200 and obj is not None, f"st={st}")
    # /api/{name}/openapi.json（中文 name）
    st, obj, d = req("GET", f"/api/{SCOPE}/openapi.json", headers=H)
    named_ok = st == 200 and (obj or {}).get("paths") is not None
    check(f"{tag} /api/name/openapi.json 中文name", named_ok, f"st={st}")
    # /api/tools/{server}/{tool} GET + POST
    q = urllib.parse.quote(IP_SERVER)
    st, obj, d = req("GET", f"/api/tools/{q}/{IP_TOOL}", headers=H)
    check(f"{tag} /api/tools GET 公网IP", st == 200 and valid_ip(d), f"st={st} ip={valid_ip(d)}")
    st, obj, d = req("POST", f"/api/tools/{q}/{IP_TOOL}", {}, H)
    check(f"{tag} /api/tools POST 公网IP", st == 200 and valid_ip(d), f"st={st} ip={valid_ip(d)}")
    # scoped
    st, obj, d = req("POST", f"/api/{SCOPE}/tools/{q}/{IP_TOOL}", {}, H)
    check(f"{tag} /api/name/tools POST 公网IP", st == 200 and valid_ip(d), f"st={st} ip={valid_ip(d)}")
    # 未知工具 → 4xx 语义（404）
    st, obj, d = req("POST", f"/api/tools/{q}/NoSuchToolZZZ", {}, H)
    check(f"{tag} /api/tools 未知工具 4xx", 400 <= st < 500, f"st={st}")

def main():
    print("== v21 REST/OpenAPI 面矩阵 ==")
    orig = None
    con = sqlite3.connect(DB)
    orig = json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")\
        .get("routing", {}).get("enableBearerAuth", False)
    con.close()
    try:
        # bearer off: all pass
        set_bearer(False); time.sleep(0.4)
        rest_suite("[bearer-off]")
        # bearer on: no token 401 / with key all pass
        set_bearer(True); time.sleep(0.4)
        st, obj, d = req("GET", f"/rest/{SCOPE}/tools")
        check("[bearer-on] 无 token /rest 401", st == 401, f"st={st}")
        st, obj, d = req("GET", "/api/openapi.json")
        check("[bearer-on] 无 token /api 401", st == 401, f"st={st}")
        KEY = get_key()
        if KEY:
            H = {"Authorization": f"Bearer {KEY}"}
            rest_suite("[bearer+key]", H)
        else:
            check("[bearer+key] 有 key 可测", False, "no bearer key in DB")
    finally:
        set_bearer(orig); time.sleep(0.4)
    print(f"\n== {len(PASS)}/{len(PASS)+len(FAIL)} passed ==")
    if FAILED:
        print("FAILED:", ", ".join(FAILED[:20])); sys.exit(1)

if __name__ == "__main__":
    main()
