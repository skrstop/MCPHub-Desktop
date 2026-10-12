#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Mock stdio MCP upstream for the ui:// dynamic-resource E2E suite
(rmcp_round_apps_ui.py).

Speaks newline-delimited JSON-RPC over stdio (rmcp child-process transport).
Behavior:
  initialize            -> echo protocolVersion, advertise tools+resources
  notifications/*       -> no response
  tools/list            -> 1 dummy tool
  resources/list        -> EMPTY (ui:// widgets are dynamic, never listed)
  resources/read        -> ui://test/widget  => TextResourceContents html
                        -> anything else    => JSON-RPC error -32602
Every successful initialize appends "INIT <ts>" to $APPSUI_MOCK_LOG (used by
the suite to assert cold-start-then-reuse semantics for on-demand servers).
"""
import json
import os
import sys
import time

LOG = os.environ.get("APPSUI_MOCK_LOG", "")

WIDGET_URI = "ui://test/widget"


def log(line):
    if LOG:
        try:
            with open(LOG, "a", encoding="utf-8") as f:
                f.write(line + "\n")
        except Exception:
            pass


def reply(rid, result):
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": rid, "result": result}) + "\n")
    sys.stdout.flush()


def err(rid, code, msg):
    sys.stdout.write(json.dumps(
        {"jsonrpc": "2.0", "id": rid,
         "error": {"code": code, "message": msg}}) + "\n")
    sys.stdout.flush()


def main():
    for raw in sys.stdin:
        raw = raw.strip()
        if not raw:
            continue
        try:
            msg = json.loads(raw)
        except Exception:
            continue
        method = msg.get("method", "")
        rid = msg.get("id")
        if method == "initialize":
            log("INIT %s" % time.time())
            reply(rid, {
                "protocolVersion": msg.get("params", {}).get(
                    "protocolVersion", "2025-06-18"),
                "capabilities": {"tools": {}, "resources": {}},
                "serverInfo": {"name": "appsui-mock", "version": "1.0.0"},
            })
        elif rid is None:
            continue  # notification — no response
        elif method == "tools/list":
            reply(rid, {"tools": [{
                "name": "dummy",
                "description": "dummy tool",
                "inputSchema": {"type": "object", "properties": {}}}]})
        elif method == "resources/list":
            reply(rid, {"resources": []})
        elif method == "resources/read":
            uri = (msg.get("params") or {}).get("uri", "")
            if uri == WIDGET_URI:
                reply(rid, {"contents": [{
                    "uri": WIDGET_URI,
                    "mimeType": "text/html",
                    "text": "<html>widget</html>"}]})
            else:
                err(rid, -32602, "Resource '%s' not found" % uri)
        else:
            err(rid, -32601, "Method not found: %s" % method)


if __name__ == "__main__":
    try:
        main()
    finally:
        log("EXIT %s" % time.time())
