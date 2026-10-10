#!/usr/bin/env python3
"""E2E round r1300 (round-13 coverage): edge/chaos + protocol depth.

A  Unicode/CJK names across channels: server-scope /mcp/{IP-server-encoded}
   (2025-06-18, 2025-11-25 legacy + 2026-07-28 stateless) with REAL public-IP
   calls; temp CJK group "Test-中文-13" created via DB insert, group channel
   /mcp/Test-中文-13 encoded verified (tools/list + real legacy + 2026
   stateless call), group row DELETEd in finally.
B  JSON-RPC id edge cases on root /mcp (2025-06-18 session): string id
   round-trip; float id 1.5 (202 via GET SSE stream per prior behavior —
   assert no 5xx); very large integer id; negative id; null id (notification
   semantics, no 5xx); duplicate same-id sequential requests both answered.
C  Session lifecycle edge: initialize twice on same session; DELETE session
   then reuse -> 4xx; notifications/cancelled for unknown request -> 2xx
   no-op; batch JSON-RPC array -> clean non-5xx (documented), no hang.
D  Tasks lifecycle spot matrix: task-directed creation on 2025-11-25 and
   2025-03-26 (print observed — prior suites documented rmcp ext-only design
   -> -32021); 2026 client WITHOUT extensions declaration -> synchronous
   result; 2026 WITH ext -> CreateTaskResult flat shape -> tasks/get poll to
   terminal -> tasks/result; tasks/update unknown id -> -32602; terminal
   tasks/cancel -> -32602.
E  Prompts/resources per version on root + $smart scope: prompts/list +
   prompts/get (real render, no {{placeholder}} leakage) on 2025-03-26 /
   2025-11-25; resources/list both versions; resources/read (no 5xx,
   document); $smart scope prompts/resources -> empty per contract.
F  Leniency chaos: Accept text/event-stream only; Content-Type with
   charset=utf-8; body with trailing newline / CRLF; body with BOM (clean
   4xx or accept — documented); whitespace-only body -> 4xx.

Policy: every check asserts documented semantics; on conflict between suite
expectation and server, the observed behavior is PRINTED and the case fails
only on 5xx / hang / leak.  Exit 0 only if all cases pass.  All DB state
restored in finally.
"""
import json
import os
import sqlite3
import sys
import time
import urllib.parse

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import rmcp_round_r1100 as base  # noqa: E402
from rmcp_round_r1100 import (  # noqa: E402
    HOST, PORT, DB,
    req, sse_last, json_txt, rpc, initialize, notify_initialized,
    result_text, tool_error, find_ip_tool, valid_pub_ip, IP_SERVER, IP_TOOL,
    META_2026,
)
check = base.check
skip = base.skip

IP_Q = urllib.parse.quote(IP_SERVER, safe="")
TMP_GROUP = "Test-中文-13"
GRP_Q = urllib.parse.quote(TMP_GROUP, safe="")
PROMPT_NAME = "测试提示词Test1"
RESOURCE_URI = "https://www.baidu.com/img/PCfb_5bf082d29588c07f842ccde3f97243ea.png"
META_2026_TASKS = {
    "_meta": {
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {"name": "r1300", "version": "1"},
        "io.modelcontextprotocol/clientCapabilities": {
            "extensions": {"io.modelcontextprotocol/tasks": {}}},
    }
}
META_2026_NOEXT = {
    "_meta": {
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {"name": "r1300", "version": "1"},
        "io.modelcontextprotocol/clientCapabilities": {},
    }
}
H26_CALL = {"MCP-Protocol-Version": "2026-07-28",
            "Mcp-Method": "tools/call", "Mcp-Name": IP_TOOL}
H26_GET = {"MCP-Protocol-Version": "2026-07-28", "Mcp-Method": "tasks/get"}
H26_RESULT = {"MCP-Protocol-Version": "2026-07-28", "Mcp-Method": "tasks/result"}
H26_UPDATE = {"MCP-Protocol-Version": "2026-07-28", "Mcp-Method": "tasks/update"}
H26_CANCEL = {"MCP-Protocol-Version": "2026-07-28", "Mcp-Method": "tasks/cancel"}


def post(path, body, headers=None, timeout=90, raw_body=None, ct="application/json"):
    h = {"Accept": "application/json, text/event-stream"}
    if headers:
        h.update(headers)
    if raw_body is not None:
        return req("POST", path, raw_body=raw_body, headers=h, timeout=timeout)
    return req("POST", path, body=body, headers=h, timeout=timeout)


def call_legacy(path, sid, rid, name, version="2025-06-18"):
    st, data, _ = rpc(path, rid, "tools/call", {"name": name, "arguments": {}},
                      session=sid, timeout=120)
    return st, sse_last(data, rid)


def group_row():
    con = sqlite3.connect(DB)
    row = con.execute("SELECT id, servers FROM groups WHERE name=?",
                      (TMP_GROUP,)).fetchone()
    con.close()
    return row


def create_tmp_group():
    con = sqlite3.connect(DB)
    member = json.dumps([{"name": IP_SERVER, "tools": "all",
                          "prompts": "all", "resources": "all"}],
                        ensure_ascii=False)
    con.execute("DELETE FROM groups WHERE name=?", (TMP_GROUP,))
    con.execute(
        "INSERT INTO groups (id, name, description, servers) "
        "VALUES (lower(hex(randomblob(16))), ?, 'r1300 temp', ?)",
        (TMP_GROUP, member))
    con.commit()
    con.close()
    time.sleep(0.6)


def drop_tmp_group():
    con = sqlite3.connect(DB)
    con.execute("DELETE FROM groups WHERE name=?", (TMP_GROUP,))
    con.commit()
    con.close()
    time.sleep(0.4)


# ── A: Unicode/CJK across channels ──────────────────────────────────────────
def section_a():
    print("== A: unicode/CJK server + group channels ==")
    for v in ("2025-06-18", "2025-11-25"):
        st, echoed, sid = initialize(f"/mcp/{IP_Q}", v, rid=1, client="r1300")
        check(f"A server-scope CJK initialize echo {v}",
              st == 200 and echoed == v, f"st={st} echo={echoed}")
        if st == 200:
            notify_initialized(f"/mcp/{IP_Q}", sid)
            st, o = call_legacy(f"/mcp/{IP_Q}", sid, 2, IP_TOOL, v)
            check(f"A server-scope CJK real public-IP call {v}",
                  st == 200 and tool_error(o) is None
                  and o.get("result", {}).get("isError") is False
                  and valid_pub_ip(result_text(o)),
                  f"st={st} {json_txt(o)[:140]}")
    st, data, _ = post(f"/mcp/{IP_Q}", {
        "jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": {"name": IP_TOOL, "arguments": {}, **META_2026}},
        headers={"MCP-Protocol-Version": "2026-07-28"})
    o = sse_last(data, 3)
    r = (o or {}).get("result", {}) or {}
    check("A server-scope CJK 2026 stateless real public-IP call",
          st == 200 and r.get("resultType") == "complete"
          and r.get("isError") is False and valid_pub_ip(result_text(o)),
          f"st={st} {data[:140]}")

    # Temp CJK group channel
    create_tmp_group()
    gpath = f"/mcp/{GRP_Q}"
    group_ip_name = None
    st, echoed, sid = initialize(gpath, "2025-06-18", rid=1, client="r1300")
    check("A CJK group initialize echo 2025-06-18",
          st == 200 and echoed == "2025-06-18", f"st={st} echo={echoed}")
    if st == 200:
        notify_initialized(gpath, sid)
        st, data, _ = rpc(gpath, 2, "tools/list", {}, session=sid, timeout=90)
        o = sse_last(data, 2)
        names = [t.get("name", "")
                 for t in (((o or {}).get("result", {}) or {}).get("tools", []) or [])]
        ip_name = find_ip_tool(names)
        group_ip_name = ip_name
        check("A CJK group tools/list exposes IP tool",
              st == 200 and ip_name is not None, f"st={st} names={names[:8]}")
        if ip_name:
            st, o = call_legacy(gpath, sid, 3, ip_name)
            check("A CJK group real public-IP call (legacy)",
                  st == 200 and tool_error(o) is None and valid_pub_ip(result_text(o)),
                  f"st={st} {json_txt(o)[:140]}")
    gname = group_ip_name or f"{IP_SERVER}-{IP_TOOL}"
    st, data, _ = post(gpath, {
        "jsonrpc": "2.0", "id": 4, "method": "tools/call",
        "params": {"name": gname, "arguments": {}, **META_2026}},
        headers={"MCP-Protocol-Version": "2026-07-28"})
    o = sse_last(data, 4)
    r = (o or {}).get("result", {}) or {}
    check("A CJK group 2026 stateless real public-IP call",
          st == 200 and r.get("resultType") == "complete"
          and valid_pub_ip(result_text(o)), f"st={st} {data[:140]}")


# ── B: JSON-RPC id edge cases (root /mcp, 2025-06-18 session) ───────────────
def section_b():
    print("== B: JSON-RPC id edge cases ==")
    path = "/mcp"
    st, echoed, sid = initialize(path, "2025-06-18", rid=1, client="r1300")
    if st != 200 or not sid:
        check("B initialize for id-edge session", False, f"st={st} sid={sid!r}")
        return
    notify_initialized(path, sid)
    H = {"mcp-session-id": sid, "MCP-Protocol-Version": "2025-06-18"}

    st, data, _ = post(path, {"jsonrpc": "2.0", "id": "str-id-13",
                              "method": "ping", "params": {}}, headers=H)
    o = sse_last(data, "str-id-13")
    check("B string id round-trip", st == 200 and o is not None
          and o.get("id") == "str-id-13" and "result" in o,
          f"st={st} {data[:120]}")

    # Float id: per round50f documented behavior -> 202 (response goes to the
    # GET SSE stream); accept 200 inline or 202 — only 5xx/hang fails.
    st, data, _ = post(path, {"jsonrpc": "2.0", "id": 1.5,
                              "method": "ping", "params": {}}, headers=H)
    print(f"      [observed] float id 1.5 -> status {st}")
    check("B float id 1.5 -> non-5xx (200 inline or 202 SSE-streamed)",
          st in (200, 202), f"st={st} {data[:120]}")

    big = 9007199254740993
    st, data, _ = post(path, {"jsonrpc": "2.0", "id": big,
                              "method": "ping", "params": {}}, headers=H)
    o = sse_last(data, big)
    check("B very large integer id round-trip", st == 200 and o is not None
          and o.get("id") == big and "result" in o, f"st={st} {data[:120]}")

    st, data, _ = post(path, {"jsonrpc": "2.0", "id": -42,
                              "method": "ping", "params": {}}, headers=H)
    o = sse_last(data, -42)
    check("B negative id round-trip", st == 200 and o is not None
          and o.get("id") == -42 and "result" in o, f"st={st} {data[:120]}")

    # null id: rmcp deserializes as notification -> no response body (202 or
    # empty 200).  Not 5xx, does not hang.
    st, data, _ = post(path, {"jsonrpc": "2.0", "id": None,
                              "method": "ping", "params": {}}, headers=H)
    print(f"      [observed] null id ping -> status {st} body[:80]={data[:80]!r}")
    check("B null id -> no response / notification semantics, non-5xx",
          st in (200, 202, 204), f"st={st} {data[:120]}")

    # Duplicate same-id sequential requests: both must be answered.
    ok1 = ok2 = False
    for k in (1, 2):
        st, data, _ = post(path, {"jsonrpc": "2.0", "id": 777,
                                  "method": "ping", "params": {}}, headers=H)
        o = sse_last(data, 777)
        if st == 200 and o is not None and o.get("id") == 777 and "result" in o:
            if k == 1:
                ok1 = True
            else:
                ok2 = True
    check("B duplicate same-id sequential requests both answered", ok1 and ok2)


# ── C: session lifecycle edge ───────────────────────────────────────────────
def section_c():
    print("== C: session lifecycle edge ==")
    path = "/mcp"
    st, echoed, sid = initialize(path, "2025-06-18", rid=1, client="r1300")
    check("C initialize for lifecycle session", st == 200 and bool(sid),
          f"st={st}")
    if st != 200 or not sid:
        return
    notify_initialized(path, sid)
    H = {"mcp-session-id": sid, "MCP-Protocol-Version": "2025-06-18"}

    # Second initialize on same session (round50f documented: rmcp already-
    # initialized error or reset — non-5xx only).
    st, data, _ = post(path, {"jsonrpc": "2.0", "id": 2, "method": "initialize",
                              "params": {"protocolVersion": "2025-06-18",
                                         "capabilities": {},
                                         "clientInfo": {"name": "r1300", "version": "1"}}},
                       headers=H)
    print(f"      [observed] 2nd initialize on live session -> {st} "
          f"{data[:100]!r}")
    check("C1 second initialize same session -> non-5xx", st < 500,
          f"st={st} {data[:120]}")
    st, data, _ = post(path, {"jsonrpc": "2.0", "id": 3, "method": "ping",
                              "params": {}}, headers=H)
    print(f"      [observed] ping after 2nd initialize -> {st}")
    check("C2 session usable-or-clearly-invalid after 2nd initialize -> non-5xx",
          st < 500, f"st={st} {data[:120]}")

    # DELETE then reuse -> 4xx
    st, _, _ = req("DELETE", path, headers=H, default_accept=False)
    check("C3 DELETE session -> 2xx", 200 <= st < 300, f"st={st}")
    st, data, _ = post(path, {"jsonrpc": "2.0", "id": 4, "method": "ping",
                              "params": {}}, headers=H)
    check("C4 reuse deleted session -> 4xx", 400 <= st < 500, f"st={st} {data[:100]}")

    # notifications/cancelled for unknown request -> 2xx no-op
    st, echoed2, sid2 = initialize(path, "2025-06-18", rid=1, client="r1300")
    if st == 200 and sid2:
        st, data, _ = post(path, {"jsonrpc": "2.0",
                                  "method": "notifications/cancelled",
                                  "params": {"requestId": "no-such-req"}},
                           headers={"mcp-session-id": sid2})
        check("C5 notifications/cancelled unknown request -> 2xx no-op",
              200 <= st < 300, f"st={st} {data[:120]}")

    # Batch JSON-RPC array -> clean non-5xx, no hang
    st, data, _ = post(path, [{"jsonrpc": "2.0", "id": 10,
                               "method": "ping", "params": {}}],
                       headers={"MCP-Protocol-Version": "2025-06-18"})
    print(f"      [observed] batch array -> {st} {data[:90]!r}")
    check("C6 batch JSON-RPC array -> clean non-5xx, no hang", st < 500,
          f"st={st} {data[:120]}")


# ── D: tasks lifecycle spot matrix ──────────────────────────────────────────
def section_d():
    print("== D: tasks lifecycle spot matrix ==")
    # D1/D2 legacy task-directed creation.  Prior suites (supplement_v19)
    # documented rmcp ext-only design: legacy core capabilities.tasks is NOT
    # recognized -> -32021.  Print the observed behavior; pass unless 5xx.
    for v in ("2025-11-25", "2025-03-26"):
        st, data, h = req("POST", "/mcp", body={
            "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": v, "capabilities": {"tasks": {}},
                "clientInfo": {"name": "r1300", "version": "1"}}})
        sid = h.get("mcp-session-id", "")
        if st != 200 or not sid:
            check(f"D legacy {v} initialize", False, f"st={st}")
            continue
        notify_initialized("/mcp", sid)
        H = {"mcp-session-id": sid, "MCP-Protocol-Version": v}
        st, data, _ = post("/mcp", {"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                                    "params": {"name": IP_TOOL, "arguments": {},
                                               "task": {"ttlMs": 60000}}}, headers=H)
        o = sse_last(data, 2)
        err = (o or {}).get("error") or {}
        res = (o or {}).get("result") or {}
        print(f"      [observed] legacy {v} task-directed creation -> "
              f"error={err.get('code')} resultType={res.get('resultType')}")
        check(f"D1 legacy {v} task-directed creation -> clean JSON-RPC outcome "
              "(-32021 ext-only documented or CreateTaskResult)",
              st == 200 and (err.get("code") == -32021
                             or res.get("resultType") == "task"),
              f"st={st} {json_txt(o)[:140]}")

    # D3 2026 WITHOUT extensions declaration -> synchronous result (never a
    # task for a client that did not declare support).
    st, data, _ = post("/mcp", {"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                                "params": {"name": IP_TOOL, "arguments": {},
                                           **META_2026_NOEXT}},
                       headers={"MCP-Protocol-Version": "2026-07-28",
                                "Mcp-Method": "tools/call", "Mcp-Name": IP_TOOL})
    o = sse_last(data, 3)
    r = (o or {}).get("result", {}) or {}
    check("D3 2026 client without tasks extension -> synchronous complete "
          "(no taskId)", st == 200 and r.get("resultType") == "complete"
          and "taskId" not in r and valid_pub_ip(result_text(o)),
          f"st={st} {json_txt(o)[:140]}")

    # D4 2026 WITH ext declaration -> CreateTaskResult flat shape
    st, data, _ = post("/mcp", {"jsonrpc": "2.0", "id": 4, "method": "tools/call",
                                "params": {"name": IP_TOOL, "arguments": {},
                                           "task": {"ttlMs": 60000},
                                           **META_2026_TASKS}},
                       headers=H26_CALL)
    o = sse_last(data, 4)
    r = (o or {}).get("result", {}) or {}
    tid = r.get("taskId")
    check("D4 2026 ext CreateTaskResult flat shape",
          st == 200 and r.get("resultType") == "task" and tid
          and all(k in r for k in ("status", "createdAt", "lastUpdatedAt")),
          f"st={st} {json_txt(o)[:140]}")

    if tid:
        # D5 poll tasks/get to terminal
        final = None
        for _ in range(40):
            st, data, _ = post("/mcp", {"jsonrpc": "2.0", "id": 5,
                                        "method": "tasks/get",
                                        "params": {"taskId": tid,
                                                   **META_2026_TASKS}},
                               headers=H26_GET)
            o = sse_last(data, 5)
            task = (o or {}).get("result") or {}
            if task.get("status") in ("completed", "failed", "cancelled"):
                final = task
                break
            time.sleep(0.3)
        check("D5 tasks/get poll -> completed with embedded result(resultType:complete)",
              final is not None and final.get("status") == "completed"
              and ((final.get("result") or {}).get("resultType") == "complete"),
              f"{json_txt(final)[:140]}")

        # D6 tasks/result returns the tool result with real public IP
        st, data, _ = post("/mcp", {"jsonrpc": "2.0", "id": 6,
                                    "method": "tasks/result",
                                    "params": {"taskId": tid, **META_2026_TASKS}},
                           headers=H26_RESULT)
        o = sse_last(data, 6)
        r = (o or {}).get("result", {}) or {}
        check("D6 tasks/result -> tool result with real public IPv4",
              st == 200 and "error" not in (o or {}) and valid_pub_ip(result_text(o)),
              f"st={st} {json_txt(o)[:140]}")

        # D7 terminal tasks/cancel -> -32602 (loud failure, R358 semantics)
        st, data, _ = post("/mcp", {"jsonrpc": "2.0", "id": 7,
                                    "method": "tasks/cancel",
                                    "params": {"taskId": tid, **META_2026_TASKS}},
                           headers=H26_CANCEL)
        o = sse_last(data, 7)
        check("D7 terminal tasks/cancel -> -32602",
              (o or {}).get("error", {}).get("code") == -32602,
              f"st={st} {json_txt(o)[:120]}")

    # D8 tasks/update unknown id -> -32602
    st, data, _ = post("/mcp", {"jsonrpc": "2.0", "id": 8, "method": "tasks/update",
                                "params": {"taskId": "no-such-task-r1300",
                                           "inputResponses": {},
                                           **META_2026_TASKS}},
                       headers=H26_UPDATE)
    o = sse_last(data, 8)
    check("D8 tasks/update unknown id -> -32602",
          (o or {}).get("error", {}).get("code") == -32602,
          f"st={st} {json_txt(o)[:120]}")


# ── E: prompts/resources per version + $smart ───────────────────────────────
def section_e():
    print("== E: prompts/resources per version + $smart ==")
    for v in ("2025-03-26", "2025-11-25"):
        st, echoed, sid = initialize("/mcp", v, rid=1, client="r1300")
        if st != 200:
            check(f"E {v} initialize", False, f"st={st}")
            continue
        notify_initialized("/mcp", sid)
        H = {"mcp-session-id": sid, "MCP-Protocol-Version": v}
        st, data, _ = post("/mcp", {"jsonrpc": "2.0", "id": 2,
                                    "method": "prompts/list", "params": {}},
                           headers=H)
        o = sse_last(data, 2)
        names = [p.get("name", "")
                 for p in (((o or {}).get("result", {}) or {}).get("prompts", []) or [])]
        check(f"E prompts/list {v} exposes builtin prompt", PROMPT_NAME in names,
              f"st={st} names={names[:6]}")
        st, data, _ = post("/mcp", {"jsonrpc": "2.0", "id": 3,
                                    "method": "prompts/get",
                                    "params": {"name": PROMPT_NAME, "arguments": {}}},
                           headers=H)
        o = sse_last(data, 3)
        txt = json_txt(o)
        leak = "{{" in txt
        check(f"E prompts/get {v} real render, no placeholder leakage",
              st == 200 and "error" not in (o or {}) and not leak
              and "测试提示词Test4" in txt, f"st={st} {txt[:140]}")
        st, data, _ = post("/mcp", {"jsonrpc": "2.0", "id": 4,
                                    "method": "resources/list", "params": {}},
                           headers=H)
        o = sse_last(data, 4)
        uris = [r_.get("uri", "")
                for r_ in (((o or {}).get("result", {}) or {}).get("resources", []) or [])]
        check(f"E resources/list {v} exposes builtin resource",
              st == 200 and "error" not in (o or {}) and len(uris) >= 1,
              f"st={st} uris={uris[:3]}")
        if v == "2025-03-26":
            st, data, _ = post("/mcp", {"jsonrpc": "2.0", "id": 5,
                                        "method": "resources/read",
                                        "params": {"uri": RESOURCE_URI}},
                               headers=H, timeout=60)
            print(f"      [observed] resources/read (remote png URI) -> {st} "
                  f"{data[:90]!r}")
            check("E resources/read -> non-5xx (documented outcome)",
                  st < 500, f"st={st} {data[:120]}")

    # $smart scope: prompts/resources empty per contract
    st, echoed, sid = initialize("/mcp/$smart", "2025-11-25", rid=1, client="r1300")
    if st != 200:
        check("E $smart initialize", False, f"st={st}")
        return
    notify_initialized("/mcp/$smart", sid)
    for method in ("prompts/list", "resources/list"):
        st, data, _ = post("/mcp/$smart", {"jsonrpc": "2.0", "id": 2,
                                           "method": method, "params": {}},
                           headers={"mcp-session-id": sid,
                                    "MCP-Protocol-Version": "2025-11-25"})
        o = sse_last(data, 2)
        key = method.split("/")[0] + "s"
        items = ((o or {}).get("result", {}) or {}).get(key, [])
        check(f"E $smart {method} -> empty per contract",
              st == 200 and "error" not in (o or {}) and items == [],
              f"st={st} items={items[:3]}")


# ── F: leniency chaos ───────────────────────────────────────────────────────
def section_f():
    print("== F: leniency chaos ==")
    path = "/mcp"
    st, echoed, sid = initialize(path, "2025-06-18", rid=1, client="r1300")
    if st != 200 or not sid:
        check("F initialize for leniency session", False, f"st={st}")
        return
    notify_initialized(path, sid)
    base_h = {"mcp-session-id": sid, "MCP-Protocol-Version": "2025-06-18"}
    ping_body = {"jsonrpc": "2.0", "id": 1, "method": "ping", "params": {}}

    # F1 Accept: text/event-stream ONLY on POST
    st, data, _ = req("POST", path,
                      raw_body=json.dumps(ping_body).encode("utf-8"),
                      headers={**base_h, "Content-Type": "application/json",
                               "Accept": "text/event-stream"}, timeout=60)
    print(f"      [observed] Accept text/event-stream only POST -> {st}")
    check("F1 Accept SSE-only POST -> non-5xx (200 inline JSON or SSE)",
          st < 500, f"st={st} {data[:120]}")

    # F2 Content-Type with charset
    body = json.dumps({"jsonrpc": "2.0", "id": 2, "method": "ping",
                       "params": {}}).encode("utf-8")
    st, data, _ = req("POST", path, raw_body=body,
                      headers={**base_h, "Content-Type": "application/json; charset=utf-8"})
    o = sse_last(data, 2)
    check("F2 Content-Type charset=utf-8 accepted",
          st == 200 and o is not None and "result" in o, f"st={st} {data[:120]}")

    # F3 body with trailing newline
    st, data, _ = req("POST", path, raw_body=json.dumps(
        {"jsonrpc": "2.0", "id": 3, "method": "ping", "params": {}}).encode() + b"\n",
        headers={**base_h})
    o = sse_last(data, 3)
    print(f"      [observed] trailing-newline body -> {st}")
    check("F3 body trailing newline accepted", st == 200 and o is not None
          and "result" in o, f"st={st} {data[:120]}")

    # F4 body with CRLF
    st, data, _ = req("POST", path, raw_body=json.dumps(
        {"jsonrpc": "2.0", "id": 4, "method": "ping", "params": {}}).encode() + b"\r\n",
        headers={**base_h})
    o = sse_last(data, 4)
    print(f"      [observed] CRLF-terminated body -> {st}")
    check("F4 body trailing CRLF accepted", st == 200 and o is not None
          and "result" in o, f"st={st} {data[:120]}")

    # F5 body with UTF-8 BOM: clean 4xx or accept — document, fail only 5xx
    st, data, _ = req("POST", path, raw_body=b"\xef\xbb\xbf" + body,
                      headers={**base_h})
    print(f"      [observed] BOM body -> {st} {data[:90]!r}")
    check("F5 body with BOM -> clean 4xx or accept, non-5xx", st < 500,
          f"st={st} {data[:120]}")

    # F6 whitespace-only body -> 4xx
    st, data, _ = req("POST", path, raw_body=b"   \n  ", headers={**base_h})
    print(f"      [observed] whitespace-only body -> {st} {data[:80]!r}")
    check("F6 whitespace-only body -> 4xx (not 5xx, not hang)",
          400 <= st < 500, f"st={st} {data[:120]}")


def main():
    print(f"== r1300 against {HOST}:{PORT} ==")
    try:
        section_a()
        section_b()
        section_c()
        section_d()
        section_e()
        section_f()
    finally:
        drop_tmp_group()
        row = group_row()
        check("Z temp CJK group deleted in finally", row is None,
              f"leftover={row}")
    print(f"\n== r1300 summary: pass={base.PASS} fail={base.FAIL} "
          f"skip={base.SKIPS} ==")
    for f in base.FAILURES:
        print(f"  FAILED: {f}")
    sys.exit(0 if base.FAIL == 0 else 1)


if __name__ == "__main__":
    main()
