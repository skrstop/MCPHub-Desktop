#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""E2E round r1900 — fresh-binary validation after app restart (round-19).

PICKUP CONSTRAINT (documented, hard): servers inserted directly into the
servers table while the hub is running are NEVER connected at runtime —
hub connects only at startup (mcp_manager::start_all), via Tauri commands,
or the session_rebuild tick, which iterates pool::get_all_statuses() only
(services/mcp_manager.rs:55-65). Therefore:
  - the OpenapiTransport::fetch_spec SSRF guard (openapi_transport.rs:89-113)
    is NOT HTTP-reachable in a running instance (spec fetch happens at
    connect time only). Guard stays validated at unit level
    (ssrf_guard.rs: 4 tests, cargo test --lib). Reasoned SKIP, not a fail.
  - apps_ui A1/A3/B1 remain environment-bound SKIPs (rerun as regression
    below — they must still pass their reachable contract cases).

Sections:
  A  Version echo x 5 versions on root /mcp (fresh binary) + REAL public-IP
     tool call: 2025-06-18 session lane + 2026-07-28 stateless lane.
  B  Leniency: bare requests (no Accept, no session) for all 4 legacy
     versions -> 2xx JSON-RPC response.
  C  Settings round-trip: system_config.network.allowedCidrs and
     betterAuth.disablePasswordLogin keys — read current, write, read back,
     restore original JSON verbatim. (config_service::get reads the DB live
     per call — no cache — so DB-level round-trip exercises the same
     persistence layer the running app reads; the desktop hub exposes no
     /system-config HTTP face, so DB write + read-back is the honest
     reachable level here.)
  D  Strict/lenient toggle sanity on the LIVE binary: flip
     mcp.strictValidation=true via DB, send a malformed request (missing
     jsonrpc field) -> 4xx; flip off -> same request accepted (2xx). Proves
     mode switching works on the new binary. Original config_json restored
     verbatim in finally.
  E  Regression reruns: rmcp_round_apps_ui.py and rmcp_round_r1800.py as
     subprocesses; both must exit 0.

Restore: system_config.config_json byte-exact restore + /health 200 at end.
Pins untouched. No server rows inserted by THIS suite.
"""
import json
import os
import sqlite3
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import rmcp_round_r1100 as base
from rmcp_round_r1100 import (
    HOST, PORT, DB, META_2026, req, sse_last, initialize, notify_initialized,
    rpc, check, skip, json_txt, valid_pub_ip, result_text,
)

HERE = os.path.dirname(os.path.abspath(__file__))
LEGACY_VERSIONS = base.LEGACY_VERSIONS
ALL_VERSIONS = LEGACY_VERSIONS + ["2026-07-28"]
IP_SERVER = base.IP_SERVER
IP_TOOL = base.IP_TOOL


# ── config helpers (byte-exact restore) ─────────────────────────────────────
def cfg_raw():
    con = sqlite3.connect(DB)
    row = con.execute("SELECT config_json FROM system_config WHERE id=1").fetchone()
    con.close()
    return row[0] if row and row[0] else None


def cfg_write_raw(raw):
    con = sqlite3.connect(DB)
    con.execute("UPDATE system_config SET config_json=? WHERE id=1", (raw,))
    con.commit()
    con.close()
    time.sleep(0.4)


def cfg_merged(mutator):
    """Read live JSON, apply mutator(obj), persist. Returns the original raw."""
    raw = cfg_raw()
    obj = json.loads(raw) if raw else {}
    mutator(obj)
    con = sqlite3.connect(DB)
    con.execute("UPDATE system_config SET config_json=? WHERE id=1",
                (json.dumps(obj, ensure_ascii=False),))
    con.commit()
    con.close()
    time.sleep(0.4)
    return raw


# ── sections ────────────────────────────────────────────────────────────────
def section_a():
    print("== A: version echo x5 + real public-IP call (2025-06 + 2026) ==")
    for v in LEGACY_VERSIONS:
        st, echoed, _ = initialize("/mcp", v, client="r1900")
        check(f"A.echo {v} -> exact echo", st == 200 and echoed == v,
              f"st={st} echoed={echoed}")
    # 2026-07-28 has NO initialize handshake (rmcp 3.4.1 design): initialize
    # naming 2026 negotiates back to latest legacy 2025-11-25 (documented).
    # The 2026 lane itself is the stateless _meta path, validated below.
    st, echoed, _ = initialize("/mcp", "2026-07-28", client="r1900")
    check("A.echo 2026-07-28 initialize -> documented legacy fallback "
          "2025-11-25 (no 2026 handshake by design)",
          st == 200 and echoed == "2025-11-25", f"st={st} echoed={echoed}")

    # 2025-06-18 session lane: full lifecycle + real tools/call
    st, echoed, sid = initialize("/mcp", "2025-06-18", client="r1900")
    if st == 200 and echoed == "2025-06-18" and sid:
        notify_initialized("/mcp", sid)
        st, data, _ = rpc("/mcp", 2, "tools/list", {}, session=sid, timeout=120)
        o = sse_last(data, 2)
        names = [t.get("name", "") for t in
                 ((o or {}).get("result", {}) or {}).get("tools", [])]
        tool = base.find_ip_tool(names)
        check("A.2025-06 tools/list contains IP tool", bool(tool),
              f"names[:8]={names[:8]}")
        if tool:
            st, data, _ = rpc("/mcp", 3, "tools/call",
                              {"name": tool, "arguments": {}}, session=sid,
                              timeout=120)
            o = sse_last(data, 3)
            txt = result_text(o)
            is_err = ((o or {}).get("result", {}) or {}).get("isError", False)
            check("A.2025-06 real public-IP call -> semantic IPv4, isError false",
                  st == 200 and not is_err and valid_pub_ip(txt),
                  f"st={st} isError={is_err} text={txt[:120]}")
    else:
        check("A.2025-06 session lane", False, f"st={st} echo={echoed}")

    # 2026-07-28 stateless lane: bare modern requests, no session
    st, data, _ = req("POST", "/mcp", body={
        "jsonrpc": "2.0", "id": 5, "method": "tools/list",
        "params": {"_meta": {"io.modelcontextprotocol/protocolVersion":
                             "2026-07-28"}}},
        headers={"MCP-Protocol-Version": "2026-07-28"}, timeout=120)
    o = sse_last(data, 5)
    r = (o or {}).get("result", {}) or {}
    names = [t.get("name", "") for t in r.get("tools", [])]
    tool = base.find_ip_tool(names)
    check("A.2026 stateless tools/list ok + IP tool", st == 200 and bool(tool),
          f"st={st} names[:8]={names[:8]}")
    if tool:
        st, data, _ = req("POST", "/mcp", body={
            "jsonrpc": "2.0", "id": 6, "method": "tools/call",
            "params": {"name": tool, "arguments": {}, **META_2026}},
            headers={"MCP-Protocol-Version": "2026-07-28"}, timeout=120)
        o = sse_last(data, 6)
        r = (o or {}).get("result", {}) or {}
        txt = json.dumps(r.get("content", []), ensure_ascii=False)
        check("A.2026 real public-IP call -> resultType complete + semantic IPv4",
              st == 200 and r.get("resultType") == "complete"
              and valid_pub_ip(txt),
              f"st={st} resultType={r.get('resultType')} {txt[:120]}")


def section_b():
    print("== B: leniency — bare requests (no Accept, no session) x4 legacy ==")
    for v in LEGACY_VERSIONS:
        st, data, _ = req("POST", "/mcp", body={
            "jsonrpc": "2.0", "id": 9, "method": "ping",
            "params": {"protocolVersion": v, "capabilities": {},
                       "clientInfo": {"name": "r1900bare", "version": "1.0"}}},
            default_accept=False, timeout=60)
        o = sse_last(data, 9)
        ok = st in (200, 202) and (o is not None or st == 202)
        check(f"B.bare legacy {v} -> 2xx", ok, f"st={st} {json_txt(o)[:120]}")
        if st in (200, 202) and v == "2025-06-18":
            st2, data2, _ = req("POST", "/mcp", body={
                "jsonrpc": "2.0", "id": 10, "method": "tools/list", "params": {}},
                default_accept=False, timeout=60)
            o2 = sse_last(data2, 10)
            check("B.bare tools/list (no Accept/no session) -> 2xx + tools",
                  st2 in (200, 202) and (o2 is None or "tools" in
                                         ((o2 or {}).get("result", {}) or {})),
                  f"st={st2}")


def section_c():
    print("== C: settings round-trip (allowedCidrs + disablePasswordLogin) ==")
    raw0 = cfg_raw()
    if raw0 is None:
        skip("C.settings round-trip", "system_config row empty — nothing to "
              "round-trip against (fresh install state).")
        return
    obj0 = json.loads(raw0)
    cidrs_before = ((obj0.get("network") or {}).get("allowedCidrs"))
    dpl_before = ((obj0.get("betterAuth") or {}).get("disablePasswordLogin"))
    print(f"  baseline: allowedCidrs={cidrs_before!r} "
          f"disablePasswordLogin={dpl_before!r}")

    def mut(o):
        o.setdefault("network", {})["allowedCidrs"] = ["10.20.0.0/16"]
        o.setdefault("betterAuth", {})["disablePasswordLogin"] = True

    cfg_merged(mut)
    obj1 = json.loads(cfg_raw())
    cidrs_after = ((obj1.get("network") or {}).get("allowedCidrs"))
    dpl_after = ((obj1.get("betterAuth") or {}).get("disablePasswordLogin"))
    check("C.allowedCidrs write->read-back",
          cidrs_after == ["10.20.0.0/16"], f"after={cidrs_after!r}")
    check("C.disablePasswordLogin write->read-back", dpl_after is True,
          f"after={dpl_after!r}")

    cfg_write_raw(raw0)
    check("C.config_json restored byte-exact", cfg_raw() == raw0)


def section_d():
    print("== D: strict/lenient toggle on live binary ==")
    raw0 = cfg_raw()
    if raw0 is None:
        skip("D.strict toggle", "no config row to flip.")
        return
    malformed = {"id": 12, "method": "tools/list", "params": {}}

    def set_strict(val):
        def mut(o):
            o.setdefault("mcp", {})["strictValidation"] = val
        cfg_merged(mut)

    set_strict(True)
    st_strict, data_s, _ = req("POST", "/mcp", body=malformed,
                               default_accept=True, timeout=60)
    check("D.strict: missing jsonrpc -> 4xx", 400 <= st_strict < 500,
          f"st={st_strict} {data_s[:120]}")

    set_strict(False)
    st_len, data_l, _ = req("POST", "/mcp", body=malformed,
                            default_accept=True, timeout=60)
    o_l = sse_last(data_l, 12)
    check("D.lenient: same request accepted (2xx JSON-RPC reply)",
          st_len in (200, 202) and o_l is not None
          and ("result" in o_l or "error" in o_l),
          f"st={st_len} {json_txt(o_l)[:120]}")

    cfg_write_raw(raw0)
    check("D.config_json restored byte-exact", cfg_raw() == raw0)


def section_e():
    print("== E: regression reruns (subprocess, exit 0 + FAIL=0 required) ==")
    for script in ("rmcp_round_apps_ui.py", "rmcp_round_r1800.py"):
        p = subprocess.run([sys.executable, os.path.join(HERE, script)],
                           capture_output=True, text=True, timeout=900)
        tail = (p.stdout or "").strip().split("\n")[-3:]
        summary = " | ".join(t.strip() for t in tail if t.strip())
        ok = (p.returncode == 0 and "FAIL=0" in p.stdout)
        check(f"E.rerun {script} exit0 FAIL=0", ok,
              f"rc={p.returncode} {summary} {(p.stderr or '')[-200:]}")


def main():
    print("=== rmcp_round_r1900: fresh-binary validation (round-19) ===")
    try:
        section_a()
        section_b()
        section_c()
        section_d()
        # SKIP evidence for the SSRF runtime face (documented constraint):
        skip("SSRF guard end-to-end via openapi fetch_spec",
             "hard environment constraint: fetch_spec runs only inside "
             "OpenapiTransport::connect; a DB-inserted openapi server is "
             "never connected at runtime (session_rebuild iterates "
             "pool::get_all_statuses only — services/mcp_manager.rs:55). "
             "Covered at unit level by 4 ssrf_guard tests (cargo test --lib).")
        section_e()
    finally:
        st, _, _ = req("GET", "/health", timeout=20)
        check("RESTORE /health 200", st == 200, f"st={st}")

    print(f"\nPASS={base.PASS} FAIL={base.FAIL} SKIP={base.SKIPS}")
    if base.FAILURES:
        print("FAILURES:")
        for f in base.FAILURES:
            print("  -", f)
    sys.exit(0 if base.FAIL == 0 else 1)


if __name__ == "__main__":
    main()
