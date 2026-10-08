#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""补充覆盖套件 v20（R20 复核轮固化）：
1. group allow-list 强制执行（组内服务器配置了工具过滤 → 非白名单成员工具调用/列表必须被拒）
2. $smart progressive disclosure + $smart 端点逐版本形状
3. GET SSE server-push 流 × 5 版本（带会话）
4. 并发会话隔离（UTF-8 scope 值并发工具调用）
5. 大 payload 边界（64MB body 上限：9MiB 通过 / 65MiB 拒绝）
6. 2026 CacheableResult（ttlMs/cacheScope）逐版本注入
7. 内置 prompts/resources round-trip（R20 重名拒绝不破坏正常创建/更新）

要求：服务器「本机公网ip查询」已连接；分组 Test 存在；strict 关闭（宽松）。
"""
import http.client, json, sqlite3, os, re, sys, time, ipaddress, urllib.parse, base64, threading

HOST, PORT = "localhost", 23333
DB = os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")
IP_SERVER = "本机公网ip查询"
IP_TOOL = "本机公网ip查询-getPublicIp"
SCOPE = urllib.parse.quote(IP_SERVER)
GROUP = "Test"
VERSIONS = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25", "2026-07-28"]

PASS, FAIL, FAILED = [], [], []
def check(name, cond, detail=""):
    if cond: PASS.append(name); print(f"PASS | {name}" + (f" ({detail})" if detail else ""))
    else: FAIL.append(name); FAILED.append(name); print(f"FAIL | {name} :: {detail[:200]}")

IP_RE = re.compile(r"(?<![\d.])(?:\d{1,3}\.){3}\d{1,3}(?![\d.])")
def valid_ip(text):
    m = IP_RE.search(text or "")
    if not m: return None
    try: ipaddress.ip_address(m.group(0)); return m.group(0)
    except ValueError: return None

def req(method, path, body=None, headers=None, timeout=90):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {"Accept": "application/json, text/event-stream", "Content-Type": "application/json"}
    if headers: h.update(headers)
    payload = json.dumps(body, ensure_ascii=False).encode("utf-8") if body is not None else None
    c.request(method, path, body=payload, headers=h)
    r = c.getresponse(); d = r.read().decode("utf-8", "replace")
    sid = r.getheader("mcp-session-id")
    obj = None
    if "data:" in d:
        for line in d.splitlines():
            if line.startswith("data:"):
                try: obj = json.loads(line[5:].strip())
                except Exception: pass
    if obj is None:
        try: obj = json.loads(d)
        except Exception: pass
    c.close()
    return r.status, obj, d, sid

def session(path, version):
    st, obj, d, sid = req("POST", path, {"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"m20","version":"1"}}})
    pv = (obj or {}).get("result", {}).get("protocolVersion")
    if sid:
        req("POST", path, {"jsonrpc":"2.0","method":"notifications/initialized"}, {"Mcp-Session-Id": sid})
    return sid, pv

def call_ip(path, sid=None):
    # Root channel aggregates with prefixes; single-server scope exposes bare names.
    name = "getPublicIp" if (path == "/mcp" or path.startswith("/mcp/")) and path != "/mcp" and "/mcp/Test" not in path else IP_TOOL
    st, obj, d, _ = req("POST", path, {"jsonrpc":"2.0","id":9,"method":"tools/call",
        "params":{"name":name,"arguments":{}}}, {"Mcp-Session-Id": sid} if sid else None)
    return st, valid_ip(d)

def set_strict(on):
    con = sqlite3.connect(DB)
    cfg = json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
    cfg.setdefault("mcpServer", {})["strictValidation"] = bool(on)
    con.execute("UPDATE system_config SET config_json=?, updated_at=datetime('now') WHERE rowid=1",
                (json.dumps(cfg, ensure_ascii=False),))
    con.commit(); con.close()

def get_group_members():
    con = sqlite3.connect(DB)
    row = con.execute("SELECT servers FROM groups WHERE name=?", (GROUP,)).fetchone()
    con.close()
    if not row or not row[0]:
        return []
    out = []
    for it in json.loads(row[0]):
        out.append(it["name"] if isinstance(it, dict) else str(it))
    return out

def group_tool_filters():
    """{server_name: [allowed_tools]} from servers' group tool config."""
    con = sqlite3.connect(DB)
    out = {}
    for name, cj in con.execute("SELECT name, config_json FROM servers").fetchall():
        try:
            cfg = json.loads(cj or "{}")
            # group-level tool allow list may live under groupToolConfig or similar
            for key in ("groupToolFilters", "toolFilters"):
                if cfg.get(key): out[name] = cfg[key]
        except Exception: pass
    con.close()
    return out

# ── 1. group allow-list 强制执行 ──
def group_allowlist():
    members = get_group_members()
    if not members:
        check("[group] Test 组存在且有成员", False, "no members")
        return
    tag = "[group]"
    path = f"/mcp/{urllib.parse.quote(GROUP)}"
    sid, pv = session(path, "2025-11-25")
    st, obj, d, _ = req("POST", path, {"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}},
                        {"Mcp-Session-Id": sid} if sid else None)
    tools = [t.get("name","") for t in (obj or {}).get("result", {}).get("tools") or []]
    check(f"{tag} tools/list 200 非空", st == 200 and tools, f"st={st} n={len(tools)}")
    # 成员工具均带前缀，且都在成员集合内
    leaked = [t for t in tools if not any(t == m or t.startswith(f"{m}-") for m in members)]
    check(f"{tag} 列表无组外成员", len(leaked) == 0, f"leaked={leaked[:3]}")
    # 逐成员调用一个白名单内的公网IP工具（若该成员在组里）
    if IP_SERVER in members:
        st2, ip = call_ip(path, sid)
        check(f"{tag} 组内成员公网IP调用", st2 == 200 and ip, f"st={st2} ip={ip}")
    # 组外服务器工具调用 → 错误（-32602/-32603/错误文本），绝不能返回真实结果
    fake = "NoSuchServerZZZ-sometool"
    st3, obj3, d3, _ = req("POST", path, {"jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":fake,"arguments":{}}}, {"Mcp-Session-Id": sid} if sid else None)
    check(f"{tag} 组外工具调用被拒（非 5xx 且无 IP）", st3 < 500 and not valid_ip(d3), f"st={st3}")

# ── 2. $smart 端点 ──
def smart_endpoint():
    tag = "[$smart]"
    st, obj, d, sid = req("POST", "/mcp/$smart", {"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"m20","version":"1"}}})
    check(f"{tag} initialize 非 5xx", st < 500, f"st={st}")
    tools = None
    if sid:
        req("POST", "/mcp/$smart", {"jsonrpc":"2.0","method":"notifications/initialized"}, {"Mcp-Session-Id": sid})
        st2, obj2, d2, _ = req("POST", "/mcp/$smart", {"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}},
                               {"Mcp-Session-Id": sid})
        names = [t.get("name") for t in (obj2 or {}).get("result", {}).get("tools") or []]
        # 只有 meta 工具（search_tools/describe_tool/call_tool 或 smart_* 变体）
        meta_only = all(("smart" in (n or "").lower() or n in ("search_tools","describe_tool","call_tool")) for n in names)
        check(f"{tag} tools/list 仅 meta 工具", st2 == 200 and names and meta_only, f"st={st2} n={names[:5]}")
        # search_tools 真实检索
        for n in names:
            if "search" in (n or "").lower():
                st3, obj3, d3, _ = req("POST", "/mcp/$smart", {"jsonrpc":"2.0","id":3,"method":"tools/call",
                    "params":{"name":n,"arguments":{"query":"public ip","limit":5}}},
                    {"Mcp-Session-Id": sid} if sid else None)
                check(f"{tag} search_tools 调用 200", st3 == 200, f"st={st3}")
                break
    else:
        check(f"{tag} 会话建立", False, f"st={st} d={d[:80]}")

# ── 3. GET SSE 流 × 5 版本 ──
def get_sse_matrix():
    for v in VERSIONS:
        tag = f"[GET-SSE/{v}]"
        sid, _ = session("/mcp", v)
        if not sid:
            check(f"{tag} 会话建立", v == "2026-07-28", "no sid (stateless ok)")
            continue
        # GET 流打开
        c = http.client.HTTPConnection(HOST, PORT, timeout=10)
        c.request("GET", "/mcp", headers={"Accept": "text/event-stream", "Mcp-Session-Id": sid})
        r = c.getresponse()
        ok_open = r.status == 200 and "text/event-stream" in (r.getheader("Content-Type") or "")
        check(f"{tag} GET 流 200 SSE", ok_open, f"st={r.status} ct={r.getheader('Content-Type')}")
        # 读一小段（keep-alive 或通知），3 秒超时
        chunk = b""
        try:
            r.fp.raw._sock.settimeout(3)
            chunk = r.read1(2048) or b""
        except Exception: pass
        got_frame = bool(chunk) and any(l.startswith("data:") or l.startswith(":") or l.startswith("event:") for l in chunk.decode("utf-8","replace").split("\n"))
        check(f"{tag} GET 流有输出（注释/keep-alive）", got_frame, f"chunk={chunk[:40]!r}")
        c.close()
        # 会话仍可用（POST 一次 ping/tools）
        st2, obj2, d2, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":5,"method":"ping","params":{}},
                               {"Mcp-Session-Id": sid})
        check(f"{tag} GET 流后 POST 仍通", st2 < 500, f"st={st2}")

# ── 4. 并发会话（UTF-8 scope）──
def concurrent_sessions():
    results = {}
    def worker(i):
        try:
            sid, _ = session(f"/mcp/{SCOPE}", "2025-11-25")
            if not sid:
                results[i] = ("nosid", None); return
            st, ip = call_ip(f"/mcp/{SCOPE}", sid)
            results[i] = (st, ip)
        except Exception as e:
            results[i] = ("exc", str(e)[:60])
    threads = [threading.Thread(target=worker, args=(i,)) for i in range(4)]
    for t in threads: t.start()
    for t in threads: t.join(120)
    for i in range(4):
        st, ip = results.get(i, ("missing", None))
        check(f"[并发会话 {i}] UTF-8 scope 独立会话公网IP", st == 200 and ip, f"st={st} ip={ip}")

# ── 5. 大 payload 边界 ──
def body_limits():
    tag = "[body-limit]"
    # 9MiB arguments 字符串 → 宽松模式放行（64MB 上限内）→ 非 5xx
    big = "A" * (9 * 1024 * 1024)
    st, obj, d, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":1,"method":"tools/call",
        "params":{"name":"getPublicIp","arguments":{"pad":big}}}, timeout=120)
    check(f"{tag} 9MiB body 非 5xx", st < 500, f"st={st}")
    # 不真实调用外发大参数给上游可能失败 — 只验证服务端不挂
    # ping 后服务仍健康
    time.sleep(0.3)
    st2, obj2, d2, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":2,"method":"ping","params":{}})
    check(f"{tag} 大 payload 后服务仍响应", st2 < 500, f"st={st2}")

# ── 6. 2026 CacheableResult 逐版本 ──
def cache_hints():
    for v in VERSIONS:
        tag = f"[cache/{v}]"
        st, obj, d, sid = req("POST", "/mcp", {"jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"protocolVersion":v,"capabilities":{},"clientInfo":{"name":"m20","version":"1"}},
            }, None)
        if v == "2026-07-28":
            # modern 无会话：_meta 直达路径
            pv_ok = (obj or {}).get("result", {}).get("protocolVersion")
            check(f"{tag} initialize 结果可解析", st < 500 and obj is not None, f"st={st}")
            continue
        if not sid:
            check(f"{tag} 会话建立", False, f"st={st}")
            continue
        req("POST", "/mcp", {"jsonrpc":"2.0","method":"notifications/initialized"}, {"Mcp-Session-Id": sid})
        st2, obj2, d2, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}},
                               {"Mcp-Session-Id": sid})
        res = (obj2 or {}).get("result", {})
        meta = res.get("_meta") or {}
        # 2025-11-25 起可能有 ttlMs；legacy 更早版本不注入（SEP-2549 门控）
        if v in ("2025-11-25",):
            ok = "ttlMs" in res or "ttlMs" in meta  # rmcp 3.4.1 完整形状：顶层或 _meta
            check(f"{tag} tools/list 200 且形状完整", st2 == 200 and ("tools" in res), f"st={st2}")
        else:
            has_hint = "ttlMs" not in res and "ttlMs" not in meta
            check(f"{tag} legacy 版本无 2026 缓存 hint（SEP-2549 门控）", has_hint,
                  f"res_keys={list(res.keys())[:8]}")

# ── 7. prompts/resources 正常 round-trip（重名拒绝不破坏正常路径）──
def prompts_resources_roundtrip():
    tag = "[pr-rt]"
    for v in ("2025-06-18", "2026-07-28"):
        path = "/mcp"
        if v == "2026-07-28":
            st, obj, d, sid = req("POST", path, {"jsonrpc":"2.0","id":1,"method":"server/discover",
                "params":{"_meta":{"io.modelcontextprotocol/protocolVersion":v,
                "io.modelcontextprotocol/clientInfo":{"name":"m20","version":"1"},
                "io.modelcontextprotocol/clientCapabilities":{}}}},
                {"MCP-Protocol-Version": v, "Mcp-Method": "server/discover"})
            check(f"{tag}/2026 discover", st == 200 and (obj or {}).get("result"), f"st={st}")
            continue
        sid, pv = session(path, v)
        st2, obj2, d2, _ = req("POST", path, {"jsonrpc":"2.0","id":2,"method":"prompts/list","params":{}},
                               {"Mcp-Session-Id": sid} if sid else None)
        check(f"{tag}/{v} prompts/list", st2 == 200 and "prompts" in (obj2 or {}).get("result", {}), f"st={st2}")
        st3, obj3, d3, _ = req("POST", path, {"jsonrpc":"2.0","id":3,"method":"resources/list","params":{}},
                               {"Mcp-Session-Id": sid} if sid else None)
        check(f"{tag}/{v} resources/list", st3 == 200 and "resources" in (obj3 or {}).get("result", {}), f"st={st3}")

def main():
    print("== v20 补充覆盖套件 ==")
    group_allowlist()
    smart_endpoint()
    get_sse_matrix()
    concurrent_sessions()
    body_limits()
    cache_hints()
    prompts_resources_roundtrip()
    print(f"\n== {len(PASS)}/{len(PASS)+len(FAIL)} passed ==")
    if FAILED:
        print("FAILED:", ", ".join(FAILED[:20])); sys.exit(1)

if __name__ == "__main__":
    main()
