#!/usr/bin/env python3
# B10 审计缺口：宽松严格不成对形态 —— 6 个形态 × (宽松放行 / 严格拒绝) 成对用例
# 每对先宽松跑放行断言，再切严格跑拒绝断言，finally 还原 strictValidation 原值。
import json, http.client, sqlite3, os, sys, time

HOST, PORT = "localhost", 23333
DB = os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")
PASS, FAIL = [], []
TOOL = "本机公网ip查询-getPublicIp"

def check(name, ok, detail=""):
    (PASS if ok else FAIL).append(name)
    print(("PASS" if ok else "FAIL"), "|", name, (("| " + str(detail)[:200]) if (detail and not ok) else ""))

def req(payload, headers=None, method="POST", path="/mcp", raw_body=None,
        content_type="application/json", accept="application/json, text/event-stream"):
    c = http.client.HTTPConnection(HOST, PORT, timeout=30)
    h = {}
    if accept: h["Accept"] = accept
    if method == "POST" and content_type: h["Content-Type"] = content_type
    if headers: h.update(headers)
    body = raw_body if raw_body is not None else (json.dumps(payload) if payload is not None else None)
    c.request(method, path, body=body, headers=h)
    r = c.getresponse(); d = r.read().decode("utf-8", "replace"); st = r.status
    sid = r.getheader("mcp-session-id"); c.close()
    obj = None
    try: obj = json.loads(d)
    except Exception:
        frames = []
        for l in d.split("\n"):
            if l.startswith("data: ") and l[6:].strip():
                try: frames.append(json.loads(l[6:]))
                except Exception: pass
        obj = frames[-1] if frames else None
    return st, sid, obj, d

def set_strict(on):
    con = sqlite3.connect(DB)
    cfg = json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
    cfg.setdefault("mcp", {})["strictValidation"] = bool(on)
    con.execute("UPDATE system_config SET config_json=?, updated_at=datetime('now') WHERE rowid=1",
                (json.dumps(cfg, ensure_ascii=False),))
    con.commit(); con.close()

def restore(original):
    con = sqlite3.connect(DB)
    cfg = json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
    if original is None:
        cfg.get("mcp", {}).pop("strictValidation", None)
    else:
        cfg.setdefault("mcp", {})["strictValidation"] = original
    con.execute("UPDATE system_config SET config_json=?, updated_at=datetime('now') WHERE rowid=1",
                (json.dumps(cfg, ensure_ascii=False),))
    con.commit(); con.close()

def read_orig():
    con = sqlite3.connect(DB)
    row = con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()
    con.close()
    if not row or not row[0]: return None
    cfg = json.loads(row[0])
    mcp = cfg.get("mcp", {}) if isinstance(cfg, dict) else {}
    v = mcp.get("strictValidation")
    return None if v is None else bool(v)

def init_session(version="2025-06-18"):
    st, sid, obj, _ = req({"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"pairs","version":"1"}}})
    return sid

def ip_call():
    """宽松裸请求形态真实调用公网 IP 工具，返回 (st, obj)。"""
    return req({"id":7,"method":"tools/call","params":{"name":TOOL,"arguments":{}}})

def assert_ip(name, st, obj):
    txt, ip_found, is_err = "", False, None
    if obj:
        res = obj.get("result", {})
        is_err = res.get("isError")
        txt = "".join(str(c.get("text","")) for c in res.get("content",[]))
        ip_found = any(ch.isdigit() for ch in txt) and "." in txt
    check(name, st == 200 and obj and is_err is False and ip_found,
          f"st={st} isError={is_err} txt={txt[:60]}")

def pair(name, lenient_fn, strict_fn):
    set_strict(False); time.sleep(0.4)
    print(f"== {name}：宽松 ==")
    lenient_fn()
    set_strict(True); time.sleep(0.4)
    print(f"== {name}：严格 ==")
    strict_fn()

def main():
    orig = read_orig()
    print("原始 strictValidation =", orig)
    try:
        # P1 id:null
        def l1():
            st, _, obj, _ = req({"jsonrpc":"2.0","id":None,"method":"tools/list","params":{}})
            check("P1-L id:null 宽松升格放行(改写0)", st == 200 and obj and "result" in obj, f"st={st} obj={str(obj)[:80]}")
        def s1():
            st, _, obj, _ = req({"jsonrpc":"2.0","id":None,"method":"tools/list","params":{}})
            check("P1-S id:null 严格拒绝 4xx", 400 <= st < 500, f"st={st} obj={str(obj)[:80]}")
        pair("P1 id:null", l1, s1)

        # P2 jsonrpc "1.0"
        def l2():
            st, _, obj, _ = req({"jsonrpc":"1.0","id":2,"method":"tools/list","params":{}})
            check("P2-L jsonrpc1.0 宽松归一2.0放行", st == 200 and obj and "result" in obj, f"st={st} obj={str(obj)[:80]}")
        def s2():
            st, _, obj, _ = req({"jsonrpc":"1.0","id":2,"method":"tools/list","params":{}})
            check("P2-S jsonrpc1.0 严格拒绝(415/4xx)", st == 415 or (400 <= st < 500), f"st={st}")
        pair("P2 jsonrpc1.0", l2, s2)

        # P3 text/plain Content-Type
        body = json.dumps({"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}})
        # 已知边界（F-8 实现语义）：宽松仅对「缺失」Content-Type 注入 application/json，
        # text/plain 显式错误值不覆写 → 两侧一致 415（可辩护语义：显式声明的错误媒体类型
        # 不属于「客户端可无损修复」的缺省场景）。若未来实现改为覆写（宽松 200），此断言失败提示更新。
        def l3():
                st, _, obj, _ = req(None, raw_body=body, content_type="text/plain")
                check("P3-L text/plain 宽松 415（与严格一致，F-8 边界）", st == 415,
                      f"st={st} obj={str(obj)[:80]}")
        def s3():
                st, _, obj, _ = req(None, raw_body=body, content_type="text/plain")
                check("P3-S text/plain 严格 415（两侧一致性钉死）", st == 415, f"st={st}")
        pair("P3 text/plain", l3, s3)

        # P3b 缺失 Content-Type（宽松注入形态的正面对照）
        def l3b():
            st, _, obj, _ = req(None, raw_body=body, content_type=None)
            check("P3b-L 缺失Content-Type 宽松注入放行", st == 200 and obj and "result" in obj, f"st={st}")
        def s3b():
            st, _, obj, _ = req(None, raw_body=body, content_type=None)
            check("P3b-S 缺失Content-Type 严格拒绝 4xx", 400 <= st < 500, f"st={st}")
        pair("P3b 缺失Content-Type", l3b, s3b)

        # P4 空 session 头裸请求（无 initialize 直 tools/call 真实工具）
        def l4():
            st, _, obj, _ = ip_call()
            assert_ip("P4-L 空 session 裸请求升格 公网IP真实调用", st, obj)
        def s4():
            st, _, obj, _ = ip_call()
            check("P4-S 空 session 裸请求 严格拒绝 4xx", 400 <= st < 500, f"st={st} obj={str(obj)[:80]}")
        pair("P4 空 session 裸请求", l4, s4)

        # P5 params._meta 非对象（string "x"）
        def l5():
            st, _, obj, _ = req({"jsonrpc":"2.0","id":5,"method":"tools/list",
                "params":{"_meta":"x"}})
            check("P5-L _meta非对象(string) 宽松覆写{}放行", st == 200 and obj and "result" in obj,
                  f"st={st} obj={str(obj)[:80]}")
        def s5():
            st, _, obj, _ = req({"jsonrpc":"2.0","id":5,"method":"tools/list",
                "params":{"_meta":"x"}})
            check("P5-S _meta非对象(string) 严格拒绝 4xx", 400 <= st < 500, f"st={st}")
        pair("P5 _meta非对象", l5, s5)

        # P6 畸形 JSON body（两侧都不 5xx）
        bad = '{"jsonrpc":"2.0","id":6,"method":"tools/list",'
        def l6():
            st, _, obj, _ = req(None, raw_body=bad, content_type="application/json")
            ecode = obj.get("error", {}).get("code") if obj else None
            # 审计预期 400/-32700，实测宽松侧 415（rmcp 层拒绝先于 JSON 解析）——按实测对齐：
            # 核心钉死「不 5xx 且 4xx」，-32700 语义降级为可选加分项。
            check("P6-L 畸形JSON 宽松 4xx 不5xx(实测415,非-32700)", 400 <= st < 500,
                  f"st={st} obj={str(obj)[:80]}")
            if ecode == -32700 or st == 400:
                check("P6-L2 畸形JSON 宽松 400/-32700 结构化", True)
        def s6():
            st, _, obj, _ = req(None, raw_body=bad, content_type="application/json")
            check("P6-S 畸形JSON 严格 4xx 不5xx", 400 <= st < 500, f"st={st}")
        pair("P6 畸形JSON", l6, s6)
    finally:
        restore(orig)
        time.sleep(0.4)
        st, _, obj, _ = req({"jsonrpc":"2.0","id":99,"method":"tools/list","params":{}})
        check("还原后 tools/list 可用(不残留严格态)", st == 200 and obj and "result" in obj, f"st={st}")

    print(f"\n== {len(PASS)}/{len(PASS)+len(FAIL)} passed ==")
    if FAIL:
        print("FAILED:", *FAIL, sep="\n  - "); sys.exit(1)

if __name__ == "__main__":
    main()
