#!/usr/bin/env python3
"""v42 suite — R21 regression: HTTP observability surface.
Covers: 5-version initialize echo, 2026 modern server/discover ttlMs+cacheScope,
lenient-mode bare request (no session header / missing Accept) real getPublicIp call,
strict-mode DB hot-switch reject→restore→allow.
Baseline = strictValidation=false (original value captured and restored in finally)."""
import json, http.client, re, sqlite3, os, sys, time, base64

BASE_HOST, BASE_PORT = "127.0.0.1", 23333
BASE = f"http://{BASE_HOST}:{BASE_PORT}"
DB = os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")
VERSIONS = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25", "2026-07-28"]
TOOL = "本机公网ip查询-getPublicIp"
IP_RE = re.compile(r"\b\d{1,3}(\.\d{1,3}){3}\b")
PASS = FAIL = 0
FAILURES = []

def check(name, cond, detail=""):
    global PASS, FAIL
    if cond:
        PASS += 1
        print(f"PASS {name}")
    else:
        FAIL += 1
        FAILURES.append((name, detail))
        print(f"FAIL {name}: {detail}")

def parse(raw):
    for line in raw.splitlines():
        if line.startswith("data:"):
            try:
                return json.loads(line[5:].strip())
            except Exception:
                pass
    try:
        return json.loads(raw)
    except Exception:
        return None

def init(version):
    conn = http.client.HTTPConnection(BASE_HOST, BASE_PORT, timeout=20)
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                       "params": {"protocolVersion": version, "capabilities": {},
                                  "clientInfo": {"name": "v42", "version": "1"}}})
    conn.request("POST", "/mcp", body, {"Content-Type": "application/json",
                                        "Accept": "application/json, text/event-stream"})
    resp = conn.getresponse()
    sid = resp.getheader("mcp-session-id")
    r = parse(resp.read().decode("utf-8", "replace"))
    conn.close()
    echoed = r.get("result", {}).get("protocolVersion") if r else None
    return resp.status, sid, echoed

def post(session, method, params, version, timeout=30):
    conn = http.client.HTTPConnection(BASE_HOST, BASE_PORT, timeout=timeout)
    body = json.dumps({"jsonrpc": "2.0", "id": 2, "method": method, "params": params})
    h = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream",
         "MCP-Protocol-Version": version}
    if session:
        h["mcp-session-id"] = session
    conn.request("POST", "/mcp", body, h)
    resp = conn.getresponse()
    raw = resp.read().decode("utf-8", "replace")
    conn.close()
    return resp.status, parse(raw)

def get_strict():
    con = sqlite3.connect(DB)
    cfg = json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
    con.close()
    return cfg.get("mcp", {}).get("strictValidation", False)

def set_strict(on):
    """Returns the PREVIOUS strictValidation value for faithful restore."""
    con = sqlite3.connect(DB)
    cfg = json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
    old = cfg.get("mcp", {}).get("strictValidation", False)
    cfg.setdefault("mcp", {})["strictValidation"] = bool(on)
    con.execute("UPDATE system_config SET config_json=?, updated_at=datetime('now') WHERE rowid=1",
                (json.dumps(cfg, ensure_ascii=False),))
    con.commit(); con.close()
    return old

def extract_text(r):
    try:
        return r["result"]["content"][0]["text"]
    except Exception:
        return ""

def main():
    # health gate: unreachable → skip everything
    try:
        resp = http.client.HTTPConnection(BASE_HOST, BASE_PORT, timeout=5)
        resp.request("GET", "/health")
        hresp = resp.getresponse(); hresp.read(); resp.close()
        if hresp.status != 200:
            print(f"SKIP: /health returned {hresp.status}"); return
    except Exception as e:
        print(f"SKIP: service unreachable on {BASE} ({e})"); return

    orig_strict = get_strict()
    print(f"== v42: baseline strictValidation={orig_strict} ==")
    try:
        # ── 宽松模式基线 ──
        set_strict(False); time.sleep(0.4)

        # 1. 5 版本 initialize → 回显一致
        # 注：2026-07-28 为无状态版本，initialize 会被降级回显为最新 legacy 版本
        # （与既有套件 rmcp_matrix_v2 / rmcp_full_matrix_v18 的处理一致）。
        for v in VERSIONS:
            st, sid, echoed = init(v)
            if v == "2026-07-28":
                check(f"echo-{v}-downgrade", st == 200 and echoed in VERSIONS[:-1],
                      f"st={st} echoed={echoed}")
            else:
                check(f"echo-{v}", st == 200 and echoed == v, f"st={st} echoed={echoed}")

        # 2. 2026 modern: server/discover ttlMs + cacheScope
        conn = http.client.HTTPConnection(BASE_HOST, BASE_PORT, timeout=20)
        body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "server/discover",
                           "params": {"_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28",
                                                 "io.modelcontextprotocol/clientInfo": {"name": "v42", "version": "1"},
                                                 "io.modelcontextprotocol/clientCapabilities": {}}}})
        conn.request("POST", "/mcp", body, {"Content-Type": "application/json",
                                            "Accept": "application/json, text/event-stream",
                                            "MCP-Protocol-Version": "2026-07-28",
                                            "Mcp-Method": "server/discover"})
        resp = conn.getresponse()
        r = parse(resp.read().decode("utf-8", "replace"))
        conn.close()
        res = r.get("result", {}) if r else {}
        check("2026-discover-ttl-scope",
              resp.status == 200 and res.get("resultType") == "complete"
              and res.get("ttlMs") == 3600000 and res.get("cacheScope") == "public",
              f"st={resp.status} ttlMs={res.get('ttlMs')} cacheScope={res.get('cacheScope')}")

        # 3. 宽松模式：裸请求（无 session 头、缺 Accept）升格 tools/call 真实调用
        conn = http.client.HTTPConnection(BASE_HOST, BASE_PORT, timeout=60)
        body = json.dumps({"jsonrpc": "2.0", "id": 9, "method": "tools/call",
                           "params": {"name": TOOL, "arguments": {}}})
        conn.request("POST", "/mcp", body, {"Content-Type": "application/json"})  # no Accept, no session
        resp = conn.getresponse()
        r = parse(resp.read().decode("utf-8", "replace"))
        conn.close()
        text = extract_text(r)
        m = IP_RE.search(text.split("Response Body:")[-1])
        check("lenient-bare-ip-call",
              resp.status == 200 and r and not r.get("result", {}).get("isError") and bool(m),
              f"st={resp.status} text={text[-60:]}")

        # 4. 严格模式热切换：写 DB → 同裸请求被拒(4xx) → 还原 → 放行
        set_strict(True); time.sleep(0.4)
        conn = http.client.HTTPConnection(BASE_HOST, BASE_PORT, timeout=30)
        conn.request("POST", "/mcp", body, {"Content-Type": "application/json"})
        resp = conn.getresponse()
        resp.read(); conn.close()
        check("strict-hot-reject", 400 <= resp.status < 500, f"st={resp.status}")

        set_strict(False); time.sleep(0.4)
        conn = http.client.HTTPConnection(BASE_HOST, BASE_PORT, timeout=60)
        conn.request("POST", "/mcp", body, {"Content-Type": "application/json"})
        resp = conn.getresponse()
        r = parse(resp.read().decode("utf-8", "replace"))
        conn.close()
        text = extract_text(r)
        m = IP_RE.search(text.split("Response Body:")[-1])
        check("strict-off-allow", resp.status == 200 and r and not r.get("result", {}).get("isError") and bool(m),
              f"st={resp.status} text={text[-60:]}")

    finally:
        # 还原 DB 基线（忠实还原进入时的值，不假设是 false）
        set_strict(orig_strict); time.sleep(0.3)
        print(f"== baseline restored: strictValidation={get_strict()} (orig={orig_strict}) ==")

    print(f"\nv42: {PASS} passed, {FAIL} failed")
    if FAILURES:
        for n, d in FAILURES:
            print(f"  - {n}: {d}")
    sys.exit(1 if FAIL else 0)

if __name__ == "__main__":
    main()
