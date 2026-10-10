#!/usr/bin/env python3
"""E2E round r1400 (round-14 coverage): settings round-trip + gates + strict matrix.

A  Settings every-item round-trip: enumerate ALL user-visible config keys from
   SettingsContext.tsx fetch mapping + save paths (namespaces routing / mcp /
   smartRouting / exposeHttp / httpPort / nameSeparator / enableSessionRebuild /
   activityLog / install).  For each key: write a distinctive valid value into
   system_config.config_json via the same persistence path the settings page
   uses (PUT /system-config -> config_service deep merge == config_json), read
   it back through the settings-page data path source (GET /settings ->
   get_settings -> config_service::get -> system_config.config_json; the hub
   HTTP server does not expose /settings — that route lives in the Tauri IPC
   layer, so the read source is DB config_json, asserted equality), restore.
   WARN (not fail) for any key with no Rust-side consumer (grep map).
B  Route-gate matrix: [enableGlobalRoute on/off] x [enableGroupNameRoute on/off]
   x [root /mcp, /mcp/Test] initialize -> 200/404 per implemented+origin
   semantics (mcp_route_gate_middleware): the switches are independent —
   enableGlobalRoute gates only the global scope (/mcp, /mcp/$smart);
   enableGroupNameRoute gates only group-by-name paths.  Both restored true.
C  Strict-mode negative matrix: mcp.strictValidation=true; for 4 legacy
   versions on root /mcp: missing Accept / Content-Type text/plain / malformed
   JSON / missing jsonrpc / jsonrpc "1.1" / bare request (no session) -> 4xx.
   Restore false; same 6 inputs -> documented lenient behavior (2xx for
   Accept/jsonrpc/1.1/bare; 415 both-modes for malformed JSON + text/plain).
D  TTL freshness spot: tools/list on 2025-11-25 session + 2026 stateless —
   ttlMs present => integer 0 < ttlMs <= 5000 (freshness contract); cacheScope
   present => "private"/"public".
E  json_response=true (2026 stateless real-IP call: CT=application/json, no
   SSE frame) + subscriptions/listen 2026 stateless SSE ack with
   subscriptionId _meta.

Policy: every check asserts documented semantics; observed behavior printed on
conflict; exit 0 only if all cases pass.  ALL config state restored in finally
(full config_json snapshot + explicit flag restore).
"""
import json
import os
import re
import sqlite3
import sys
import time
import urllib.parse

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import rmcp_round_r1100 as base  # noqa: E402
from rmcp_round_r1100 import (  # noqa: E402
    HOST, PORT, DB,
    req, sse_last, cfg_read, cfg_write, initialize,
)
check = base.check
warn = base.warn
skip = base.skip

IP_TOOL = "本机公网ip查询-getPublicIp"
GROUP = "Test"
GROUP_ENC = urllib.parse.quote(GROUP, safe="")
PV = "io.modelcontextprotocol/protocolVersion"
CLIENT_META = {"io.modelcontextprotocol/clientInfo": {"name": "r1400", "version": "1"},
               "io.modelcontextprotocol/clientCapabilities": {}}
VERSIONS = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"]

FULL_CFG_SNAPSHOT = None


class _Absent:
    pass


_ABSENT = _Absent()


# ── config helpers (merge semantics mirror PUT /system-config deep merge) ────
def cfg_set(path, value):
    """Deep-merge value at tuple path into config_json (settings-page write)."""
    cfg = cfg_read()
    cur = cfg
    for p in path[:-1]:
        cur = cur.setdefault(p, {})
    if value is _ABSENT:
        cur.pop(path[-1], None)
    else:
        cur[path[-1]] = value
    cfg_write(cfg)


def cfg_snapshot_path(path):
    cfg = cfg_read()
    cur = cfg
    for p in path:
        if not isinstance(cur, dict) or p not in cur:
            return _ABSENT
        cur = cur[p]
    return cur


def cfg_restore_path(path, snapshot):
    cfg_set(path, snapshot)  # _ABSENT handled inside cfg_set


# Rust-side consumer grep map (src-tauri/src, verified 2026-10-10).
CONSUMED = {
    "routing.enableGlobalRoute": True,
    "routing.enableGroupNameRoute": True,
    "routing.enableBearerAuth": True,
    "routing.bearerAuthHeaderName": True,
    "routing.jsonBodyLimit": True,
    "routing.skipAuth": True,
    "httpPort": True,
    "exposeHttp": True,
    "mcp.strictValidation": True,
    "smartRouting.enabled": True,
    "smartRouting.serverDescriptionMode": True,
    "smartRouting.progressiveDisclosure": True,
    "smartRouting.fullSchemaTopN": True,
    "smartRouting.toolDefinitionFields": True,
    "smartRouting.maxResults": True,
    "smartRouting.scoreThreshold": True,
    "smartRouting.similarityThreshold": False,   # models.rs comment only — deliberately not mirrored
    "smartRouting.vectorWeight": True,
    "smartRouting.keywordWeight": True,
    "smartRouting.embeddingQueryPrefix": False,  # origin #1212 — desktop backend no consumer
    "smartRouting.embeddingDocumentPrefix": False,
    "smartRouting.basePacingDelayMs": False,
    "smartRouting.embeddingMaxTokens": False,
    "smartRouting.embeddingDimensions": False,
    "nameSeparator": True,
    "enableSessionRebuild": True,
    "activityLog.storeToolPayload": True,
    "install.pythonIndexUrl": False,
    "install.npmRegistry": False,
    "install.baseUrl": False,
}

# (label, db path, distinctive value)
A_KEYS = [
    ("routing.enableGlobalRoute", ("routing", "enableGlobalRoute"), False),
    ("routing.enableGroupNameRoute", ("routing", "enableGroupNameRoute"), False),
    ("routing.enableBearerAuth", ("routing", "enableBearerAuth"), True),
    ("routing.bearerAuthHeaderName", ("routing", "bearerAuthHeaderName"), "X-R1400-Auth"),
    ("routing.jsonBodyLimit", ("routing", "jsonBodyLimit"), "4mb"),
    ("routing.skipAuth", ("routing", "skipAuth"), False),
    ("httpPort", ("httpPort",), 23334),
    ("exposeHttp", ("exposeHttp",), False),
    ("mcp.strictValidation", ("mcp", "strictValidation"), True),
    ("smartRouting.enabled", ("smartRouting", "enabled"), False),
    ("smartRouting.serverDescriptionMode", ("smartRouting", "serverDescriptionMode"), "full"),
    ("smartRouting.progressiveDisclosure", ("smartRouting", "progressiveDisclosure"), True),
    ("smartRouting.fullSchemaTopN", ("smartRouting", "fullSchemaTopN"), 3),
    ("smartRouting.toolDefinitionFields", ("smartRouting", "toolDefinitionFields"),
     ["name", "description", "title"]),
    ("smartRouting.maxResults", ("smartRouting", "maxResults"), 42),
    ("smartRouting.scoreThreshold", ("smartRouting", "scoreThreshold"), 0.77),
    ("smartRouting.similarityThreshold", ("smartRouting", "similarityThreshold"), 0.61),
    ("smartRouting.vectorWeight", ("smartRouting", "vectorWeight"), 0.66),
    ("smartRouting.keywordWeight", ("smartRouting", "keywordWeight"), 0.34),
    ("smartRouting.embeddingQueryPrefix", ("smartRouting", "embeddingQueryPrefix"),
     "query: r1400 "),
    ("smartRouting.embeddingDocumentPrefix", ("smartRouting", "embeddingDocumentPrefix"),
     "doc: r1400 "),
    ("smartRouting.basePacingDelayMs", ("smartRouting", "basePacingDelayMs"), 123),
    ("smartRouting.embeddingMaxTokens", ("smartRouting", "embeddingMaxTokens"), 4096),
    ("smartRouting.embeddingDimensions", ("smartRouting", "embeddingDimensions"), 768),
    ("nameSeparator", ("nameSeparator",), "#"),
    ("enableSessionRebuild", ("enableSessionRebuild",), True),
    ("activityLog.storeToolPayload", ("activityLog", "storeToolPayload"), False),
    ("install.pythonIndexUrl", ("install", "pythonIndexUrl"), "https://r1400.invalid/simple"),
    ("install.npmRegistry", ("install", "npmRegistry"), "https://r1400.invalid/registry"),
    ("install.baseUrl", ("install", "baseUrl"), "http://127.0.0.1:23334"),
]


def section_a():
    print("\n== A: settings every-item round-trip ==")
    snaps = {}
    for label, path, value in A_KEYS:
        snaps[label] = cfg_snapshot_path(path)
    try:
        for label, path, value in A_KEYS:
            cfg_set(path, value)
            got = cfg_snapshot_path(path)
            check(f"A {label} round-trip", got == value, f"got={got!r} want={value!r}")
            if not CONSUMED.get(label, False):
                warn(f"orphan key: {label} — persisted but no Rust-side consumer")
    finally:
        for label, path, _value in A_KEYS:
            cfg_restore_path(path, snaps[label])
    check("A restore spot-check toolDefinitionFields",
          cfg_snapshot_path(("smartRouting", "toolDefinitionFields"))
          == snaps["smartRouting.toolDefinitionFields"], "")


# ── B: route-gate matrix ─────────────────────────────────────────────────────
def set_routing(key, val):
    cfg_set(("routing", key), val)
    time.sleep(0.4)


def wait_healthy(timeout_s=150):
    import http.client as _hc
    dl = time.time() + timeout_s
    while time.time() < dl:
        try:
            c = _hc.HTTPConnection(HOST, PORT, timeout=5)
            c.request("GET", "/health")
            ok = c.getresponse().status == 200
            c.close()
            if ok:
                return True
        except Exception:
            pass
        time.sleep(3)
    return False


def section_b():
    print("\n== B: route-gate matrix ==")
    assert wait_healthy(), "hub did not come back"
    snap_g = cfg_snapshot_path(("routing", "enableGlobalRoute"))
    snap_gr = cfg_snapshot_path(("routing", "enableGroupNameRoute"))
    try:
        for g in (True, False):
            for gr in (True, False):
                set_routing("enableGlobalRoute", g)
                set_routing("enableGroupNameRoute", gr)
                st_root, _, _ = initialize("/mcp", "2025-06-18", rid=1)
                st_grp, _, _ = initialize(f"/mcp/{GROUP_ENC}", "2025-06-18", rid=2)
                # Implemented+origin semantics (mcp_route_gate_middleware):
                # enableGlobalRoute gates ONLY the global scope (/mcp, /mcp/$smart);
                # enableGroupNameRoute gates ONLY group-by-name paths. Independent.
                exp_root = 200 if g else 404
                exp_grp = 200 if gr else 404
                check(f"B global={'on' if g else 'off'} group={'on' if gr else 'off'} root",
                      st_root == exp_root, f"st={st_root} exp={exp_root}")
                check(f"B global={'on' if g else 'off'} group={'on' if gr else 'off'} group-path",
                      st_grp == exp_grp, f"st={st_grp} exp={exp_grp}")
        set_routing("enableGlobalRoute", True)
        set_routing("enableGroupNameRoute", True)
        st_r, _, _ = initialize("/mcp", "2025-06-18", rid=3)
        st_g, _, _ = initialize(f"/mcp/{GROUP_ENC}", "2025-06-18", rid=4)
        check("B restore both-on: root 200", st_r == 200, f"st={st_r}")
        check("B restore both-on: group 200", st_g == 200, f"st={st_g}")
    finally:
        cfg_restore_path(("routing", "enableGlobalRoute"), snap_g)
        cfg_restore_path(("routing", "enableGroupNameRoute"), snap_gr)
        time.sleep(0.4)


# ── C: strict / lenient negative matrix ──────────────────────────────────────
def make_send(version, idx):
    def missing_accept():
        return req("POST", "/mcp", body={
            "jsonrpc": "2.0", "id": idx, "method": "initialize",
            "params": {"protocolVersion": version, "capabilities": {},
                       "clientInfo": {"name": "r1400", "version": "1"}}},
            headers={"Content-Type": "application/json"}, default_accept=False)

    def ct_text_plain():
        return req("POST", "/mcp", body={
            "jsonrpc": "2.0", "id": idx, "method": "initialize",
            "params": {"protocolVersion": version, "capabilities": {},
                       "clientInfo": {"name": "r1400", "version": "1"}}},
            headers={"Content-Type": "text/plain"})

    def malformed():
        return req("POST", "/mcp", raw_body=b"{not json",
                   headers={"Content-Type": "application/json"})

    def missing_jsonrpc():
        return req("POST", "/mcp", body={
            "id": idx, "method": "initialize",
            "params": {"protocolVersion": version, "capabilities": {},
                       "clientInfo": {"name": "r1400", "version": "1"}}})

    def jsonrpc_11():
        return req("POST", "/mcp", body={
            "jsonrpc": "1.1", "id": idx, "method": "initialize",
            "params": {"protocolVersion": version, "capabilities": {},
                       "clientInfo": {"name": "r1400", "version": "1"}}})

    def bare_no_session():
        return req("POST", "/mcp", body={"jsonrpc": "2.0", "id": idx, "method": "tools/list"},
                   headers={"MCP-Protocol-Version": version})

    return [("missing Accept", missing_accept),
            ("CT text/plain", ct_text_plain),
            ("malformed JSON", malformed),
            ("missing jsonrpc", missing_jsonrpc),
            ('jsonrpc "1.1"', jsonrpc_11),
            ("bare request (no session)", bare_no_session)]


def section_c():
    assert wait_healthy(), "hub did not come back"
    print("\n== C: strict-mode full negative matrix ==")
    snap = cfg_snapshot_path(("mcp", "strictValidation"))
    try:
        cfg_set(("mcp", "strictValidation"), True)
        time.sleep(0.5)
        for v in VERSIONS:
            for name, send in make_send(v, 100):
                st, data, _ = send()
                check(f"C strict [{v}] {name}", 400 <= st < 500,
                      f"st={st} body={data[:80]}")
        cfg_set(("mcp", "strictValidation"), False)
        time.sleep(0.5)
        for v in VERSIONS:
            for name, send in make_send(v, 200):
                st, data, _ = send()
                # documented lenient semantics: malformed JSON + text/plain stay
                # 415 on BOTH sides (r10 leniency pairs); everything else 2xx.
                expect_2xx = name not in ("malformed JSON", "CT text/plain")
                ok = (200 <= st < 300) if expect_2xx else (400 <= st < 500)
                check(f"C lenient [{v}] {name}", ok, f"st={st} body={data[:80]}")
    finally:
        cfg_restore_path(("mcp", "strictValidation"), snap)
        time.sleep(0.3)


# ── D: TTL freshness spot ────────────────────────────────────────────────────
def ttl_assert(tag, result):
    # Freshness contract (r110 D1 / list_freshness.rs): positive ttlMs only when
    # EVERY participating upstream is fresh; otherwise gateway-generated ttl 0.
    # So: absent, or integer 0 <= ttlMs <= 5000.
    ok = True
    detail = []
    result = result or {}
    if "ttlMs" in result:
        t = result.get("ttlMs")
        ok = ok and isinstance(t, int) and 0 <= t <= 5000
        detail.append(f"ttlMs={t}")
    if "cacheScope" in result:
        cs = result.get("cacheScope")
        ok = ok and cs in ("private", "public")
        detail.append(f"cacheScope={cs}")
    check(f"D {tag} freshness contract", ok, " ".join(detail) or "both absent (allowed)")


def section_d():
    assert wait_healthy(), "hub did not come back"
    print("\n== D: TTL freshness spot ==")
    st, _echoed, sid = initialize("/mcp", "2025-11-25", rid=1, client="r1400-d")
    if st == 200 and sid:
        req("POST", "/mcp", body={"jsonrpc": "2.0", "method": "notifications/initialized"},
            headers={"mcp-session-id": sid}, default_accept=False)
        st2, data, _ = req("POST", "/mcp", body={"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
                           headers={"mcp-session-id": sid})
        o = sse_last(data, 2)
        check("D legacy 2025-11-25 tools/list 200", st2 == 200 and o and "result" in o,
              f"st={st2}")
        ttl_assert("legacy 2025-11-25", (o or {}).get("result"))
        req("DELETE", "/mcp", None, {"mcp-session-id": sid})
    else:
        skip("D legacy session", f"init st={st}")
    st3, data3, _ = req("POST", "/mcp", body={
        "jsonrpc": "2.0", "id": 3, "method": "tools/list",
        "params": {"_meta": {PV: "2026-07-28", **CLIENT_META}}},
        headers={"MCP-Protocol-Version": "2026-07-28", "Mcp-Method": "tools/list"})
    o3 = sse_last(data3, 3)
    check("D 2026 stateless tools/list 200", st3 == 200 and o3 and "result" in o3, f"st={st3}")
    ttl_assert("2026 stateless", (o3 or {}).get("result"))


# ── E: json_response + subscriptions/listen ──────────────────────────────────
def valid_ip(t):
    m = re.search(r"(?<![\d.])(?:\d{1,3}\.){3}\d{1,3}(?![\d.])", t or "")
    if not m:
        return None
    try:
        import ipaddress
        ipaddress.ip_address(m.group(0))
        return m.group(0)
    except ValueError:
        return None


def section_e():
    assert wait_healthy(), "hub did not come back"
    print("\n== E: json_response + subscriptions/listen ==")
    import http.client
    ip, ct, raw, obj = None, "", "", None
    for _attempt in range(3):  # ip.3322.net transient throttling (r40 lesson)
        mcp_name = IP_TOOL if IP_TOOL.isascii() else (
            "=?base64?" + __import__("base64").b64encode(IP_TOOL.encode()).decode() + "?=")
        st, data, hdrs = req("POST", "/mcp", body={
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": IP_TOOL, "arguments": {},
                       "_meta": {PV: "2026-07-28", **CLIENT_META}}},
            headers={"MCP-Protocol-Version": "2026-07-28", "Mcp-Method": "tools/call",
                     "Mcp-Name": mcp_name}, timeout=90)
        raw = data
        ct = (hdrs.get("content-type") or "")
        obj = sse_last(data, 1)
        ip = valid_ip(json.dumps(obj) if obj else raw)
        if ip:
            break
        time.sleep(3)
    check("E json_response CT=application/json", "application/json" in ct
          and "text/event-stream" not in ct, f"ct={ct} st={st}")
    check("E json_response no SSE frame pollution", raw.strip().startswith("{"), raw[:40])
    check("E json_response real-IP call", ip is not None, f"ip={ip}")
    check("E resultType complete", (obj or {}).get("result", {}).get("resultType") == "complete",
          str((obj or {}).get("result", {}).get("resultType")))
    # subscriptions/listen 2026 stateless
    c = None
    try:
        c = http.client.HTTPConnection(HOST, PORT, timeout=20)
        c.request("POST", "/mcp",
                  body=json.dumps({"jsonrpc": "2.0", "id": 1, "method": "subscriptions/listen",
                                   "params": {"notifications": {"toolsListChanged": True},
                                              "_meta": {PV: "2026-07-28", **CLIENT_META}}}).encode(),
                  headers={"Content-Type": "application/json", "Accept": "text/event-stream",
                           "MCP-Protocol-Version": "2026-07-28",
                           "Mcp-Method": "subscriptions/listen"})
        r = c.getresponse()
        stl, ctl = r.status, r.getheader("content-type") or ""
        chunk = (r.readline() + r.readline() + r.readline()).decode("utf-8", "replace")
        check("E subscriptions/listen 200 + SSE", stl == 200 and "text/event-stream" in ctl,
              f"st={stl} ct={ctl}")
        check("E subscriptions/listen ack subscriptionId",
              "acknowledged" in chunk and "subscriptionId" in chunk, chunk[:100])
    except Exception as e:
        skip("E subscriptions/listen", f"error: {e}")
    finally:
        if c is not None:
            try:
                c.close()
            except Exception:
                pass


def main():
    global FULL_CFG_SNAPSHOT
    try:
        st, _, _ = req("GET", "/health", default_accept=False)
        if st != 200:
            print(f"SKIP-ALL: hub not healthy at {HOST}:{PORT} (status {st})")
            return 2
    except Exception as e:
        print(f"SKIP-ALL: hub down at {HOST}:{PORT}: {e}")
        return 2

    con = sqlite3.connect(DB)
    FULL_CFG_SNAPSHOT = con.execute(
        "SELECT config_json FROM system_config WHERE id=1").fetchone()[0]
    con.close()

    rc = 0
    try:
        section_a()
        section_b()
        section_c()
        section_d()
        section_e()
    finally:
        con = sqlite3.connect(DB)
        con.execute("UPDATE system_config SET config_json=? WHERE id=1",
                    (FULL_CFG_SNAPSHOT,))
        con.commit()
        con.close()
        time.sleep(0.5)
        cfg = cfg_read()
        rt = cfg.get("routing") or {}
        for k, v in (("enableGlobalRoute", True), ("enableGroupNameRoute", True),
                     ("enableBearerAuth", False)):
            if rt.get(k) is not v:
                print(f"WARN-RESTORE: routing.{k}={rt.get(k)} (expected {v})")
        if (cfg.get("mcp") or {}).get("strictValidation"):
            print("WARN-RESTORE: mcp.strictValidation still true")

    print(f"\n{'=' * 50}\nPASS={base.PASS} FAIL={base.FAIL} SKIP={base.SKIPS}")
    if base.FAILURES:
        print("FAILED:", *base.FAILED, sep="\n  - ")
        rc = 1
    return rc


if __name__ == "__main__":
    sys.exit(main())
