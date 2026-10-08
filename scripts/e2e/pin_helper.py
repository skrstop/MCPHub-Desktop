"""Shared pin helper for E2E suites that call tools through the $smart lane.

The MCP $smart surface has list/call pin parity: tools/list exposes only
pinned tools and both the raw-name and smart_route_call lanes refuse
unpinned tools. Suites that exercise a real tool call through $smart must
pin the target first and restore afterwards.
"""
import sqlite3
import time

DB = "/Users/jphoebe/Library/Application Support/app.mcphub.desktop/mcphub.db"


def pin(server: str, tool: str):
    con = sqlite3.connect(DB)
    con.execute(
        """INSERT INTO server_tool_config (id, server_name, item_type, item_name, enabled, description, pinned)
           VALUES (lower(hex(randomblob(16))), ?, 'tool', ?, 1, NULL, 1)
           ON CONFLICT(server_name, item_type, item_name) DO UPDATE SET pinned=1""",
        (server, tool),
    )
    con.commit()
    con.close()
    time.sleep(0.6)


def unpin(server: str, tool: str):
    con = sqlite3.connect(DB)
    con.execute(
        "UPDATE server_tool_config SET pinned=0 WHERE server_name=? AND item_type='tool' AND item_name=?",
        (server, tool),
    )
    con.commit()
    con.close()
    time.sleep(0.6)
