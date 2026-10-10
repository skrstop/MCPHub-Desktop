#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""E2E round r1700 (round 17): pin ROUND-16 fixes + version×channel convergence.

A  [R16 NAME_COLLISION hard-Err] DISCOVERED: current rag_file_create stores
   content BY UUID ({id}.{docType}, P2b "same-named creates coexist").
   Round-17 fix reworked the guard: it fires ONLY when content_path_for(old)
   RESOLVES TO the bare name path (candidate 4) and exists; uploads bypass
   by resolution (never reach candidate 4); case-only renames short-circuit
   via eq_ignore_ascii_case; the late rename block's foreign-collision
   warn-skip is now a hard Err too. Asserted here:
   (a) LEGACY by-name layout (synthetic pre-P2b docs): rename docA onto
       docB's display name -> hard error BEFORE any fs mutation; docB
       content intact, docA keeps its own name/content (A1-A4).
   (b) by-uuid fresh MCP doc rename onto an existing other doc's display
       name -> SAFE outcome contract: success OR NAME_COLLISION error, but
       in either case BOTH docs' contents read back intact and the renamed
       doc's content resolves correctly (A5-A9).
B  [R16 reindex cap + decode] positive: CJK doc via rag_file_create ->
   rag_search finds it (shared decode path alive). reindex_all trigger is
   model-swap (Tauri IPC) -> reasoned SKIP.
C  [R16 OCR status] get_ocr_status is a Tauri command (lib.rs:587,
   commands/rag.rs) with NO HTTP face -> reasoned SKIP.
D  [contract script] python3 scripts/check_frontend_rust_contract.py ->
   exit 0 + "OK" in output (CI-able assertion captured in-suite).
E  [convergence sweep — the core ask] for EACH of 5 versions: initialize
   echo MUST equal request version on /mcp root, then one REAL public-IP
   tools/call (legacy = session lane; 2026 = stateless _meta lane) with
   isError:false + IPv4 in body, resilient retry for ip.3322.net rate
   limits.
F  [leniency regression, all 4 legacy versions] bare request (no Accept,
   no session, version header only) -> 2xx valid JSON-RPC. strict-off is
   part of the CAMPAIGN BASELINE enforced at start.
G  [OAuth lifecycle] full headless flow on the desktop provider: dynamic
   register -> consent page nonce -> approve 302+code -> code exchange ->
   refresh_token grant (pattern cloned from r1100 section_d).

Cleanup: config forced to CAMPAIGN BASELINE (strictValidation=false at top
level AND inside mcp; exposeHttp=true), pins=0, synthetic docs/files/rows
removed, /health 200.  Exit 0 only if all real cases pass.
"""
import json
import os
import re
import sqlite3
import subprocess
import sys
import time
import uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import rmcp_round_r1100 as base
from rmcp_round_r1100 import (
    HOST, DB, LEGACY_VERSIONS, IP_SERVER, IP_TOOL, META_2026,
    req, sse_last, cfg_read, cfg_write, initialize, notify_initialized, rpc,
    result_text, json_txt, valid_pub_ip,
)

check = base.check
skip = base.skip
warn = base.warn

FILES_DIR = os.path.expanduser(
    "~/Library/Application Support/app.mcphub.desktop/rag/files")
SYNTH_DOCS = []  # (doc_id, content_path, meta_path)

CAMPAIGN_STRICT_PATHS = [("top", "strictValidation"), ("mcp", "mcp.strictValidation")]


def force_campaign_baseline():
    """Restore to the CAMPAIGN BASELINE (not as-found): strict=false both
    levels, exposeHttp=true."""
    cfg = cfg_read()
    cfg["strictValidation"] = False
    cfg.setdefault("mcp", {})["strictValidation"] = False
    cfg["exposeHttp"] = True
    cfg_write(cfg)


def baseline_report():
    cfg = cfg_read()
    top = cfg.get("strictValidation")
    inner = (cfg.get("mcp") or {}).get("strictValidation")
    expose = cfg.get("exposeHttp")
    check("RESTORE baseline strictValidation=false (top level)", top is False,
          f"top={top}")
    check("RESTORE baseline mcp.strictValidation=false (inside mcp)",
          inner is False, f"mcp.strictValidation={inner}")
    check("RESTORE baseline exposeHttp=true", expose is True, f"exposeHttp={expose}")


# ── MCP builtin RAG helpers (pattern from r1600) ────────────────────────────
def mcp_session(path="/mcp", version="2025-06-18"):
    st, echoed, sid = initialize(path, version, client="r1700")
    if st == 200:
        notify_initialized(path, sid)
    return sid if st == 200 else None


def tools_list(sid, rid=5, path="/mcp"):
    st, data, _ = rpc(path, rid, "tools/list", {}, session=sid, timeout=90)
    o = sse_last(data, rid)
    return [t.get("name", "") for t in
            (((o or {}).get("result", {}) or {}).get("tools", []) or [])]


def call_tool(sid, rid, name, args, path="/mcp", timeout=120):
    st, data, _ = rpc(path, rid, "tools/call",
                      {"name": name, "arguments": args}, session=sid,
                      timeout=timeout)
    return st, sse_last(data, rid)


def find_rag_tool(sid, want):
    names = tools_list(sid)
    for n in names:
        if n == want or n.endswith(f"-{want}"):
            return n
    return None


def rag_file_create(sid, rid, doc_name, doc_type, content):
    tool = find_rag_tool(sid, "rag_file_create")
    if tool is None:
        return None, None
    return call_tool(sid, rid, tool, {
        "docName": doc_name, "docType": doc_type, "docContent": content})


def rag_file_update(sid, rid, doc_id, **fields):
    tool = find_rag_tool(sid, "rag_file_update")
    if tool is None:
        return None, None
    return call_tool(sid, rid, tool, {"docId": doc_id, **fields})


def rag_file_delete(sid, rid, doc_id, timeout=90):
    tool = find_rag_tool(sid, "rag_file_delete")
    if tool is None:
        return None, None
    return call_tool(sid, rid, tool, {"docId": doc_id}, timeout=timeout)


def rag_search(sid, rid, query):
    tool = find_rag_tool(sid, "rag_search")
    if tool is None:
        return None, None
    return call_tool(sid, rid, tool, {"query": query})


def rag_get(sid, rid, doc_id):
    tool = find_rag_tool(sid, "rag_get")
    if tool is None:
        return None, None
    return call_tool(sid, rid, tool, {"docId": doc_id})


def db_doc_row(doc_id):
    con = sqlite3.connect(DB)
    row = con.execute(
        "SELECT name, chunk_count FROM rag_docs WHERE id=?", (doc_id,)).fetchone()
    con.close()
    return row


def wait_row_gone(doc_id, secs=25.0):
    deadline = time.time() + secs
    while time.time() < deadline:
        if db_doc_row(doc_id) is None:
            return True
        time.sleep(0.7)
    return db_doc_row(doc_id) is None


def make_synthetic_byname_doc(name, chunk_count=1):
    """DB row + content file ({name} — pre-P2b rag_file_create naming, the
    legacy layout the collision guard protects) + meta."""
    doc_id = str(uuid.uuid4())
    content = f"r1700 legacy by-name synthetic {name}\n" + "x" * 64
    content_path = os.path.join(FILES_DIR, name)
    meta_path = os.path.join(FILES_DIR, f"{doc_id}.meta")
    with open(content_path, "w", encoding="utf-8") as f:
        f.write(content)
    meta = {
        "id": doc_id, "name": name, "title": None, "tags": [],
        "size": len(content.encode()), "uploaded_at": "2026-10-10 00:00",
        "chunk_count": chunk_count, "version": 1, "file_type": "Text",
        "method": "copy", "original_path": None, "md5": None, "source": None,
    }
    with open(meta_path, "w", encoding="utf-8") as f:
        json.dump(meta, f, ensure_ascii=False)
    con = sqlite3.connect(DB)
    con.execute(
        "INSERT INTO rag_docs (id, name, size, uploaded_at, file_type, method,"
        " original_path, md5, version, chunk_count) VALUES (?,?,?,?,?,?,?,?,?,?)",
        (doc_id, name, len(content.encode()), "2026-10-10 00:00", "Text",
         "copy", None, None, 1, chunk_count))
    con.commit()
    con.close()
    SYNTH_DOCS.append((doc_id, content_path, meta_path))
    return doc_id


def sweep_r1700():
    con = sqlite3.connect(DB)
    con.execute("DELETE FROM rag_docs WHERE name LIKE 'r1700%'")
    con.commit()
    con.close()
    try:
        for fn in os.listdir(FILES_DIR):
            if fn.startswith("r1700"):
                try:
                    os.remove(os.path.join(FILES_DIR, fn))
                except OSError:
                    pass
    except OSError:
        pass


# ── A: R16 NAME_COLLISION guard (legacy layout) + by-uuid harmless rename ───
def section_a():
    print("== A: R16 rag_file_update NAME_COLLISION (guard + P2b safe rename) ==")
    marker_a = "R1700ALPHA alpha body 协议内容 alpha"
    marker_b = "R1700BETA beta body 协议内容 beta"
    sid = mcp_session()
    if sid is None:
        skip("A (no /mcp session or RAG tools unavailable)")
        return

    # ── A.part1: guard reachable via LEGACY by-name layout (synthetic) ──
    # The collision guard requires content on disk at BOTH {old_name} and
    # {new_name} and NO {doc_id} file — exactly the pre-P2b rag_file_create
    # naming, which content_path_for still resolves (candidate 4).
    id_a = make_synthetic_byname_doc("r1700_alpha.txt")
    id_b = make_synthetic_byname_doc("r1700_beta.txt")
    with open(os.path.join(FILES_DIR, "r1700_alpha.txt"), "w", encoding="utf-8") as f:
        f.write(marker_a)
    with open(os.path.join(FILES_DIR, "r1700_beta.txt"), "w", encoding="utf-8") as f:
        f.write(marker_b)
    st, o = rag_file_update(sid, 12, id_a, docName="r1700_beta.txt")
    err = (o or {}).get("error")
    is_err = ((o.get("result") or {}).get("isError") if o else None)
    rejected = (isinstance(err, dict) and "NAME_COLLISION" in (err.get("message") or "")) \
        or is_err is True
    check("A1 legacy by-name layout: rename-collision REJECTED (hard error)",
          st == 200 and rejected, f"st={st} err={err} isError={is_err}")
    warn(f"A1 detail: st={st} jsonrpc_error={err} isError={is_err}")
    time.sleep(0.5)
    st2, o2 = rag_get(sid, 13, id_b)
    got_b = result_text(o2)
    check("A2 docB content intact (no silent overwrite)",
          marker_b in got_b and marker_a not in got_b, got_b[:120])
    st3, o3 = rag_get(sid, 14, id_a)
    got_a = result_text(o3)
    check("A3 docA keeps its own name/content (rejection wholesale)",
          marker_a in got_a and marker_b not in got_a, got_a[:120])
    row_a, row_b = db_doc_row(id_a), db_doc_row(id_b)
    check("A4 both legacy docs keep DB names (no meta rename persisted)",
          row_a is not None and row_a[0] == "r1700_alpha.txt"
          and row_b is not None and row_b[0] == "r1700_beta.txt",
          f"a={row_a} b={row_b}")

    # ── A.part2: fresh MCP docs are BY-UUID (P2b) — rename must be SAFE ──
    # Round-17 fix: the head guard only fires when content_path_for(old)
    # RESOLVES TO the bare name path (candidate 4); by-uuid docs resolve via
    # candidate 1 ({id}{ext}) so they bypass. The late rename block now also
    # hard-Errs on a foreign new_path collision. Either outcome is accepted
    # here as long as BOTH docs' contents still resolve correctly (read back
    # and compare) — a silent clobber is the only failure.
    st, o = rag_file_create(sid, 15, "r1700_uuid_a", "txt", "R1700UUID alpha fresh")
    uid = None
    try:
        uid = json.loads(result_text(o)).get("docId") if o else None
    except Exception:
        pass
    check("A5 rag_file_create fresh doc ok", st == 200 and uid, f"st={st} {json_txt(o)[:120]}")
    if uid:
        st, o = rag_file_update(sid, 16, uid, docName="r1700_beta.txt")
        is_err = ((o.get("result") or {}).get("isError") if o else None)
        err = (o or {}).get("error")
        succeeded = st == 200 and o is not None and is_err is False
        collision_err = (isinstance(err, dict)
                         and "NAME_COLLISION" in (err.get("message") or "")) \
            or (isinstance(err, dict) is False and is_err is True
                and "NAME_COLLISION" in json_txt(o))
        check("A6 by-uuid (fresh MCP) doc rename onto existing name -> "
              "SAFE outcome (success OR NAME_COLLISION error, never silent "
              "clobber)",
              st == 200 and (succeeded or collision_err),
              f"st={st} err={err} isError={is_err}")
        warn(f"A6 detail: succeeded={succeeded} collision_err={collision_err} "
             f"isError={is_err} err={err}")
        # Decisive: read BOTH docs back and compare content.
        st, o = rag_get(sid, 17, id_b)
        got_b = result_text(o)
        check("A7 legacy docB content STILL INTACT (no clobber, either "
              "outcome)", marker_b in got_b and "R1700UUID" not in got_b,
              got_b[:120])
        st, o = rag_get(sid, 18, uid)
        got_u = result_text(o)
        if succeeded:
            # Renamed doc's content must still resolve under the NEW name.
            check("A8 renamed by-uuid doc's content resolves correctly "
                  "(fresh alpha body intact under new name)",
                  "R1700UUID" in got_u and marker_b not in got_u, got_u[:120])
        else:
            check("A8 rename rejected wholesale: by-uuid doc keeps old "
                  "name+content",
                  "R1700UUID" in got_u, got_u[:120])
        row_u = db_doc_row(uid)
        if succeeded:
            check("A9 by-uuid doc renamed in DB (name=beta, row alive)",
                  row_u is not None and row_u[0] == "r1700_beta.txt", f"row={row_u}")
        else:
            check("A9 by-uuid doc keeps old DB name (rejection wholesale)",
                  row_u is not None and row_u[0] == "r1700_uuid_a.txt",
                  f"row={row_u}")
        rag_file_delete(sid, 93, uid)
        wait_row_gone(uid, secs=10.0)

    rag_file_delete(sid, 91, id_a)
    rag_file_delete(sid, 92, id_b)
    gone = wait_row_gone(id_a) and wait_row_gone(id_b)
    if gone:
        check("A9 cleanup: docs removed", True)
    else:
        warn("A9 SQL mirror rows linger (deferred prune) — finally-sweep "
             "will force-clean")
        skip("A9 cleanup row-prune confirmation (deferred prune / db-lock)")


# ── B: R16 reindex cap + decode (positive half) ─────────────────────────────
def section_b():
    print("== B: R16 reindex decode (positive: CJK doc indexed + searchable) ==")
    sid = mcp_session()
    if sid is None:
        skip("B (no /mcp session or RAG tools unavailable)")
        return
    marker = "R1700CJK 中文字符嵌入检索标记 embed 测试"
    st, o = rag_file_create(sid, 20, "r1700_cjk", "txt", marker)
    doc_id = None
    try:
        doc_id = json.loads(result_text(o)).get("docId") if o else None
    except Exception:
        pass
    check("B1 rag_file_create CJK doc ok", st == 200 and doc_id, f"st={st} {json_txt(o)[:120]}")
    if not doc_id:
        return
    row = db_doc_row(doc_id)
    check("B2 chunk_count > 0 (index alive)", row is not None and row[1] > 0, f"row={row}")
    st, o = rag_search(sid, 21, "R1700CJK 中文字符")
    txt = result_text(o)
    check("B3 rag_search finds the CJK doc (shared decode path)",
          st == 200 and o is not None and ("R1700" in txt or "中文" in txt),
          f"st={st} {txt[:140]}")
    rag_file_delete(sid, 22, doc_id)
    if wait_row_gone(doc_id):
        check("B4 cleanup: CJK doc removed", True)
    else:
        warn("B4 SQL mirror row lingers — finally-sweep will force-clean")
        skip("B4 cleanup row-prune confirmation (deferred prune / db-lock)")


# ── D: contract script (CI-able) ────────────────────────────────────────────
def section_d():
    print("== D: check_frontend_rust_contract.py ==")
    p = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                     "check_frontend_rust_contract.py")
    repo = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    r = subprocess.run([sys.executable, p], capture_output=True, text=True,
                       timeout=120, cwd=repo)
    out = (r.stdout or "") + (r.stderr or "")
    check("D1 contract script exit 0", r.returncode == 0,
          f"rc={r.returncode} out={out[-200:]}")
    check("D2 contract output contains OK", "OK" in out, f"out={out[-200:]}")
    warn(f"D output tail: {out.strip()[-120:]}")


# ── E: version×channel convergence sweep (core ask) ─────────────────────────
def resilient_ip_call(session_lane=True, version=None):
    """One real public-IP tools/call with retry for ip.3322.net rate limits.
    Returns (st, o, tries) on first success that carries a valid IPv4; else
    the LAST failure for verdict."""
    last = (None, None, 0)
    for attempt in range(1, 5):
        if session_lane:
            st, echoed, sid = initialize("/mcp", version, client="r1700e")
            if st != 200:
                last = (st, None, attempt)
                time.sleep(3)
                continue
            notify_initialized("/mcp", sid)
            names = tools_list(sid, rid=5)
            ip_name = next(
                (n for n in names if n == IP_TOOL or n.endswith(f"-{IP_TOOL}")), None)
            if not ip_name:
                last = (None, "no-ip-tool", attempt)
                time.sleep(3)
                continue
            st, data, _ = rpc("/mcp", 7, "tools/call",
                              {"name": ip_name, "arguments": {}},
                              session=sid, timeout=90)
            o = sse_last(data, 7)
        else:
            st, data, _ = req("POST", "/mcp", body={
                "jsonrpc": "2.0", "id": 8, "method": "tools/call",
                "params": {"name": f"{IP_SERVER}-{IP_TOOL}", "arguments": {},
                           **META_2026}},
                headers={"MCP-Protocol-Version": "2026-07-28"}, timeout=90)
            o = sse_last(data, 8)
        if (st == 200 and o is not None
                and ((o.get("result") or {}).get("isError") is False)
                and valid_pub_ip(result_text(o))):
            return st, o, attempt
        last = (st, o, attempt)
        time.sleep(3)
    return last


def section_e():
    print("== E: 5 versions × initialize echo + REAL public-IP call (convergence) ==")
    for v in LEGACY_VERSIONS:
        st, echoed, _ = initialize("/mcp", v, client="r1700f")
        check(f"E1 {v} initialize echo EXACT on /mcp root",
              st == 200 and echoed == v, f"st={st} echo={echoed}")
    for v in LEGACY_VERSIONS:
        st, o, tries = resilient_ip_call(session_lane=True, version=v)
        ok = (st == 200 and o is not None
              and ((o.get("result") or {}).get("isError") is False)
              and valid_pub_ip(result_text(o)))
        check(f"E2 {v} REAL public-IP call (session lane, isError:false + IPv4)",
              ok, f"st={st} tries={tries} {json_txt(o)[:140]}")
        warn(f"E2 {v}: tries={tries}")
    st, o, tries = resilient_ip_call(session_lane=False)
    r = (o or {}).get("result", {}) or {}
    check("E3 2026-07-28 stateless REAL public-IP call (resultType:complete + IPv4)",
          st == 200 and r.get("resultType") == "complete"
          and r.get("isError") is False and valid_pub_ip(result_text(o)),
          f"st={st} tries={tries} {json_txt(o)[:140]}")


# ── F: leniency regression on all legacy versions ───────────────────────────
def section_f():
    print("== F: leniency bare-request regression (4 legacy versions) ==")
    # Campaign baseline enforced: strict must already be off (force at start).
    for v in LEGACY_VERSIONS:
        st, data, _ = req("POST", "/mcp", body={
            "jsonrpc": "2.0", "id": f"f-{v}", "method": "tools/call",
            "params": {"name": f"{IP_SERVER}-{IP_TOOL}", "arguments": {}}},
            default_accept=False, headers={"MCP-Protocol-Version": v}, timeout=90)
        o = sse_last(data, f"f-{v}")
        check(f"F1 {v} bare tools/call (no Accept, no session) -> 2xx valid JSON-RPC",
              200 <= st < 300 and o is not None and o.get("jsonrpc") == "2.0"
              and o.get("id") == f"f-{v}"
              and (("result" in o) or isinstance(o.get("error"), dict)),
              f"st={st} {data[:120]}")


# ── G: OAuth lifecycle (headless, cloned from r1100 section_d) ──────────────
def retry_http(fn, tries=5, delay=1.5):
    """Config writes can briefly rebind the hub listener; retry conn errors
    so a rebind window isn't read as a verdict."""
    last_exc = None
    for _ in range(tries):
        try:
            return fn()
        except Exception as e:
            last_exc = e
            time.sleep(delay)
    raise last_exc


def section_g():
    print("== G: OAuth provider lifecycle (register -> consent -> code -> refresh) ==")
    import secrets
    import hashlib
    import base64
    from urllib.parse import quote

    orig = cfg_read().get("oauthServer")
    con = sqlite3.connect(DB)
    try:
        con.execute("DELETE FROM oauth_clients WHERE name LIKE 'r1700-e2e%'")
    except Exception:
        pass
    con.commit()
    con.close()
    redirect = "http://127.0.0.1/callback"
    try:
        cfg = cfg_read()
        cfg["oauthServer"] = {"enabled": True, "requireClientSecret": False,
                              "allowedScopes": [], "requireState": True,
                              "dynamicRegistration": {"enabled": True}}
        cfg_write(cfg)
        time.sleep(1.0)
        st, data, _ = retry_http(lambda: req("POST", "/oauth/register", body={
            "client_name": "r1700-e2e", "redirect_uris": [redirect]},
            default_accept=False))
        try:
            reg = json.loads(data)
        except Exception:
            reg = {}
        client_id = reg.get("client_id")
        check("G1 dynamic registration -> 201 with client_id",
              st == 201 and bool(client_id), f"st={st} {data[:140]}")
        if not client_id:
            skip("G2-G5 positive flow (no client_id)")
            client_id = "bogus-client"
        verifier = secrets.token_urlsafe(48)
        challenge = base64.urlsafe_b64encode(
            hashlib.sha256(verifier.encode()).digest()).rstrip(b"=").decode()
        q = (f"?response_type=code&client_id={quote(client_id, safe='')}"
             f"&redirect_uri={quote(redirect, safe='')}&scope=mcp:all&state=r1700"
             f"&code_challenge={challenge}&code_challenge_method=S256")
        st, data, _ = retry_http(
            lambda: req("GET", f"/oauth/authorize{q}", default_accept=False))
        m = re.search(r'name="nonce" value="([^"]+)"', data)
        nonce = m.group(1) if m else None
        check("G2 authorize endpoint -> 200 consent page with nonce",
              st == 200 and nonce is not None and "Approve" in data,
              f"st={st} nonce={'yes' if nonce else 'no'}")
        code = None
        if st == 200 and nonce:
            form = ("nonce={}&client_id={}&redirect_uri={}&scope=mcp:all&state=r1700"
                    "&code_challenge={}&resource=&action=approve").format(
                quote(nonce, safe=""), quote(client_id, safe=""),
                quote(redirect, safe=""), quote(challenge, safe=""))
            st2, data2, h2 = req("POST", "/oauth/authorize", body=form, form=True,
                                 default_accept=False)
            loc = h2.get("location", "")
            mc = re.search(r"[?&]code=([^&]+)", loc)
            code = mc.group(1) if mc else None
            check("G3 approve POST -> 302 with code + state",
                  st2 == 302 and code is not None and "state=r1700" in loc,
                  f"st2={st2} loc={loc[:120]}")
        else:
            skip("G3 approve POST (no consent page)")
        if code:
            body = ("grant_type=authorization_code&code={}&redirect_uri={}"
                    "&client_id={}&code_verifier={}").format(
                quote(code, safe=""), quote(redirect, safe=""),
                quote(client_id, safe=""), quote(verifier, safe=""))
            st, data, _ = retry_http(lambda: req("POST", "/oauth/token", body=body,
                                                 form=True, default_accept=False))
            try:
                tok = json.loads(data)
            except Exception:
                tok = {}
            check("G4 code exchange -> 200 access_token + refresh_token",
                  st == 200 and tok.get("access_token") and tok.get("refresh_token"),
                  f"st={st} {data[:140]}")
            rt = tok.get("refresh_token")
            if rt:
                body = ("grant_type=refresh_token&refresh_token={}&client_id={}").format(
                    quote(rt, safe=""), quote(client_id, safe=""))
                st, data, _ = retry_http(lambda: req("POST", "/oauth/token",
                                                     body=body, form=True,
                                                     default_accept=False))
                try:
                    tok2 = json.loads(data)
                except Exception:
                    tok2 = {}
                check("G5 refresh_token grant -> 200 new access_token",
                      st == 200 and bool(tok2.get("access_token")),
                      f"st={st} {data[:140]}")
            else:
                skip("G5 refresh_token grant (no refresh token issued)")
        else:
            skip("G4 code exchange (no code)")
            skip("G5 refresh_token grant (no code)")
        st, data, _ = req("GET", "/.well-known/oauth-protected-resource",
                          default_accept=False)
        check("G6 protected-resource metadata 200", st == 200, f"st={st}")
    finally:
        try:
            con = sqlite3.connect(DB)
            cur = con.execute("SELECT client_id FROM oauth_clients WHERE name LIKE 'r1700-e2e%'")
            ids = [r[0] for r in cur.fetchall()]
            for cid in ids:
                for tbl in ("oauth_codes", "oauth_tokens"):
                    try:
                        con.execute(f"DELETE FROM {tbl} WHERE client_id=?", (cid,))
                    except Exception:
                        pass
                con.execute("DELETE FROM oauth_clients WHERE client_id=?", (cid,))
            con.commit()
            con.close()
        except Exception:
            pass
        cfg0 = cfg_read()
        if orig is None:
            cfg0.pop("oauthServer", None)
        else:
            cfg0["oauthServer"] = orig
        cfg_write(cfg0)


SKIP_STUBS = [
    ("R16 NAME_COLLISION guard on FRESH MCP-created docs (by-uuid)",
     "P2b stores rag_file_create content by uuid ({id}.{docType}); the"
     " round-17 guard only fires when content_path_for(old) RESOLVES TO the"
     " bare name path (candidate 4) — by-uuid docs resolve via candidate 1"
     " and bypass, so no deterministic error outcome exists for them (the"
     " late rename block's new hard-Err may or may not fire depending on the"
     " candidate chain). A6-A9 instead assert the SAFE outcome contract:"
     " success OR NAME_COLLISION error, but in BOTH cases both docs' contents"
     " read back intact (silent clobber is the failure). The guard itself is"
     " exercised on the legacy by-name layout (A1-A4)."),
    ("R16 reindex cap (negative half)",
     "reindex_all triggers ONLY on a model swap (Tauri IPC rag model select /"
     " mv consumer registration); not HTTP-exposed — cannot force the cap/"
     "decode failure path over the hub surface. Positive half (shared decode"
     " path alive via rag_search) covered in B."),
    ("R16 OCR status",
     "get_ocr_status is registered as a Tauri IPC command (src-tauri/src/"
     "lib.rs:587, commands/rag.rs) with no HTTP face on the hub surface —"
     " unreachable via HTTP E2E; covered by Rust-side tests."),
]


def main():
    try:
        st, _, _ = req("GET", "/health", default_accept=False)
        if st != 200:
            print(f"SKIP-ALL: hub not healthy at {HOST} (status {st})")
            return 2
    except Exception as e:
        print(f"SKIP-ALL: hub down: {e}")
        return 2

    con = sqlite3.connect(DB)
    pins_before = con.execute(
        "SELECT count(*) FROM server_tool_config WHERE pinned=1").fetchone()[0]
    con.close()

    for name, why in SKIP_STUBS:
        skip(name, why)

    force_campaign_baseline()
    rc = 0
    try:
        section_e()   # convergence sweep first (read-only, core ask)
        section_f()   # leniency regression (read-only)
        section_d()   # contract script (read-only)
        for fn in (section_a, section_b, section_g):
            try:
                fn()
            except Exception as e:
                check(f"{fn.__name__} completed without crash", False, repr(e))
    finally:
        # Campaign baseline (NOT as-found restore): strict=false both levels,
        # exposeHttp=true, pins=0, synthetic docs swept.
        force_campaign_baseline()
        con = sqlite3.connect(DB)
        con.execute("UPDATE server_tool_config SET pinned=0")
        con.commit()
        con.close()
        for doc_id, content_path, meta_path in SYNTH_DOCS:
            try:
                os.remove(content_path)
            except OSError:
                pass
            try:
                os.remove(meta_path)
            except OSError:
                pass
        sweep_r1700()
        time.sleep(0.5)

    baseline_report()
    con = sqlite3.connect(DB)
    pins_after = con.execute(
        "SELECT count(*) FROM server_tool_config WHERE pinned=1").fetchone()[0]
    n_left = con.execute(
        "SELECT count(*) FROM rag_docs WHERE name LIKE 'r1700%'").fetchone()[0]
    con.close()
    check("RESTORE pins=0 (campaign baseline)", pins_after == 0,
          f"before={pins_before} after={pins_after}")
    check("RESTORE no r1700_* rag docs left", n_left == 0, f"n={n_left}")
    st, _, _ = req("GET", "/health", default_accept=False)
    check("RESTORE /health 200", st == 200, f"st={st}")

    print(f"\n{'=' * 50}\nPASS={base.PASS} FAIL={base.FAIL} SKIP={base.SKIPS}")
    if base.FAILURES:
        print("FAILED:", *base.FAILURES, sep="\n  - ")
        rc = 1
    return rc


if __name__ == "__main__":
    sys.exit(main())
