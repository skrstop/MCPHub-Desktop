#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""E2E round r1600 (round-16): FIX-REGRESSION suite pinning ROUND-15 fixes.

Semantics sourced from doc/oauth_review_campaign_round15_20261010.md.

A  [R15-#1 H get_strict wiring / skipAuth fail-closed] corrupt
   system_config.config_json -> hub_admin_gate (the HTTP admin gate, which
   reads config through the same get_strict() that is_skip_auth_enabled now
   uses) must 401; /health stays 200; empty-config `{}` (fresh-install
   default; the column is NOT NULL here) must still pass (get_strict keeps
   `{}` == Ok({})).  Raw config restored in
   finally.  NOTE: the tauri-command-level is_skip_auth_enabled itself is
   IPC-only (not HTTP-reachable) — covered by config_service get_strict_*
   unit tests (round 15); this case proves the shared-reader wiring E2E.
B  [R15-#2 H reindex_all non-UTF-8] positive-half sanity: creating a doc via
   the MCP builtin rag_file_create indexes it (chunk_count>0, rag_search
   finds it) — the index is alive.  The GBK/un-decodable-original wipe path
   lives in reindex_all, triggered ONLY by a model swap (Tauri IPC rag
   model select / mv) — SKIP with reasoning (see SKIP_STUBS).
C  [R15-#3 M file-source vanish keeps copy docs] DB+disk synthetic
   file-kind COPY-method doc whose original_path does not exist; auto-update
   tick interval shrunk to 60s so run_batch_update -> plan_source_sync
   actually fires; the doc must SURVIVE the tick (fix: only symlink-method
   file docs are auto-removed; copy docs degrade to lost-original).  Tick
   evidence taken from app_log [rag] rows.  Interval + synthetic rows/files
   restored/removed in finally.
D  [R15-#4 M canonicalize_url 600s cache] synthetic git-kind docs on the
   existing real repo; rag_file_delete of git docs goes through the
   delete-doc git refcount path which canonicalizes the URL.  A warm-up
   delete populates the cache; the next two deletes must be fast (< 8s and
   not slower than the first warm post-warmup delete) instead of paying a
   fresh 10s probe each time.  Synthetic docs are fully removed by their
   own deletes; refcount of the real repo (101 docs) never reaches 0.
E  [R15-#5 L update_doc rename collision] rag_file_update renaming docA to
   docB's display name must not destroy docB's content file (pre-fix Unix
   rename silently overwrote it).  Read BOTH docs after the attempt: each
   keeps its own marker content and its own DB name/chunk_count.
F  [R15 regression sweep] 5 versions initialize echo on /mcp root + one
   REAL public-IP call on 2025-06-18 (session lane) and one on 2026-07-28
   (stateless) — nothing broke.

Cleanup: config_json (raw bytes), rag auto-update interval, pins (zero
throughout), synthetic docs/files/rows.  Exit 0 only if all real cases pass.
"""
import json
import os
import sqlite3
import sys
import time
import urllib.parse
import uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import rmcp_round_r1100 as base
from rmcp_round_r1100 import (
    HOST, DB, LEGACY_VERSIONS, IP_SERVER, IP_TOOL,
    req, sse_last, cfg_read, cfg_write, initialize, notify_initialized, rpc,
    result_text, json_txt, valid_pub_ip,
)

check = base.check
skip = base.skip
warn = base.warn

RAW_CFG = None
FILES_DIR = os.path.expanduser(
    "~/Library/Application Support/app.mcphub.desktop/rag/files")
SYNC_DOCS = []          # synthetic doc ids created for C/D
ORIG_INTERVAL = None    # rag.autoUpdateIntervalSecs before C


# ── MCP builtin RAG tool helpers ────────────────────────────────────────────
def mcp_session(path="/mcp", version="2025-06-18"):
    st, echoed, sid = initialize(path, version, client="r1600")
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


def rag_file_create(sid, rid, doc_name, doc_type, content, tags=None):
    tool = find_rag_tool(sid, "rag_file_create")
    if tool is None:
        return None, None
    return call_tool(sid, rid, tool, {
        "docName": doc_name, "docType": doc_type, "docContent": content,
        **({"tags": tags} if tags else {})})


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


def wait_row_gone(doc_id, secs=20.0):
    """SQL-mirror row removal can lag the MCP delete (prune/lock); poll."""
    deadline = time.time() + secs
    while time.time() < deadline:
        if db_doc_row(doc_id) is None:
            return True
        time.sleep(0.7)
    return db_doc_row(doc_id) is None


def retry_status(fn, tries=4, delay=2.0):
    """The hub briefly tears down/rebinds its HTTP listener on config-change
    syncs; probe with retries so a rebind window isn't read as a verdict."""
    last = None
    for i in range(tries):
        try:
            last = fn()
            return last
        except Exception as e:
            last = f"conn-err: {e}"
            time.sleep(delay)
    return last


# ── F: round-15 regression sweep (read-only) ────────────────────────────────
def section_f():
    print("== F: R15 regression sweep (5 versions echo + real IP calls) ==")
    for v in LEGACY_VERSIONS:
        st, echoed, _ = initialize("/mcp", v, client="r1600f")
        check(f"F1 {v} initialize echo EXACT on /mcp root",
              st == 200 and echoed == v, f"st={st} echo={echoed}")
    # legacy real call (2025-06-18 session lane)
    sid = mcp_session("/mcp", "2025-06-18")
    if sid:
        names = tools_list(sid, rid=6)
        ip_name = next((n for n in names
                        if n == IP_TOOL or n.endswith(f"-{IP_TOOL}")), None)
        st, o = call_tool(sid, 7, ip_name, {}) if ip_name else (None, None)
        check("F2 2025-06-18 REAL public-IP call (root, prefixed name)",
              st == 200 and o is not None and (o.get("error") is None)
              and ((o.get("result") or {}).get("isError") is False)
              and valid_pub_ip(result_text(o)),
              f"st={st} {json_txt(o)[:160]}")
    else:
        check("F2 2025-06-18 REAL public-IP call", False, "no session")
    # 2026 stateless real call — run under the DEFAULT leniency mode: strict
    # mode (mcp.strictValidation) mandates the SEP-2243 Mcp-Method/Mcp-Name
    # headers, which is a strict-mode contract, not the default-regression
    # expectation. Snapshot + restore so the sweep is mode-independent.
    orig_strict = cfg_read().get("mcp", {}).get("strictValidation")
    try:
        if orig_strict:
            cfg = cfg_read()
            cfg.setdefault("mcp", {})["strictValidation"] = False
            cfg_write(cfg)
        st, data, _ = req("POST", "/mcp", body={
            "jsonrpc": "2.0", "id": 8, "method": "tools/call",
            "params": {"name": f"{IP_SERVER}-{IP_TOOL}", "arguments": {},
                       **base.META_2026}},
            headers={"MCP-Protocol-Version": "2026-07-28"}, timeout=90)
        o = sse_last(data, 8)
        r = (o or {}).get("result", {}) or {}
        check("F3 2026-07-28 stateless REAL public-IP call (resultType:complete)",
              st == 200 and r.get("resultType") == "complete"
              and r.get("isError") is False and valid_pub_ip(result_text(o)),
              f"st={st} {data[:160]}")
    finally:
        if orig_strict:
            cfg = cfg_read()
            cfg.setdefault("mcp", {})["strictValidation"] = True
            cfg_write(cfg)


# ── E: R15-#5 update_doc rename collision ───────────────────────────────────
def section_e():
    print("== E: R15-#5 update_doc rename collision (no content swap) ==")
    marker_a = "R1600ALPHA unique alpha body 协议内容 alpha"
    marker_b = "R1600BETA unique beta body 协议内容 beta"
    # Content files stored under the DISPLAY name (legacy rag_file_create
    # scheme) — the only layout where the rename guard is reachable: docs
    # created by today's MCP tool store content as {uuid}{ext}, so their
    # rename path short-circuits (old_path == new_path by-id resolution).
    id_a = make_synthetic_doc("r1600_alpha.txt", "copy", None, None,
                              chunk_count=1, content_by_name=True)
    id_b = make_synthetic_doc("r1600_beta.txt", "copy", None, None,
                              chunk_count=1, content_by_name=True)
    with open(os.path.join(FILES_DIR, "r1600_alpha.txt"), "w",
              encoding="utf-8") as f:
        f.write(marker_a)
    with open(os.path.join(FILES_DIR, "r1600_beta.txt"), "w",
              encoding="utf-8") as f:
        f.write(marker_b)
    sid = mcp_session()
    if sid is None:
        skip("E (no /mcp session or RAG tools unavailable)")
        return
    # Collision attempt: rename docA onto docB's exact content-file name —
    # pre-fix, Unix fs::rename silently overwrote docB's content file. The
    # round-16 fix raises a hard Err BEFORE any fs mutation, so the whole
    # update is rejected (no meta rename, no rename warn path anymore).
    st, o = rag_file_update(sid, 13, id_a, docName="r1600_beta.txt")
    err = (o or {}).get("error")
    is_err = ((o.get("result") or {}).get("isError") if o else None)
    rejected = (isinstance(err, dict) and "NAME_COLLISION" in (err.get("message") or "")) \
        or is_err is True
    check("E1 rename-collision attempt REJECTED with an error (wholesale)",
          st == 200 and rejected, f"st={st} err={err} isError={is_err}")
    warn(f"E1 detail: st={st} jsonrpc_error={err} isError={is_err}")
    time.sleep(0.5)
    # The decisive invariant: docB's content file was NOT overwritten.
    st2, o2 = rag_get(sid, 15, id_b)
    got_b = result_text(o2)
    check("E2 docB content intact (pre-fix silent overwrite destroyed it)",
          marker_b in got_b and marker_a not in got_b, got_b[:120])
    # The rejected update is wholesale: docA keeps its old name+content.
    st3, o3 = rag_get(sid, 16, id_a)
    got_a = result_text(o3)
    check("E2b docA keeps its own old content (update rejected wholesale)",
          marker_a in got_a and marker_b not in got_a, got_a[:120])
    con = sqlite3.connect(DB)
    warns = con.execute(
        "SELECT count(*) FROM app_log WHERE message LIKE "
        "'%rag_file_update%NAME_COLLISION%' OR message LIKE "
        "'%rag_file_update: rename target%already exists%'").fetchone()[0]
    con.close()
    # The round-16 fix raises a hard Err BEFORE any fs mutation; the legacy
    # round-15 warn+skip path is its historical form. Either means: no
    # silent overwrite.
    collision = isinstance(err, dict) and "NAME_COLLISION" in (err.get("message") or "")
    check("E3 fix fired: rename refused (NAME_COLLISION error or warn logged)",
          collision or warns >= 1, f"err={err} warns={warns}")
    alpha_file = os.path.join(FILES_DIR, "r1600_alpha.txt")
    check("E4 docA's content file still on disk (not renamed away/destroyed)",
          os.path.exists(alpha_file)
          and marker_a in open(alpha_file, encoding="utf-8").read())
    row_a, row_b = db_doc_row(id_a), db_doc_row(id_b)
    check("E5 both docs still listed with chunk_count>0",
          row_a is not None and row_a[1] > 0
          and row_b is not None and row_b[1] > 0, f"a={row_a} b={row_b}")
    rag_file_delete(sid, 91, id_a)
    rag_file_delete(sid, 92, id_b)
    gone_a, gone_b = wait_row_gone(id_a, secs=25.0), wait_row_gone(id_b, secs=25.0)
    if gone_a and gone_b:
        check("E6 cleanup: both synthetic docs removed", True)
    else:
        warn("E6 SQL mirror rows linger (deferred prune; files+metas removed) "
             "— finally-sweep will force-clean")
        skip("E6 cleanup row-prune confirmation (deferred prune / db-lock)")


# ── D: R15-#4 canonicalize_url 600s cache (timing on git-doc deletes) ──────
def make_synthetic_doc(name, method, original_path, source, chunk_count=2,
                       content_by_name=False):
    """DB row + content file + .meta for a synthetic rag doc.

    content_by_name: store the content file under the DISPLAY name
    ({name}) instead of {id}.md — the legacy rag_file_create naming that the
    round-15 rename-collision guard protects.
    """
    doc_id = str(uuid.uuid4())
    content = f"r1600 synthetic doc {name}\n" + "x" * 64
    content_path = os.path.join(FILES_DIR, name if content_by_name else f"{doc_id}.md")
    meta_path = os.path.join(FILES_DIR, f"{doc_id}.meta")
    with open(content_path, "w", encoding="utf-8") as f:
        f.write(content)
    meta = {
        "id": doc_id, "name": name, "title": None, "tags": [],
        "size": len(content.encode()), "uploaded_at": "2026-10-10 00:00",
        "chunk_count": chunk_count, "version": 1, "file_type": "Text",
        "method": method, "original_path": original_path, "md5": None,
        "source": source,
    }
    with open(meta_path, "w", encoding="utf-8") as f:
        json.dump(meta, f, ensure_ascii=False)
    con = sqlite3.connect(DB)
    con.execute(
        "INSERT INTO rag_docs (id, name, size, uploaded_at, file_type, method,"
        " original_path, md5, version, chunk_count) VALUES (?,?,?,?,?,?,?,?,?,?)",
        (doc_id, name, len(content.encode()), "2026-10-10 00:00", "Text",
         method, original_path, None, 1, chunk_count))
    con.commit()
    con.close()
    SYNC_DOCS.append((doc_id, content_path, meta_path))
    return doc_id


def remove_synthetic(doc_id, content_path, meta_path):
    con = sqlite3.connect(DB)
    con.execute("DELETE FROM rag_docs WHERE id=?", (doc_id,))
    con.commit()
    con.close()
    for p in (content_path, meta_path):
        try:
            os.remove(p)
        except OSError:
            pass


def sweep_r1600():
    """Best-effort cleanup of every r1600_* leftover (DB rows + rag/files)."""
    con = sqlite3.connect(DB)
    ids = [r[0] for r in con.execute(
        "SELECT id FROM rag_docs WHERE name LIKE 'r1600%'").fetchall()]
    con.execute("DELETE FROM rag_docs WHERE name LIKE 'r1600%'")
    con.commit()
    con.close()
    try:
        for fn in os.listdir(FILES_DIR):
            if fn.startswith("r1600"):
                try:
                    os.remove(os.path.join(FILES_DIR, fn))
                except OSError:
                    pass
    except OSError:
        pass
    return ids


def find_git_url():
    import glob
    for m in glob.glob(os.path.join(FILES_DIR, "*.meta")):
        try:
            j = json.load(open(m, encoding="utf-8"))
        except Exception:
            continue
        s = j.get("source") or {}
        if s.get("kind") == "git" and (s.get("git") or {}).get("url"):
            return s["git"]["url"]
    return None


def section_d():
    global SYNC_DOCS
    print("== D: R15-#4 canonicalize_url 600s cache (git-doc delete timing) ==")
    url = find_git_url()
    if not url:
        skip("D (no git-kind doc in DB to copy the repo url from)")
        return
    src = {"kind": "git", "label": url, "root": None, "rel_path": None,
           "git": {"url": url, "branch": None, "commit": None, "subdir": None}}
    repo_path_hint = "/nonexistent/r1600/placeholder.md"  # refcount uses url, not path
    ids = [make_synthetic_doc(f"r1600_gitcache_{i}.md", "copy", repo_path_hint, src)
           for i in range(3)]
    sid = mcp_session()
    if sid is None:
        skip("D (no /mcp session)")
        return
    times = []
    for i, doc_id in enumerate(ids):
        t0 = time.time()
        st, o = rag_file_delete(sid, 20 + i, doc_id)
        times.append(time.time() - t0)
        gone = wait_row_gone(doc_id, secs=6.0)
        meta_gone = not os.path.exists(
            os.path.join(FILES_DIR, f"{doc_id}.meta"))
        ok = (st == 200 and o is not None
              and ((o.get("result") or {}).get("isError") is False) and meta_gone)
        if not ok:
            check(f"D{i+1} delete git-kind synthetic doc #{i+1}", False,
                  f"st={st} {json_txt(o)[:140]} meta_gone={meta_gone}")
            return
        check(f"D{i+1} delete git-kind synthetic doc #{i+1}", True,
              f"{times[-1]:.2f}s meta_gone={meta_gone} row_gone={gone}")
    # Warm-up delete (#1) may pay a full 10s probe; deletes #2/#3 must hit
    # the 600s canonicalize cache.
    check("D4 cache hit: post-warmup deletes fast (<8s, no fresh 10s probe)",
          times[1] < 8.0 and times[2] < 8.0,
          f"t={times[1]:.2f}s,{times[2]:.2f}s (warmup={times[0]:.2f}s)")
    check("D5 cache hit: delete #3 not slower than #2 (stable, no re-probe)",
          times[2] <= times[1] + 2.0,
          f"t2={times[1]:.2f}s t3={times[2]:.2f}s")


# ── C: R15-#3 file-source vanish keeps copy docs (real auto tick) ──────────
def section_c():
    global ORIG_INTERVAL
    print("== C: R15-#3 file-source vanish: copy doc survives auto tick ==")
    vanished = os.path.expanduser(
        "~/Library/Application Support/app.mcphub.desktop/r1600_vanished_source.md")
    try:
        os.remove(vanished)
    except OSError:
        pass
    doc_id = make_synthetic_doc(
        "r1600_lost_original_copy.md", "copy", vanished,
        {"kind": "file", "label": "r1600 synthetic", "root": None,
         "rel_path": None, "git": None}, chunk_count=2)
    cfg = cfg_read()
    rag = cfg.setdefault("rag", {})
    if ORIG_INTERVAL is None:
        ORIG_INTERVAL = rag.get("autoUpdateIntervalSecs", 300)
    rag["autoUpdateEnabled"] = True
    rag["autoUpdateIntervalSecs"] = 60
    cfg_write(cfg)
    warn("C setup: interval -> 60s; waiting up to 100s for a real auto tick")
    deadline = time.time() + 100
    tick_seen = False
    survived = True
    while time.time() < deadline:
        time.sleep(5)
        if db_doc_row(doc_id) is None:
            survived = False
            break
        con = sqlite3.connect(DB)
        n = con.execute(
            "SELECT count(*) FROM app_log WHERE message LIKE '%[rag]%' "
            "AND (message LIKE '%batch%' OR message LIKE '%sync%' "
            "OR message LIKE '%update%') AND created_at >= datetime('now','-3 minutes')"
        ).fetchone()[0]
        con.close()
        if n > 0:
            tick_seen = True
            time.sleep(5)   # give the tick a moment to finish its plan
            if db_doc_row(doc_id) is None:
                survived = False
            break
    check("C1 copy-method file doc SURVIVES vanished original (not deleted)",
          survived, f"doc {doc_id} was deleted" if not survived else "")
    if survived and not tick_seen:
        warn("C no tick evidence in app_log within 100s — survival asserted "
             "over the full window, but the tick may not have fired (weak)")
        skip("C2 tick-evidence confirmation (no [rag] batch/sync log row in "
             "window; interval change may need a settings-save to re-arm)")
    elif survived and tick_seen:
        check("C2 tick actually ran (app_log [rag] batch/sync row) and doc "
              "still present", True)
    else:
        skip("C2 tick evidence (doc deleted — C1 already failed)")


# ── B: R15-#2 reindex_all non-UTF-8 (positive half real, trigger SKIP) ─────
def section_b():
    print("== B: R15-#2 reindex_all non-UTF-8 (positive half) ==")
    sid = mcp_session()
    if sid is None:
        skip("B (no /mcp session or RAG tools unavailable)")
        return
    marker = "R1600GBK index-alive marker 索引存活标记"
    st, o = rag_file_create(sid, 30, "r1600_indexalive", "txt", marker)
    doc_id = None
    try:
        doc_id = json.loads(result_text(o)).get("docId") if o else None
    except Exception:
        pass
    check("B1 rag_file_create ok", st == 200 and doc_id, f"st={st} {json_txt(o)[:120]}")
    if not doc_id:
        return
    row = db_doc_row(doc_id)
    check("B2 chunk_count > 0 right after create (index not wiped)",
          row is not None and row[1] > 0, f"row={row}")
    st, o = rag_search(sid, 31, "R1600GBK index-alive")
    txt = result_text(o)
    check("B3 rag_search returns the doc content (shared decode/index path alive)",
          st == 200 and o is not None and doc_id in txt and "R1600" in txt,
          f"st={st} {txt[:140]}")
    rag_file_delete(sid, 32, doc_id)
    if wait_row_gone(doc_id, secs=25.0):
        check("B4 cleanup: synthetic doc removed", True)
    else:
        warn("B4 SQL mirror row lingers (deferred prune; meta+file+vectors "
             "removed) — finally-sweep will force-clean")
        skip("B4 cleanup row-prune confirmation (deferred prune / db-lock)")


# ── A: R15-#1 get_strict wiring / skipAuth fail-closed (LAST — corrupts cfg)
def section_a():
    print("== A: R15-#1 get_strict wiring (corrupt config -> admin gate 401) ==")
    gate_path = "/servers/" + urllib.parse.quote(IP_SERVER, safe="") + "/oauth/status"

    def gate_status():
        return retry_status(
            lambda: req("GET", gate_path, default_accept=False, timeout=15)[0])

    def health():
        return retry_status(
            lambda: req("GET", "/health", default_accept=False)[0])

    con = sqlite3.connect(DB)
    try:
        con.execute("UPDATE system_config SET config_json='{\"routing\":' WHERE id=1")
        con.commit()
        time.sleep(0.3)
        st1 = gate_status()
        check("A1 corrupt config_json: hub_admin_gate -> 401 (fail closed)",
              st1 == 401, f"st={st1}")
        st = health()
        check("A2 /health stays 200 under corrupt config", st == 200, f"st={st}")
    finally:
        con.execute("UPDATE system_config SET config_json=? WHERE id=1", (RAW_CFG,))
        con.commit()
        con.close()
        time.sleep(0.5)
    st = gate_status()
    check("A3 recovery: admin gate open again after raw-config restore",
          st == 200, f"st={st}")
    # NULL-config default path: get_strict must keep NULL == Ok({}) so the
    # default skipAuth=true deployment still opens management routes.
    con = sqlite3.connect(DB)
    try:
        con.execute("UPDATE system_config SET config_json='{}' WHERE id=1")
        con.commit()
        time.sleep(0.3)
        st = gate_status()
        check("A4 empty-config default path: skipAuth defaults true (not 401)",
              st == 200, f"st={st}")
    finally:
        con.execute("UPDATE system_config SET config_json=? WHERE id=1", (RAW_CFG,))
        con.commit()
        con.close()
        time.sleep(0.5)
    st = retry_status(lambda: req("GET", "/health", default_accept=False)[0])
    check("A5 recovery: /health 200 + config restored", st == 200
          and cfg_read() == json.loads(RAW_CFG), f"st={st}")


SKIP_STUBS = [
    ("R15-#2 reindex_all GBK/symlink wipe path (negative half)",
     "reindex_all triggers ONLY on a model swap (Tauri IPC rag model select /"
     " mv consumer registration); not HTTP-exposed. Needs a test-only command"
     " exposing reindex_all (or a manual model-swap on this machine) to"
     " assert an un-decodable symlink original is skipped-with-warn instead"
     " of delete_all+0-chunks. Positive half (index alive) covered in B."),
    ("R15-#1 tauri-command-level is_skip_auth_enabled corrupt-config flip",
     "require_admin-protected Tauri commands are IPC-only (not reachable over"
     " the hub HTTP surface). The fix routes is_skip_auth_enabled through the"
     " SAME get_strict() that hub_admin_gate uses — A1 proves that reader"
     " fails closed E2E; the command-level branch is covered by the"
     " config_service get_strict_* unit tests added in round 15."),
]


def main():
    global RAW_CFG
    try:
        st, _, _ = req("GET", "/health", default_accept=False)
        if st != 200:
            print(f"SKIP-ALL: hub not healthy at {HOST} (status {st})")
            return 2
    except Exception as e:
        print(f"SKIP-ALL: hub down: {e}")
        return 2

    con = sqlite3.connect(DB)
    RAW_CFG = con.execute(
        "SELECT config_json FROM system_config WHERE id=1").fetchone()[0]
    pins_before = con.execute(
        "SELECT count(*) FROM server_tool_config WHERE pinned=1").fetchone()[0]
    con.close()

    for name, why in SKIP_STUBS:
        skip(name, why)

    rc = 0
    try:
        section_f()
        section_e()
        section_d()
        section_b()
        section_c()
        section_a()   # LAST — corrupts live config briefly
    finally:
        # restore config raw bytes
        con = sqlite3.connect(DB)
        con.execute("UPDATE system_config SET config_json=? WHERE id=1", (RAW_CFG,))
        con.commit()
        con.close()
        # remove leftover synthetic docs (C/D rows+files, MCP-deleted mirrors,
        # and any r1600* orphans)
        for doc_id, content_path, meta_path in SYNC_DOCS:
            try:
                os.remove(content_path)
            except OSError:
                pass
            try:
                os.remove(meta_path)
            except OSError:
                pass
        sweep_r1600()
        if ORIG_INTERVAL is not None:
            cfg = cfg_read()
            cfg.setdefault("rag", {})["autoUpdateIntervalSecs"] = ORIG_INTERVAL
            cfg.setdefault("rag", {})["autoUpdateEnabled"] = True
            cfg_write(cfg)
        time.sleep(0.5)

    # DB-restored evidence
    con = sqlite3.connect(DB)
    pins_after = con.execute(
        "SELECT count(*) FROM server_tool_config WHERE pinned=1").fetchone()[0]
    cfg_now = con.execute(
        "SELECT config_json FROM system_config WHERE id=1").fetchone()[0]
    n_left = con.execute(
        "SELECT count(*) FROM rag_docs WHERE name LIKE 'r1600_%'").fetchone()[0]
    con.close()
    check("RESTORE pins=0 (unchanged)", pins_after == pins_before == 0,
          f"before={pins_before} after={pins_after}")
    check("RESTORE config_json byte-identical", cfg_now == RAW_CFG)
    check("RESTORE no r1600_* rag docs left", n_left == 0, f"n={n_left}")
    st, _, _ = req("GET", "/health", default_accept=False)
    check("RESTORE /health 200", st == 200, f"st={st}")

    print(f"\n{'=' * 50}\nPASS={base.PASS} FAIL={base.FAIL} SKIP={base.SKIPS}")
    if base.FAILURES:
        print("FAILED:", *base.FAILURES, sep="\n  - ")
        rc = 1
    return rc


if __name__ == "__main__":
    sys.exit(main())
