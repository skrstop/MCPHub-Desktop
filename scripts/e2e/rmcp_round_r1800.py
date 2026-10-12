#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""E2E round r1800 (round 18): SSRF guard surface + settings round-trip +
apps_ui regression rerun — WITHOUT an app restart.

A  [SSRF guard, guarded surface = OpenAPI transport spec fetch] The guard
   (services/ssrf_guard.rs) fires in exactly ONE place: OpenapiTransport::
   fetch_spec (mcp/openapi_transport.rs:106) at transport CONNECT, reading
   system_config.network.allowedCidrs PER REQUEST (no cache). Connect is
   reachable only at startup (mcp_manager::start_all), via Tauri
   add/update/toggle/reload commands, or the session_rebuild tick — and the
   rebuild tick only considers servers ALREADY in the pool (mcp_manager.rs:55
   get_all_statuses). The hub's HTTP router (http_server.rs build_router)
   has NO management route for server create/update/connect or system-config
   PUT — verified by enumerating every route() call. Consequence (observed,
   A0): a server row inserted directly into the DB while the app runs is
   never connected, so the guard-fire assertions (A1/A2/A4/A5 + the
   allowedCidrs carve-out A3b) are SKIPped with code-cited reasoning; the
   registered rows + config edits are real and fully restored in finally.
   Expected guard behavior per variant (from ssrf_guard.rs, documented):
     loopback spec_url  -> "Blocked target IP: 127.0.0.1" (IP literal path,
                           line 180-184) or "Blocked resolved IP" for names.
     allowedCidrs carve-out ["127.0.0.1/32"] -> fetch PROCEEDS to the mock.
     public-IP 404 URL  -> guard does NOT block; failure is HTTP 404 from
                           fetch (different error class than UNSAFE_URL).
     file://            -> "Disallowed URL scheme: file" (line 170-172).
     decimal-IP host    -> NEUTRALIZED BY PARSER: Rust `url` implements the
                           WHATWG host parser, so "http://2130706433/" is
                           normalized to 127.0.0.1 BEFORE host_str() — the
                           decimal-IP bypass never reaches DNS; blocked as an
                           IP literal. (If a parser skipped normalization it
                           would fall to lookup_host and fail there.)
B  [sync regression sweep] 5 versions initialize echo EXACT on /mcp root +
   REAL public-IP tools/call (2025-06-18 session lane + 2026-07-28 stateless
   _meta lane) with retry-resilient IP (pattern from r1700 section E).
C  [settings round-trip, NEW keys] No HTTP PUT route exists for
   system-config (verified above), so the persistence layer is exercised
   directly: cfg write -> read-back -> identical (the same store
   config_service::update deep-merges into and get() reads per request):
   C1 network.allowedCidrs=["10.0.0.0/8"] round-trip (list equality);
   C2 disablePasswordLogin true -> read back -> false -> read back;
   C3 invalid CIDR ["10.0.0.0/33"] stored verbatim (validation is
      guard-side fail-closed at read time, ssrf_guard.rs:156-157).
D  [apps_ui regression] run rmcp_round_apps_ui.py as a subprocess, parse its
   PASS/FAIL/SKIP summary; expect FAIL==0, PASS>=7, SKIP<=2 (A1/A3/B1 skip
   under the same runtime-pickup constraint documented in that suite).

Restore: config_json snapshot restored verbatim + baseline asserted
(strictValidation=false top+mcp, exposeHttp=true), inserted server rows
deleted (count==0), /health 200.
"""
import json
import os
import re
import sqlite3
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import rmcp_round_r1100 as base
from rmcp_round_r1100 import (
    HOST, DB, LEGACY_VERSIONS, IP_SERVER, IP_TOOL, META_2026,
    req, sse_last, cfg_read, cfg_write, initialize, notify_initialized, rpc,
    result_text, json_txt, valid_pub_ip, check, skip, warn,
)
from rmcp_round_r1700 import resilient_ip_call

SPEC_PORT = 0  # dynamic; bound with port 0 and set by start_spec_mock()
VARIANTS = []      # rebuilt after the mock binds (needs the real port)
MINIMAL_SPEC = {}  # ditto
SSRF_SERVER = "r1800ssrf"


def _build_spec_fixtures(port):
    variants = [
        # (label, spec_url, expected-guard-behavior-if-it-ever-connected)
        ("loopback", f"http://127.0.0.1:{port}/openapi.json",
         "BLOCKED (IP literal path: 'Blocked target IP: 127.0.0.1')"),
        ("file-scheme", "file:///etc/passwd",
         "BLOCKED ('Disallowed URL scheme: file')"),
        ("decimal-ip", "http://2130706433/openapi.json",
         "BLOCKED (WHATWG parser normalizes 2130706433 -> 127.0.0.1 before "
         "host_str; if unparsed it dies at lookup_host)"),
    ]
    spec = {
        "openapi": "3.0.0",
        "info": {"title": "r1800 mock", "version": "1.0.0"},
        "servers": [{"url": f"http://127.0.0.1:{port}"}],
        "paths": {"/ping": {"get": {
            "summary": "ping", "operationId": "ping",
            "responses": {"200": {"description": "ok",
                                  "content": {"text/plain": {"schema": {"type": "string"}}}}}}}},
    }
    return variants, spec


class SpecHandler(BaseHTTPRequestHandler):
    hits = {"spec": 0}

    def log_message(self, fmt, *args):
        pass

    def do_GET(self):
        if self.path.endswith("/openapi.json"):
            SpecHandler.hits["spec"] += 1
            body = json.dumps(MINIMAL_SPEC).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        else:
            self.send_response(404)
            self.send_header("Content-Length", "0")
            self.end_headers()


def start_spec_mock():
    srv = ThreadingHTTPServer(("127.0.0.1", 0), SpecHandler)
    global SPEC_PORT, VARIANTS, MINIMAL_SPEC
    SPEC_PORT = srv.server_address[1]
    VARIANTS, MINIMAL_SPEC = _build_spec_fixtures(SPEC_PORT)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv


# ── DB helpers ──────────────────────────────────────────────────────────────
def insert_openapi_server(name, spec_url):
    openapi = json.dumps({"url": spec_url, "schema": None, "version": "3.1.0",
                          "security": None, "passthroughHeaders": []},
                         ensure_ascii=False)
    con = sqlite3.connect(DB)
    con.execute(
        """INSERT INTO servers (id, name, server_type, url, enabled,
             description, openapi)
           VALUES (lower(hex(randomblob(16))), ?, 'openapi', NULL, 1,
                   'r1800 ssrf e2e temp', ?)
           ON CONFLICT(name) DO UPDATE SET openapi=excluded.openapi,
             server_type='openapi', enabled=1""",
        (name, openapi))
    con.commit()
    con.close()


def delete_servers():
    con = sqlite3.connect(DB)
    con.execute("DELETE FROM servers WHERE name LIKE 'r1800%'")
    con.commit()
    n = con.execute(
        "SELECT count(*) FROM servers WHERE name LIKE 'r1800%'").fetchone()[0]
    con.close()
    return n


def pool_names():
    try:
        st, data, _ = req("GET", "/servers", timeout=30)
        if st != 200:
            return None
        return {s.get("name") for s in json.loads(data).get("servers", [])}
    except Exception:
        return None


def wait_pickup(name, secs=40.0):
    """Poll GET /servers (covers one 30s session_rebuild tick) for `name`."""
    deadline = time.time() + secs
    seen = False
    names = None
    while time.time() < deadline:
        names = pool_names()
        if names and name in names:
            seen = True
            break
        time.sleep(2.5)
    return seen, names


# ── A: SSRF guard surface ───────────────────────────────────────────────────
def section_a():
    print("== A: SSRF guard (openapi spec fetch surface) ==")
    for label, url, _ in VARIANTS:
        insert_openapi_server(f"{SSRF_SERVER}-{label}", url)
    insert_openapi_server(f"{SSRF_SERVER}-allow",
                          f"http://127.0.0.1:{SPEC_PORT}/openapi.json")

    # A0: empirical runtime-pickup evidence (40s covers one rebuild tick).
    picked, names = wait_pickup(f"{SSRF_SERVER}-loopback", secs=40.0)
    if names is None:
        # pool_names() errored — the absence-of-pickup claim is unverified;
        # fail loudly instead of passing vacuously.
        check("A0 DB-inserted openapi server NEVER picked up at runtime "
              "(GET /servers stays clean for 40s+)", False,
              "pool_names API failed (GET /servers error) — cannot verify "
              "the never-picked-up claim")
    else:
        check("A0 DB-inserted openapi server NEVER picked up at runtime "
              "(GET /servers stays clean for 40s+)",
              not picked, f"pool={sorted(names)}")

    # A1 loopback blocked
    skip("A1 loopback spec_url connect -> UNSAFE_URL blocked",
         "guarded fn (OpenapiTransport::fetch_spec) runs only at transport "
         "connect; connect is startup-only / Tauri-commands-only / rebuild-"
         "tick-pooled-only (mcp_manager.rs:55) and the HTTP router has no "
         "server-connect or system-config PUT route. Expected: 'Blocked "
         "target IP: 127.0.0.1'. Needs hub restart after seeding.")

    # A2 public-IP 404 URL: guard must NOT block (failure class would be
    # 'Failed to fetch OpenAPI spec ... HTTP 404', NOT UNSAFE_URL).
    skip("A2 public-IP 404 URL -> guard does NOT block (failure class is "
         "HTTP 404, not UNSAFE_URL)",
         "same connect-unreachable constraint as A1.")

    # A3 allowedCidrs carve-out: set via DB (guard reads per request, no
    # cache — ssrf_guard.rs:13-14 + openapi_transport.rs:92-105), verify
    # read-back, then check the carve-out server.
    cfg = cfg_read()
    cfg["network"] = {"allowedCidrs": ["127.0.0.1/32"]}
    cfg_write(cfg)
    back = (cfg_read().get("network") or {}).get("allowedCidrs")
    check("A3a network.allowedCidrs=['127.0.0.1/32'] persists (per-request "
          "config read source)", back == ["127.0.0.1/32"], f"back={back}")
    picked_a, names_a = wait_pickup(f"{SSRF_SERVER}-allow", secs=40.0)
    hits_before = SpecHandler.hits["spec"]  # delta baseline (suite-local mock)
    if picked_a:
        check("A3b allowedCidrs carve-out -> loopback spec fetch PROCEEDS "
              "(mock spec actually hit: delta >= 1 since pickup wait began)",
              SpecHandler.hits["spec"] >= hits_before + 1,
              f"mock_hits={SpecHandler.hits['spec']} "
              f"hits_before={hits_before}")
    else:
        skip("A3b allowedCidrs carve-out -> loopback spec fetch proceeds",
             "config carve-out verified persisted (A3a) but the connect "
             "trigger is unreachable without restart (A1 reasoning); mock "
             f"hits={SpecHandler.hits['spec']} pool={sorted(names_a or [])}")

    # A4/A5 documented-behavior skips.
    skip("A4 file:// scheme -> 'Disallowed URL scheme: file' (ssrf_guard "
         "scheme gate)", "connect-unreachable (A1 reasoning).")
    skip("A5 decimal-IP host http://2130706433/ -> blocked (WHATWG parser "
         "normalizes to 127.0.0.1 before the guard sees the host)",
         "connect-unreachable (A1 reasoning); behavior documented from "
         "url-crate WHATWG host parsing + ssrf_guard IP-literal path.")


# ── B: sync regression sweep ────────────────────────────────────────────────
def section_b():
    print("== B: 5 versions initialize echo + real public-IP call ==")
    for v in LEGACY_VERSIONS:
        st, echoed, _ = initialize("/mcp", v, client="r1800b")
        check(f"B1 {v} initialize echo EXACT on /mcp root",
              st == 200 and echoed == v, f"st={st} echo={echoed}")
    for v in ["2025-06-18"]:
        st, o, tries = resilient_ip_call(session_lane=True, version=v)
        ok = (st == 200 and o is not None
              and ((o.get("result") or {}).get("isError") is False)
              and valid_pub_ip(result_text(o)))
        check(f"B2 {v} REAL public-IP call (session lane, isError:false + "
              f"IPv4)", ok, f"st={st} tries={tries} {json_txt(o)[:140]}")
        warn(f"B2 {v}: tries={tries}")
    st, o, tries = resilient_ip_call(session_lane=False)
    r = (o or {}).get("result", {}) or {}
    check("B3 2026-07-28 stateless REAL public-IP call (resultType:"
          "complete + IPv4)",
          st == 200 and r.get("resultType") == "complete"
          and r.get("isError") is False and valid_pub_ip(result_text(o)),
          f"st={st} tries={tries} {json_txt(o)[:140]}")


# ── C: settings round-trip (NEW keys) ───────────────────────────────────────
def section_c():
    print("== C: settings round-trip (network.allowedCidrs / "
          "disablePasswordLogin) ==")
    cfg = cfg_read()
    cfg["network"] = {"allowedCidrs": ["10.0.0.0/8"]}
    cfg_write(cfg)
    back = (cfg_read().get("network") or {}).get("allowedCidrs")
    check("C1 network.allowedCidrs=['10.0.0.0/8'] round-trip identical",
          back == ["10.0.0.0/8"], f"back={back}")

    cfg = cfg_read()
    cfg["disablePasswordLogin"] = True
    cfg_write(cfg)
    back = cfg_read().get("disablePasswordLogin")
    check("C2a disablePasswordLogin=true persists", back is True, f"back={back}")
    cfg = cfg_read()
    cfg["disablePasswordLogin"] = False
    cfg_write(cfg)
    back = cfg_read().get("disablePasswordLogin")
    check("C2b disablePasswordLogin=false persists (clearable)", back is False,
          f"back={back}")

    cfg = cfg_read()
    cfg["network"] = {"allowedCidrs": ["10.0.0.0/33"]}
    cfg_write(cfg)
    back = (cfg_read().get("network") or {}).get("allowedCidrs")
    check("C3 invalid CIDR ['10.0.0.0/33'] stored verbatim (guard-side "
          "validation fails closed at read time)", back == ["10.0.0.0/33"],
          f"back={back}")
    cfg = cfg_read()
    cfg["network"] = {"allowedCidrs": []}
    cfg_write(cfg)


# ── D: apps_ui regression rerun ─────────────────────────────────────────────
def section_d():
    print("== D: rmcp_round_apps_ui.py regression rerun ==")
    p = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                     "rmcp_round_apps_ui.py")
    r = subprocess.run([sys.executable, p], capture_output=True, text=True,
                       timeout=420)
    out = (r.stdout or "") + (r.stderr or "")
    counts = None
    for line in out.splitlines():
        if line.startswith("PASS="):
            counts = line.strip()
            break
    m = re.search(r"PASS=(\d+) FAIL=(\d+) SKIP=(\d+)", counts or "")
    np_, nf, ns = (int(m.group(1)), int(m.group(2)), int(m.group(3))) if m \
        else (0, 1, 0)
    check("D1 apps_ui suite: FAIL==0", nf == 0, f"counts={counts}")
    check("D2 apps_ui suite: PASS>=7 (reachable contract cases all green)",
          np_ >= 7, f"counts={counts}")
    check("D3 apps_ui suite: SKIP<=2 (runtime-pickup skips only)", ns <= 2,
          f"counts={counts}")
    warn(f"D apps_ui summary: {counts}")
    if nf == 0:
        warn("D tail:\n" + "\n".join(out.strip().splitlines()[-4:]))


def main():
    print("=== rmcp_round_r1800: SSRF surface + settings round-trip + apps_ui rerun ===")
    # Normalize to campaign baseline FIRST, then snapshot the NORMALIZED
    # config (same pattern as r1700's force_campaign_baseline). This makes
    # the as-found snapshot-restore and the final baseline asserts
    # consistent: if the as-found config differed from the baseline, both
    # the restore and the asserts below refer to the same normalized state
    # instead of conflicting. As-found restore semantics are intentionally
    # NOT kept — the suite normalizes to the campaign baseline.
    cfg = cfg_read()
    cfg["strictValidation"] = False
    cfg.setdefault("mcp", {})["strictValidation"] = False
    cfg["exposeHttp"] = True
    cfg_write(cfg)
    time.sleep(0.6)
    snapshot = cfg_read()
    spec_srv = start_spec_mock()
    try:
        section_a()
        section_b()
        section_c()
        section_d()
    finally:
        # Restore the normalized baseline snapshot (see comment at snapshot
        # creation — suite normalizes to campaign baseline, not as-found).
        cfg_write(snapshot)
        time.sleep(0.6)
        now = cfg_read()
        check("RESTORE config snapshot identical",
              json.dumps(now, sort_keys=True, ensure_ascii=False)
              == json.dumps(snapshot, sort_keys=True, ensure_ascii=False),
              "diff — see suite log")
        cfg2 = cfg_read()
        check("RESTORE baseline strictValidation=false (top)",
              cfg2.get("strictValidation") is False,
              f"top={cfg2.get('strictValidation')}")
        check("RESTORE baseline mcp.strictValidation=false",
              (cfg2.get("mcp") or {}).get("strictValidation") is False,
              f"mcp={(cfg2.get('mcp') or {}).get('strictValidation')}")
        check("RESTORE baseline exposeHttp=true", cfg2.get("exposeHttp") is True,
              f"exposeHttp={cfg2.get('exposeHttp')}")
        remaining = delete_servers()
        check("RESTORE inserted server rows deleted (count==0)", remaining == 0,
              f"remaining={remaining}")
        try:
            st, data, _ = req("GET", "/health", timeout=20)
            check("RESTORE /health 200", st == 200, f"st={st}")
        except Exception as e:
            check("RESTORE /health 200", False, str(e))
        spec_srv.shutdown()

    print(f"\nPASS={base.PASS} FAIL={base.FAIL} SKIP={base.SKIPS}")
    if base.FAILURES:
        print("FAILURES:")
        for f in base.FAILURES:
            print("  -", f)
    sys.exit(0 if base.FAIL == 0 else 1)


if __name__ == "__main__":
    main()
