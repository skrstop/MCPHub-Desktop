#!/usr/bin/env python3
"""E2E round r1500 (round-15): FIX-REGRESSION suite for campaign rounds 9-14.

Each case names the round+fix it guards so a future failure identifies the
broken invariant.  Semantics sourced from doc/oauth_review_campaign_round9..14
reports and AGENTS.md.

A  [R9-H A14] empty-pin gate semantics: zero pins -> root $smart search
   returns hits (empty pin set must NOT wipe results), describe/call gate
   inactive for connected unpinned tools.
B  [R9/R10 call-lane parity] pin gate ACTIVE contrast (positive half of the
   same invariant): one pin -> search filters to it, describe/call succeed.
C  [R10-M merge_limit no-cut] standard mode, fullSchemaTopN unset -> every
   search hit carries inputSchema (no pre-filter truncation); setting
   fullSchemaTopN=1 trims schemas past the top hit WITHOUT dropping result
   count (cut only past topN).  Restore fullSchemaTopN absent.
D  [R11-M OAuth field-scoped saves] config round-trip: writing one oauth key
   (TTL) through the settings persistence path leaves every other key in the
   same namespace untouched (field-scoped saves must not clobber siblings).
   Restore snapshot.
E  [R12-H get_strict fail-closed wiring] corrupt system_config.config_json
   -> POST /mcp must be 401 (bearer check fails closed on config read
   failure) — run LAST; raw config_json bytes restored in finally; recovery
   asserted (health 200 + a fresh initialize 200).
   NOTE: the equivalent unit tests live in config_service.rs
   (get_strict_* — round 13); this case proves the *wiring* (check_bearer_auth
   actually routes through get_strict), which only an E2E can see.

SKIPped (reason printed):
- R9 describe sleeping on-demand pinned tool (wake guidance): needs a real
  sleeping on-demand server; not reproducible without a long-lived stdio
  on-demand fixture.
- R11 Authorization scheme-carrying passthrough (rmcp_http_transport): needs
  an upstream that requires auth; no such fixture.
- R11 mcp-session-id/accept/last-event-id reserved-header filter: same
  upstream-config dependency.
- R12 session_rebuild skips starting: timing race on a slow-starting server.
- R13 describe renamed/removed-tool message: no rename API for upstream tools.
- R14 list_servers/get_server/copy gating: Tauri-IPC-only surface, not
  reachable over HTTP.

Cleanup: pins, fullSchemaTopN, oauthServer block, config_json all restored.
Exit 0 only if all real cases pass.
"""
import json
import os
import sqlite3
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import rmcp_round_r1100 as base
from rmcp_round_r1100 import (
    HOST, DB,
    req, sse_last, cfg_read, cfg_write, initialize, result_text,
)
from pin_helper import pin, unpin
from rmcp_round_r900 import smart_session, smart_call, tool_error_code

check = base.check
skip = base.skip
warn = base.warn

IP_SERVER = base.IP_SERVER          # 本机公网ip查询
IP_TOOL = "getPublicIp"
SEARCH_QUERY = "browser"

RAW_CFG = None          # raw config_json text for restore
PIN_SNAPSHOT = None


def search_hits(sid, query=SEARCH_QUERY, limit=50, rid=10):
    st, data, _ = smart_call(sid, rid, "smart_route_search", {"query": query, "limit": limit})
    o = sse_last(data, rid)
    try:
        return st, json.loads(result_text(o))["tools"], o
    except Exception:
        return st, [], o


def clear_all_pins():
    con = sqlite3.connect(DB_PATH)
    con.execute("UPDATE server_tool_config SET pinned=0")
    con.commit()
    con.close()
    time.sleep(0.6)


DB_PATH = DB


# ── A: [R9-H A14] empty-pin gate semantics ──────────────────────────────────
def section_a():
    print("== A: R9-H A14 empty-pin gate semantics (zero pins) ==")
    clear_all_pins()
    sid = smart_session()
    if sid is None:
        skip("A (no $smart session)")
        return
    # A1 — the A14 invariant: zero pins -> search must still return hits.
    st, hits, o = search_hits(sid, rid=21)
    check("A1 [R9-H A14] zero pins: search 'browser' returns hits (no empty-set wipe)",
          st == 200 and tool_error_code(o) is None and len(hits) >= 1,
          f"st={st} n_hits={len(hits)} {base.json_txt(o)[:140]}")
    # A2 — describe on an unpinned CONNECTED tool: gate inactive -> success
    # (wake-guidance / refusal would mean the gate never deactivated).
    st, data, _ = smart_call(sid, 22, "smart_route_describe", {"toolName": IP_TOOL})
    o = sse_last(data, 22)
    txt = result_text(o)
    check("A2 [R9-A14] zero pins: describe unpinned connected tool succeeds",
          st == 200 and tool_error_code(o) is None and IP_TOOL in txt,
          f"st={st} {base.json_txt(o)[:140]}")
    check("A3 [R9-F1 contrast] zero pins: describe must NOT emit wake guidance",
          "wake" not in txt.lower() and "invoke smart_route_call" not in txt,
          txt[:120])
    # A4 — [R10-M describe fallback misdiagnosis] connected-server describe
    # must not be misdiagnosed as "server not connected, call to wake".
    check("A4 [R10-M] describe on connected tool returns schema, not misdiagnosis",
          tool_error_code(o) is None and "inputSchema" in txt, txt[:120])
    # A5 — [R10-H call lane empty-pin] direct call of unpinned connected tool.
    name = f"{IP_SERVER}-{IP_TOOL}"
    st, data, _ = smart_call(sid, 23, "smart_route_call", {"toolName": name, "arguments": {}})
    o = sse_last(data, 23)
    check("A5 [R10-H call-lane] zero pins: direct call unpinned tool -> real IPv4",
          st == 200 and tool_error_code(o) is None
          and (o.get("result", {}) or {}).get("isError") is False
          and base.valid_pub_ip(result_text(o)),
          f"st={st} {base.json_txt(o)[:160]}")


# ── B: [R9/R10] pin gate ACTIVE contrast ────────────────────────────────────
def section_b():
    print("== B: pin gate ACTIVE contrast (one pin) ==")
    pin(IP_SERVER, IP_TOOL)
    sid = smart_session()
    if sid is None:
        skip("B (no $smart session)")
        return
    st, hits, o = search_hits(sid, query="ip", rid=31)
    check("B1 [R9 parity] one pin: search filters to pinned server",
          st == 200 and len(hits) >= 1
          and all(h.get("serverName") == IP_SERVER for h in hits),
          f"st={st} hits={base.json_txt(hits)[:140]}")
    st, data, _ = smart_call(sid, 32, "smart_route_describe", {"toolName": IP_TOOL})
    o = sse_last(data, 32)
    check("B2 [R9 parity] one pin: describe pinned tool succeeds",
          st == 200 and tool_error_code(o) is None and IP_TOOL in result_text(o),
          f"st={st} {base.json_txt(o)[:140]}")
    name = f"{IP_SERVER}-{IP_TOOL}"
    st, data, _ = smart_call(sid, 33, "smart_route_call", {"toolName": name, "arguments": {}})
    o = sse_last(data, 33)
    check("B3 [R9 parity] one pin: call pinned tool -> real IPv4",
          st == 200 and tool_error_code(o) is None
          and base.valid_pub_ip(result_text(o)),
          f"st={st} {base.json_txt(o)[:160]}")
    unpin(IP_SERVER, IP_TOOL)
    sid = smart_session()
    st, hits, o = search_hits(sid, rid=34)
    check("B4 [R9-A14 restore] after unpin search functional again",
          st == 200 and tool_error_code(o) is None and len(hits) >= 1,
          f"st={st} n_hits={len(hits)}")


# ── C: [R10-M] merge_limit no-cut + fullSchemaTopN trimming ─────────────────
def section_c():
    print("== C: R10-M merge_limit no-cut / fullSchemaTopN trim ==")
    cfg = cfg_read()
    sr = cfg.setdefault("smartRouting", {})
    sr.pop("fullSchemaTopN", None)
    sr["progressiveDisclosure"] = False
    cfg_write(cfg)
    time.sleep(0.5)
    sid = smart_session()
    if sid is None:
        skip("C (no $smart session)")
        return
    st, hits, o = search_hits(sid, rid=41)
    n = len(hits)
    with_schema = sum(1 for h in hits if "inputSchema" in h)
    check("C1 [R10-M no-cut] topN unset: broad search returns hits", st == 200 and n >= 2,
          f"st={st} n={n}")
    check("C2 [R10-M no-cut] topN unset: EVERY hit carries inputSchema (no pre-trim)",
          n >= 2 and with_schema == n, f"n={n} with_schema={with_schema}")
    # fullSchemaTopN=1: exactly the first hit keeps its schema; the result
    # COUNT must not shrink (cut only past topN, never before filtering).
    cfg = cfg_read()
    cfg["smartRouting"]["fullSchemaTopN"] = 1
    cfg_write(cfg)
    time.sleep(0.5)
    sid = smart_session()
    st, hits2, o = search_hits(sid, rid=42)
    n2 = len(hits2)
    with_schema2 = sum(1 for h in hits2 if "inputSchema" in h)
    check("C3 [R10-M topN] topN=1: exactly 1 hit carries inputSchema",
          st == 200 and with_schema2 == 1, f"st={st} with_schema={with_schema2} n={n2}")
    check("C4 [R10-M topN] topN=1: result count not reduced by trimming (no-cut)",
          st == 200 and n2 == n, f"n_before={n} n_after={n2}")
    check("C5 [R10-M topN] topN=1: schema-less hits still carry name/description",
          st == 200 and all(("name" in h and "description" in h) for h in hits2),
          base.json_txt(hits2)[:160])
    cfg = cfg_read()
    cfg["smartRouting"].pop("fullSchemaTopN", None)
    cfg_write(cfg)


# ── D: [R11-M] OAuth field-scoped save round-trip ───────────────────────────
def section_d():
    print("== D: R11-M OAuth field-scoped save (no sibling clobber) ==")
    cfg = cfg_read()
    before = json.dumps(cfg.get("oauthServer") or {}, sort_keys=True, ensure_ascii=False)
    oauth = cfg.setdefault("oauthServer", {})
    old_ttl = oauth.get("accessTokenTtlSecs")
    oauth["accessTokenTtlSecs"] = (old_ttl if isinstance(old_ttl, (int, float)) else 3600) + 7
    cfg_write(cfg)
    after = cfg_read()
    got = (after.get("oauthServer") or {}).get("accessTokenTtlSecs")
    check("D1 [R11-M] field-scoped save: TTL persisted",
          got == oauth["accessTokenTtlSecs"], f"got={got}")
    snap = json.loads(before) if before != "{}" else {}
    now = after.get("oauthServer") or {}
    others_ok = True
    for k, v in snap.items():
        if k == "accessTokenTtlSecs":
            continue
        if now.get(k) != v:
            others_ok = False
            warn(f"D oauthServer.{k} drifted: {v!r} -> {now.get(k)!r}")
    check("D2 [R11-M] field-scoped save: sibling oauth keys untouched", others_ok)
    cfg = cfg_read()
    cfg["oauthServer"] = snap
    cfg_write(cfg)
    restored = json.dumps(cfg_read().get("oauthServer") or {}, sort_keys=True,
                          ensure_ascii=False)
    check("D3 [R11-M restore] oauthServer block identical after restore",
          restored == before)


# ── E: [R12-H] get_strict fail-closed WIRING (LAST — corrupts live config) ──
def section_e():
    print("== E: R12-H get_strict fail-closed wiring (corrupt config -> 401) ==")
    con = sqlite3.connect(DB)
    try:
        con.execute("UPDATE system_config SET config_json='{corrupt' WHERE id=1")
        con.commit()
        time.sleep(0.2)
        st, data, _ = req("POST", "/mcp", body={
            "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "r1500", "version": "1.0"}}})
        check("E1 [R12-H] corrupt config_json: POST /mcp initialize -> 401 (fail closed)",
              st == 401, f"st={st} {data[:120]}")
    finally:
        con.execute("UPDATE system_config SET config_json=? WHERE id=1", (RAW_CFG,))
        con.commit()
        con.close()
        time.sleep(0.5)
    st, _, _ = req("GET", "/health", default_accept=False)
    check("E2 [R12-H recovery] /health 200 after restore", st == 200, f"st={st}")
    st, echoed, _ = initialize("/mcp", "2025-06-18")
    check("E3 [R12-H recovery] initialize 200 after restore",
          st == 200 and echoed == "2025-06-18", f"st={st} echo={echoed}")


SKIP_STUBS = [
    ("R9-F1 describe sleeping on-demand pinned tool (wake guidance)",
     "needs a real sleeping on-demand stdio server; not reproducible here"),
    ("R11-H Authorization scheme-carrying header passthrough (rmcp_http_transport)",
     "requires an upstream that enforces auth; no auth upstream fixture"),
    ("R11-M reserved-header filter (mcp-session-id/accept/last-event-id)",
     "same upstream-config dependency; asserting needs an invasive server config"),
    ("R12-L session_rebuild skips starting servers",
     "timing race on a slow-starting (npx download) server; not deterministic"),
    ("R13 describe renamed/removed-tool message",
     "no API to rename/remove an upstream tool while keeping the config row"),
    ("R14-H list_servers/get_server/copy-path gating",
     "Tauri-IPC-only surface; not reachable over HTTP (covered by command audits)"),
]


def main():
    global RAW_CFG, PIN_SNAPSHOT
    try:
        st, _, _ = req("GET", "/health", default_accept=False)
        if st != 200:
            print(f"SKIP-ALL: hub not healthy at {HOST}:{PORT} (status {st})")
            return 2
    except Exception as e:
        print(f"SKIP-ALL: hub down at {HOST}:{PORT}: {e}")
        return 2

    con = sqlite3.connect(DB)
    RAW_CFG = con.execute("SELECT config_json FROM system_config WHERE id=1").fetchone()[0]
    PIN_SNAPSHOT = con.execute(
        "SELECT server_name, item_name FROM server_tool_config WHERE pinned=1").fetchall()
    con.close()

    for name, why in SKIP_STUBS:
        skip(name, why)

    rc = 0
    try:
        section_a()
        section_b()
        section_c()
        section_d()
        section_e()   # LAST — corrupts live config briefly
    finally:
        con = sqlite3.connect(DB)
        con.execute("UPDATE system_config SET config_json=? WHERE id=1", (RAW_CFG,))
        con.commit()
        con.close()
        time.sleep(0.5)
        con = sqlite3.connect(DB)
        con.execute("UPDATE server_tool_config SET pinned=0")
        for server, tool in PIN_SNAPSHOT:
            con.execute("UPDATE server_tool_config SET pinned=1 "
                        "WHERE server_name=? AND item_type='tool' AND item_name=?",
                        (server, tool))
        con.commit()
        con.close()
        time.sleep(0.5)
        cfg = cfg_read()
        if (cfg.get("smartRouting") or {}).get("fullSchemaTopN") is not None:
            warn("RESTORE-CHECK: smartRouting.fullSchemaTopN still set")
        if json.dumps(cfg.get("oauthServer") or {}, sort_keys=True) != json.dumps(
                (json.loads(RAW_CFG) if RAW_CFG else {}).get("oauthServer") or {},
                sort_keys=True):
            warn("RESTORE-CHECK: oauthServer block drifted")

    print(f"\n{'=' * 50}\nPASS={base.PASS} FAIL={base.FAIL} SKIP={base.SKIPS}")
    if base.FAILURES:
        print("FAILED:", *base.FAILURES, sep="\n  - ")
        rc = 1
    return rc


if __name__ == "__main__":
    sys.exit(main())
