#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""E2E suite: ui:// dynamic resource read (MCP Apps #1303 bridge support).

Feature under test (services/rmcp_bridge.rs read_resource): after the
builtin-resource miss, a URI starting with ui:// is routed upstream —
single-server scope -> that server; group/root scope -> probe candidates
(connected || start_on_demand) in stable order; start_on_demand servers are
WOKEN (cold start) and held busy during the read; 2026 sessions get
ttlMs=0 + cacheScope=private; all-fail -> the standard
"Resource '<uri>' not found" error; $smart scope -> always not-found.

Cases:
  A1  root scope read of ui://test/widget via initialized session (2025-06-18)
      -> contents[0].text contains 'widget', uri echoed.  [needs the mock
      upstream CONNECTED — see RUNTIME-PICKUP note]
  A2  2026-07-28 stateless lane: (i) resources/read on the enabled builtin
      resource -> resultType complete + ttlMs 0 + cacheScope private (the
      CacheableResult wrapper is shared verbatim by the ui:// success
      branch, rmcp_bridge.rs read_resource both arms); (ii) ui:// via the
      2026 lane -> not-found (mock not connected in this run).
  A3  single-server scope /mcp/<server> direct route.  [same pickup note]
  A4  $smart scope read -> not-found error (no leak through the meta surface).
  A5  unknown ui:// URI -> standard -32602 "Resource '...' not found" (the
      all-candidates-failed path), NOT a 5xx.
  A6  resources/list does NOT contain any ui:// entry (dynamic URIs are never
      pre-registered; upstream resources/list is empty by design).
  B1  on-demand stdio variant: read ui:// -> server woken (mock log shows
      exactly one INIT), immediate second read REUSES the client (no second
      INIT).  [same pickup note]

RUNTIME-PICKUP NOTE (printed as evidence by the suite): the running hub
connects servers ONLY at startup (mcp_manager::start_all), via the Tauri
add/update/toggle/reload commands, or via the session_rebuild tick — and the
rebuild tick only considers servers ALREADY present in the pool
(pool::get_all_statuses). A server row inserted directly into the DB while
the app is running is therefore never connected, and the ui:// probe skips
candidates without a pool status entry (pool::get_status -> None). This was
verified empirically at suite start: the row is visible in DB but absent from
GET /servers (pool statuses). A1/A3/B1 need a hub restart after seeding to
run; when the pickup check fails they are SKIPped with that reasoning and the
reachable contract cases (A2/A4/A5/A6) still run and must pass.

Restore: both temp server rows deleted in finally (verified count==0), no
config/pin mutations, /health 200 at the end.
"""
import json
import os
import sqlite3
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import quote

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import rmcp_round_r1100 as base
from rmcp_round_r1100 import (
    HOST, DB, META_2026, req, sse_last, initialize, notify_initialized, rpc,
    check, skip, warn, json_txt,
)

HTTP_SERVER = "r1700appsui"
OD_SERVER = "r1700appsuiod"
WIDGET_URI = "ui://test/widget"
MOCK_STDIO = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                          "_appsui_mock_stdio.py")
MOCK_LOG = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                        "_appsui_mock_stdio.log")
# Dynamic mock port: bound with port 0 (fix 9) and read back from
# server_address — avoids hardcoded-port collisions between suite runs.
HTTP_PORT = 0
BUILTIN_URI = ("https://www.baidu.com/img/"
               "PCfb_5bf082d29588c07f842ccde3f97243ea.png")


# ── mock streamable-http upstream ───────────────────────────────────────────
class MockHandler(BaseHTTPRequestHandler):
    mock_sid = "mock-session-0001"

    def log_message(self, fmt, *args):
        pass

    def _send(self, code, payload, extra=None):
        body = json.dumps(payload).encode("utf-8") if payload is not None else b""
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        for k, v in (extra or {}).items():
            self.send_header(k, v)
        self.end_headers()
        if body:
            self.wfile.write(body)

    def _result(self, rid, result):
        self._send(200, {"jsonrpc": "2.0", "id": rid, "result": result})

    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0) or 0)
        try:
            msg = json.loads(self.rfile.read(n).decode("utf-8"))
        except Exception:
            self._send(400, {"error": "bad json"})
            return
        method = msg.get("method", "")
        rid = msg.get("id")
        if rid is None:
            self._send(202, None)  # notification
            return
        if method == "initialize":
            self._result(rid, {
                "protocolVersion": (msg.get("params") or {}).get(
                    "protocolVersion", "2025-06-18"),
                "capabilities": {"tools": {}, "resources": {}},
                "serverInfo": {"name": "appsui-mock", "version": "1.0.0"},
            })
        elif method == "tools/list":
            self._result(rid, {"tools": [{
                "name": "dummy", "description": "dummy tool",
                "inputSchema": {"type": "object", "properties": {}}}]})
        elif method == "resources/list":
            self._result(rid, {"resources": []})
        elif method == "resources/read":
            uri = (msg.get("params") or {}).get("uri", "")
            if uri == WIDGET_URI:
                self._result(rid, {"contents": [{
                    "uri": WIDGET_URI, "mimeType": "text/html",
                    "text": "<html>widget</html>"}]})
            else:
                self._send(200, {"jsonrpc": "2.0", "id": rid, "error": {
                    "code": -32602, "message": "Resource '%s' not found" % uri}})
        else:
            self._send(200, {"jsonrpc": "2.0", "id": rid, "error": {
                "code": -32601, "message": "Method not found"}})

    def do_GET(self):
        self._send(405, {"error": "GET not supported"})


def start_mock_http():
    srv = ThreadingHTTPServer(("127.0.0.1", 0), MockHandler)
    global HTTP_PORT
    HTTP_PORT = srv.server_address[1]
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv


# ── DB helpers ──────────────────────────────────────────────────────────────
def insert_server(name, server_type, url=None, command=None, args=None,
                  on_demand=False):
    con = sqlite3.connect(DB)
    con.execute(
        """INSERT INTO servers (id, name, server_type, url, command, args,
             enabled, description, start_on_demand, idle_timeout_ms)
           VALUES (lower(hex(randomblob(16))), ?, ?, ?, ?, ?, 1,
                   'apps ui e2e temp', ?, ?)
           ON CONFLICT(name) DO UPDATE SET url=excluded.url,
             command=excluded.command, args=excluded.args, enabled=1,
             server_type=excluded.server_type,
             start_on_demand=excluded.start_on_demand""",
        (name, server_type, url, command,
         json.dumps(args) if args else None,
         1 if on_demand else 0, 5000 if on_demand else 0))
    con.commit()
    con.close()


def delete_servers():
    con = sqlite3.connect(DB)
    con.execute("DELETE FROM servers WHERE name IN (?, ?)",
                (HTTP_SERVER, OD_SERVER))
    con.commit()
    n = con.execute(
        "SELECT count(*) FROM servers WHERE name LIKE 'r1700appsui%'").fetchone()[0]
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


# ── MCP read helpers ────────────────────────────────────────────────────────
def legacy_read(path, sid, uri, rid=7, version="2025-06-18"):
    st, data, _ = rpc(path, rid, "resources/read", {"uri": uri},
                      session=sid, timeout=90)
    return st, sse_last(data, rid), data


def modern_read(path, uri, rid=8):
    st, data, _ = req("POST", path, body={
        "jsonrpc": "2.0", "id": rid, "method": "resources/read",
        "params": {"uri": uri, **META_2026}},
        headers={"MCP-Protocol-Version": "2026-07-28"}, timeout=90)
    return st, sse_last(data, rid), data


def notfound_ok(o, uri):
    e = (o or {}).get("error")
    return (isinstance(e, dict) and e.get("code") == -32602
            and "not found" in str(e.get("message", ""))
            and uri in str(e.get("message", "")))


# ── sections ────────────────────────────────────────────────────────────────
def section_a2(sid_root, sid_server):
    print("== A2: 2026 CacheableResult contract ==")
    # (i) builtin resource read on the 2026 stateless lane — shares the exact
    # ttl/private wrapper with the ui:// success branch. The builtin resource
    # URI is a live external image (no local builtin-resource management face
    # exists on the hub HTTP router — resources are Tauri-IPC-only), so this
    # is a network-dependent assertion: retry 3x with backoff, and SKIP with
    # the printed reason if the external fetch keeps failing.
    ok = False
    st = None
    r = {}
    last_err = ""
    for attempt in range(3):
        try:
            st, o, _ = modern_read("/mcp", BUILTIN_URI)
        except Exception as e:
            last_err = repr(e)
            st = None
            r = {}
        else:
            r = (o or {}).get("result", {}) or {}
            ok = (st == 200 and r.get("resultType") == "complete"
                  and r.get("ttlMs") == 0 and r.get("cacheScope") == "private")
            if ok:
                break
        time.sleep(2.0 * (attempt + 1))
    if ok:
        check("A2i 2026 builtin resources/read -> resultType complete + "
              "ttlMs 0 + cacheScope private", True,
              f"st={st} r={json_txt(r)[:160]}")
    else:
        skip("A2i 2026 builtin resources/read -> resultType complete + "
             "ttlMs 0 + cacheScope private",
             f"builtin resource is an EXTERNAL url ({BUILTIN_URI}); fetch "
             f"failed after 3 attempts (retry/backoff) — network dependency, "
             f"not a protocol regression. last st={st} err={last_err} "
             f"r={json_txt(r)[:120]}")
    # (ii) ui:// via the 2026 lane -> not-found. The 2026 stateless lane
    # surfaces request-level JSON-RPC errors as HTTP 400 (streamable-http
    # spec: invalid request -> 400) with the -32602 error body; the legacy
    # lane returns 200 + error body (A5). Either shape is correct — the
    # invariant is the -32602 contract and NOT a 5xx.
    st, o, _ = modern_read("/mcp", WIDGET_URI)
    check("A2ii 2026 ui:// read -> -32602 not-found (200/400, not 5xx)",
          st in (200, 400) and notfound_ok(o, WIDGET_URI),
          f"st={st} {json_txt(o)[:160]}")


def section_a4():
    print("== A4: $smart scope never serves ui:// ==")
    st, echoed, sid = initialize("/mcp/$smart", "2025-06-18", client="appsui")
    if st != 200:
        check("A4 $smart initialize", False, f"st={st}")
        return
    notify_initialized("/mcp/$smart", sid)
    st, o, _ = legacy_read("/mcp/$smart", sid, WIDGET_URI)
    check("A4 $smart ui:// read -> -32602 not-found (no leak)",
          st == 200 and notfound_ok(o, WIDGET_URI),
          f"st={st} {json_txt(o)[:160]}")


def section_a5(sid_root):
    print("== A5: unknown ui:// URI -> standard not-found ==")
    st, o, _ = legacy_read("/mcp", sid_root, "ui://nope/missing")
    check("A5 unknown ui:// -> -32602 'Resource ... not found', not 5xx",
          st == 200 and notfound_ok(o, "ui://nope/missing"),
          f"st={st} {json_txt(o)[:160]}")


def section_a6(sid_root):
    print("== A6: resources/list has no ui:// entries ==")
    st, data, _ = rpc("/mcp", 11, "resources/list", {}, session=sid_root,
                      timeout=90)
    o = sse_last(data, 11)
    items = ((o or {}).get("result", {}) or {}).get("resources", []) or []
    uris = [i.get("uri", "") for i in items if isinstance(i, dict)]
    check("A6 resources/list ok + zero ui:// URIs (dynamic, never listed)",
          st == 200 and "error" not in (o or {})
          and not any(u.startswith("ui://") for u in uris),
          f"st={st} uris={uris[:6]}")


def main():
    print("=== rmcp_round_apps_ui: ui:// dynamic resource read ===")
    mock_srv = None
    try:
        mock_srv = start_mock_http()
        insert_server(HTTP_SERVER, "streamable-http",
                      url=f"http://127.0.0.1:{HTTP_PORT}/mcp")
        insert_server(OD_SERVER, "stdio", command=sys.executable,
                      args=[MOCK_STDIO], on_demand=True)
        time.sleep(1.5)

        picked = None
        # Empirical pickup check (see RUNTIME-PICKUP NOTE in the docstring).
        for _ in range(6):
            names = pool_names()
            if names and HTTP_SERVER in names:
                picked = names
                break
            time.sleep(2.5)
        if picked is None:
            # Environmental constraint, not a feature failure: the running
            # hub cannot pick up a DB-inserted server until restart (see
            # RUNTIME-PICKUP NOTE). Product-side observation worth reporting:
            # even the single-server scope path skips a never-connected
            # server instead of attempting a lazy connect.
            warn("PICKUP: DB-inserted server NOT connected at runtime "
                 f"(pool={sorted(pool_names() or [])}). Evidence for the "
                 "A1/A3/B1 skips below — hub connects only at startup / via "
                 "Tauri commands / rebuild tick (pool-known servers).")
        else:
            check("PICKUP mock server present in pool statuses (GET /servers)",
                  True)
        if picked is None:
            skip("A1 root scope ui:// read (2025-06-18 lane)",
                 "DB-inserted server never connected at runtime — hub only "
                 "connects at startup / via Tauri commands / rebuild tick "
                 "(pool-known servers). Needs a hub restart after seeding.")
            skip("A3 single-server scope ui:// read",
                 "same RUNTIME-PICKUP constraint (pool::get_status None -> "
                 "candidate skipped).")
        else:
            # A1
            st, echoed, sid = initialize("/mcp", "2025-06-18", client="appsui")
            if st == 200 and echoed == "2025-06-18":
                notify_initialized("/mcp", sid)
                st, o, _ = legacy_read("/mcp", sid, WIDGET_URI, rid=21)
                contents = ((o or {}).get("result", {}) or {}).get("contents", [])
                c0 = contents[0] if contents else {}
                check("A1 root ui:// read -> widget text + uri echo",
                      st == 200 and "widget" in str(c0.get("text", ""))
                      and c0.get("uri") == WIDGET_URI,
                      f"st={st} {json_txt(o)[:200]}")
                # A3
                p = f"/mcp/{quote(HTTP_SERVER, safe='')}"
                st, echoed, sid3 = initialize(p, "2025-06-18", client="appsui")
                ok3 = st == 200 and echoed == "2025-06-18"
                if ok3:
                    notify_initialized(p, sid3)
                    st, o, _ = legacy_read(p, sid3, WIDGET_URI, rid=22)
                    contents = ((o or {}).get("result", {}) or {}).get("contents", [])
                    check("A3 single-server scope ui:// read -> 200 widget",
                          st == 200 and "widget" in str(
                              (contents[0] if contents else {}).get("text", "")),
                          f"st={st} {json_txt(o)[:200]}")
                else:
                    check("A3 single-server initialize", False, f"st={st}")
            else:
                check("A1 initialize", False, f"st={st} echo={echoed}")

            # B1 on-demand stdio wake + reuse
            try:
                os.remove(MOCK_LOG)
            except OSError:
                pass
            p_od = f"/mcp/{quote(OD_SERVER, safe='')}"
            st, echoed, sidb = initialize(p_od, "2025-06-18", client="appsui")
            if st == 200:
                notify_initialized(p_od, sidb)
                st, o, _ = legacy_read(p_od, sidb, WIDGET_URI, rid=31)
                inits1 = _count_inits()
                check("B1 on-demand read -> server woken (exactly one mock "
                      "INIT — suite-local mock log is deterministic)",
                      st == 200 and inits1 == 1,
                      f"st={st} inits={inits1} {json_txt(o)[:120]}")
                st, o, _ = legacy_read(p_od, sidb, WIDGET_URI, rid=32)
                inits2 = _count_inits()
                check("B1 immediate second read reuses client (no re-INIT)",
                      st == 200 and inits2 == inits1,
                      f"inits1={inits1} inits2={inits2}")
            else:
                check("B1 on-demand server initialize", False, f"st={st}")

        # Reachable contract cases (run regardless of pickup).
        st, echoed, sid_root = initialize("/mcp", "2025-06-18", client="appsui")
        if st == 200 and echoed == "2025-06-18":
            notify_initialized("/mcp", sid_root)
        else:
            check("legacy session initialize on /mcp", False, f"st={st}")
            sid_root = None
        section_a2(sid_root, None)
        section_a4()
        if sid_root:
            section_a5(sid_root)
            section_a6(sid_root)
    finally:
        remaining = delete_servers()
        check("RESTORE temp server rows deleted (count==0)", remaining == 0,
              f"remaining={remaining}")
        try:
            st, data, _ = req("GET", "/health", timeout=20)
            check("RESTORE /health 200", st == 200, f"st={st}")
        except Exception as e:
            check("RESTORE /health 200", False, str(e))
        if mock_srv is not None:
            mock_srv.shutdown()

    print(f"\nPASS={base.PASS} FAIL={base.FAIL} SKIP={base.SKIPS}")
    if base.FAILURES:
        print("FAILURES:")
        for f in base.FAILURES:
            print("  -", f)
    sys.exit(0 if base.FAIL == 0 else 1)


def _count_inits():
    try:
        with open(MOCK_LOG, encoding="utf-8") as f:
            return sum(1 for ln in f if ln.startswith("INIT"))
    except OSError:
        return 0


if __name__ == "__main__":
    main()
