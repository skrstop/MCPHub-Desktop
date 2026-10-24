#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""RAG 文档历史 symlink/copy 全链路自测（对运行中的真实应用）。

Copy 流程：MCP rag_file_create → update×2 → history_versions/get/diff → 磁盘核验 → delete。
Symlink 流程：磁盘手工构造 symlink 文档（text + extractable-with-cache）→ rag_get 视图 →
MCP update 触发快照 → history_get 核验快照来源 → delete → 外部原文件存活核验。
测毕全部清理。
"""
import http.client, json, os, shutil, sys, uuid

HOST, PORT = "localhost", 23333
APP_DIR = os.path.expanduser("~/Library/Application Support/app.mcphub.desktop")
FILES_DIR = os.path.join(APP_DIR, "rag", "files")
EXT_DIR = os.path.join(APP_DIR, "rag-selftest-external")

PASS, FAIL = [], []
def check(name, cond, detail=""):
    (PASS if cond else FAIL).append(name)
    print(("PASS | " if cond else "FAIL | ") + name + (f" :: {str(detail)[:200]}" if not cond else ""))

def req(path, body, headers=None, timeout=120):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
    if headers: h.update(headers)
    c.request("POST", path, json.dumps(body, ensure_ascii=False).encode(), h)
    r = c.getresponse(); d = r.read().decode(); sid = r.getheader("mcp-session-id"); c.close()
    obj = None
    for line in d.splitlines():
        if line.startswith("data:"):
            try: obj = json.loads(line[5:].strip())
            except Exception: pass
    if obj is None:
        try: obj = json.loads(d)
        except Exception: pass
    return r.status, obj, d, sid

def session():
    st, obj, d, sid = req("/mcp", {"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "hist-selftest", "version": "1"}}})
    if sid:
        req("/mcp", {"jsonrpc": "2.0", "method": "notifications/initialized"}, {"Mcp-Session-Id": sid})
    return sid

def call(sid, tool, args, req_id=10):
    st, obj, d, _ = req("/mcp", {"jsonrpc": "2.0", "id": req_id, "method": "tools/call",
        "params": {"name": tool, "arguments": args}}, {"Mcp-Session-Id": sid})
    res = (obj or {}).get("result") or {}
    content = res.get("content") or []
    text = content[0].get("text", "") if content else ""
    return st, res, text, obj

def text_of(sid, tool, args, req_id=10):
    st, res, text, obj = call(sid, tool, args, req_id)
    if obj and "error" in obj:
        raise RuntimeError(f"{tool} error: {obj['error']}")
    return text

# ───────────────────────── Copy 流程 ─────────────────────────
def test_copy():
    sid = session()
    name = "zzz-selftest-copy"
    doc_id = json.loads(text_of(sid, "rag_file_create",
        {"docName": name, "docType": "md", "docContent": "line1 alpha\nline2\n"}))["docId"]
    try:
        folder = os.path.join(FILES_DIR, doc_id)
        check("[copy] per-doc 文件夹布局", os.path.isdir(folder), folder)

        text_of(sid, "rag_file_update", {"docId": doc_id, "docContent": "line1 beta\nline2\n"}, 11)
        text_of(sid, "rag_file_update", {"docId": doc_id, "docContent": "line1 gamma\nline2\nline3\n"}, 12)

        hist = os.path.join(folder, "history")
        check("[copy] history 目录生成", os.path.isdir(hist))
        check("[copy] v1 快照落盘", os.path.isfile(os.path.join(hist, "v1.json")) and os.path.isfile(os.path.join(hist, "v1.md")))
        check("[copy] v2 快照落盘", os.path.isfile(os.path.join(hist, "v2.json")))

        versions = json.loads(text_of(sid, "rag_file_history_versions", {"docId": doc_id}, 13))
        check("[copy] 版本列表倒序 [2,1]", [v["version"] for v in versions] == [2, 1], versions)

        v1 = text_of(sid, "rag_file_history_get", {"docId": doc_id, "version": 1}, 14)
        check("[copy] v1 内容=第一次更新前", v1 == "line1 alpha\nline2\n", repr(v1))
        v2 = text_of(sid, "rag_file_history_get", {"docId": doc_id, "version": 2}, 15)
        check("[copy] v2 内容=第二次更新前", v2 == "line1 beta\nline2\n", repr(v2))

        diff = text_of(sid, "rag_file_history_diff", {"docId": doc_id, "fromVersion": 1, "toVersion": 2}, 16)
        check("[copy] diff v1→v2 含 -alpha +beta", "-line1 alpha" in diff and "+line1 beta" in diff, diff[:150])
        diff_cur = text_of(sid, "rag_file_history_diff", {"docId": doc_id, "fromVersion": 2}, 17)
        check("[copy] diff v2→当前 含 +gamma", "+line1 gamma" in diff_cur, diff_cur[:150])

        cur = text_of(sid, "rag_get", {"docId": doc_id}, 18)
        check("[copy] rag_get 当前内容（os21 回归）", cur == "line1 gamma\nline2\nline3\n", repr(cur))

        meta = json.load(open(os.path.join(folder, "meta.json")))
        check("[copy] meta.version=3", meta.get("version") == 3, meta.get("version"))
    finally:
        text_of(sid, "rag_file_delete", {"docId": doc_id}, 19)
        check("[copy] delete 整文件夹移除", not os.path.exists(os.path.join(FILES_DIR, doc_id)))

# ─────────────────────── Symlink 流程 ───────────────────────
def craft_meta(doc_id, name, version, method, original_path, file_type=None, raw_ext=None):
    folder = os.path.join(FILES_DIR, doc_id)
    os.makedirs(folder, exist_ok=True)
    meta = {
        "id": doc_id, "name": name, "title": None, "tags": [], "size": 0,
        "uploaded_at": "2026-10-22 22:00", "chunk_count": 0, "version": version,
        "file_type": file_type, "method": method, "original_path": original_path,
        "md5": None, "source": None, "raw_ext": raw_ext,
    }
    json.dump(meta, open(os.path.join(folder, "meta.json"), "w"))
    return folder

def test_symlink_text():
    sid = session()
    doc_id = "symtest-" + uuid.uuid4().hex[:8]
    os.makedirs(EXT_DIR, exist_ok=True)
    ext_file = os.path.join(EXT_DIR, "sym-notes.txt")
    with open(ext_file, "w") as f:
        f.write("external v1 line\n")
    craft_meta(doc_id, "zzz-symtest-notes.txt", 1, "symlink", ext_file)
    try:
        cur = text_of(sid, "rag_get", {"docId": doc_id}, 20)
        check("[sym-text] rag_get 读外部原文件", cur == "external v1 line\n", repr(cur))

        # MCP rag_file_update 对 symlink 只读拒绝（防覆写外部原文件）——正确守卫。
        st, res, text, obj = call(sid, "rag_file_update", {"docId": doc_id, "docContent": "x"}, 21)
        check("[sym-text] MCP update 正确拒绝 symlink",
              obj and "error" in obj and "read-only" in obj["error"].get("message", ""), obj)

        # updateExternalFile=true：覆写外部原文件 + 生成历史。
        text_of(sid, "rag_file_update",
                {"docId": doc_id, "docContent": "external v1 line\nmcp overwrite\n", "updateExternalFile": True}, 25)
        ext_now = open(ext_file).read()
        check("[sym-text] updateExternalFile=true 覆写外部原文件", ext_now == "external v1 line\nmcp overwrite\n", repr(ext_now))
        versions = json.loads(text_of(sid, "rag_file_history_versions", {"docId": doc_id}, 26))
        check("[sym-text] 覆写生成历史 v1", [v["version"] for v in versions] == [1], versions)
        v1 = text_of(sid, "rag_file_history_get", {"docId": doc_id, "version": 1}, 27)
        check("[sym-text] v1 快照=覆写前的外部内容", v1 == "external v1 line\n", repr(v1))
        cur = text_of(sid, "rag_get", {"docId": doc_id}, 28)
        check("[sym-text] rag_get=覆写后内容", cur == "external v1 line\nmcp overwrite\n", repr(cur))
    finally:
        text_of(sid, "rag_file_delete", {"docId": doc_id}, 24)
        check("[sym-text] delete 文件夹移除", not os.path.exists(os.path.join(FILES_DIR, doc_id)))
        check("[sym-text] 外部原文件存活（symlink 不删源）", os.path.isfile(ext_file))

def test_symlink_extractable_cache():
    sid = session()
    doc_id = "symtest-" + uuid.uuid4().hex[:8]
    os.makedirs(EXT_DIR, exist_ok=True)
    ext_file = os.path.join(EXT_DIR, "sym-scan.pdf")
    with open(ext_file, "wb") as f:
        f.write(b"%PDF-fake-external-binary")
    folder = craft_meta(doc_id, "zzz-symtest-scan.pdf", 2, "symlink", ext_file,
                        file_type="PDF", raw_ext=".pdf")
    with open(os.path.join(folder, doc_id + ".md"), "w") as f:
        f.write("# cached extraction text")
    try:
        cur = text_of(sid, "rag_get", {"docId": doc_id}, 30)
        check("[sym-pdf] rag_get 优先读 .md 缓存（非二进制乱码）", cur == "# cached extraction text", repr(cur[:80]))

        # MCP update 对 symlink 只读拒绝（同 sym-text）。
        st, res, text, obj = call(sid, "rag_file_update", {"docId": doc_id, "docContent": "# x"}, 31)
        check("[sym-pdf] MCP update 正确拒绝 symlink",
              obj and "error" in obj and "read-only" in obj["error"].get("message", ""), obj)

        # updateExternalFile=true + 可提取：外部原文件被文本覆写（用户显式 opt-in）+ 缓存同步刷新。
        text_of(sid, "rag_file_update",
                {"docId": doc_id, "docContent": "# new extraction after update\n", "updateExternalFile": True}, 36)
        ext_bytes = open(ext_file, "rb").read()
        check("[sym-pdf] updateExternalFile=true 覆写外部原文件", ext_bytes == b"# new extraction after update\n", ext_bytes[:40])
        cur2 = text_of(sid, "rag_get", {"docId": doc_id}, 37)
        check("[sym-pdf] 更新后视图=新缓存内容", cur2 == "# new extraction after update\n", repr(cur2[:80]))
        versions = json.loads(text_of(sid, "rag_file_history_versions", {"docId": doc_id}, 38))
        check("[sym-pdf] 更新生成历史 v2", [v["version"] for v in versions] == [2], versions)
        v2 = text_of(sid, "rag_file_history_get", {"docId": doc_id, "version": 2}, 39)
        check("[sym-pdf] v2 快照=更新前缓存", v2 == "# cached extraction text", repr(v2))

        # 历史查询面在 symlink 文档上正常工作（全新文档空历史）。
        fresh_id = "symtest-" + uuid.uuid4().hex[:8]
        craft_meta(fresh_id, "zzz-symtest-fresh.pdf", 1, "symlink", ext_file, file_type="PDF", raw_ext=".pdf")
        try:
            versions = json.loads(text_of(sid, "rag_file_history_versions", {"docId": fresh_id}, 40))
            check("[sym-pdf] history_versions 空列表不报错", versions == [], versions)
        finally:
            text_of(sid, "rag_file_delete", {"docId": fresh_id}, 41)
    finally:
        text_of(sid, "rag_file_delete", {"docId": doc_id}, 35)
        check("[sym-pdf] delete 文件夹移除", not os.path.exists(os.path.join(FILES_DIR, doc_id)))
        check("[sym-pdf] 外部原文件存活", os.path.isfile(ext_file))

# ─────────────────── Copy 写穿外部原文件 ───────────────────
def test_copy_write_through():
    sid = session()
    doc_id = "symtest-" + uuid.uuid4().hex[:8]
    os.makedirs(EXT_DIR, exist_ok=True)
    ext_file = os.path.join(EXT_DIR, "copy-notes.txt")
    with open(ext_file, "w") as f:
        f.write("copy source v1\n")
    # copy 导入：本地有拷贝 + original_path 指向外部源文件
    folder = craft_meta(doc_id, "zzz-copytest-notes.txt", 1, "copy", ext_file)
    with open(os.path.join(folder, doc_id + ".txt"), "w") as f:
        f.write("copy source v1\n")
    try:
        # 默认 false：只改本地拷贝，外部源文件不动。
        text_of(sid, "rag_file_update", {"docId": doc_id, "docContent": "local only edit\n"}, 50)
        check("[copy-wt] 默认不写穿外部文件", open(ext_file).read() == "copy source v1\n")

        # updateExternalFile=true：写穿外部源文件 + 生成历史。
        text_of(sid, "rag_file_update",
                {"docId": doc_id, "docContent": "write through edit\n", "updateExternalFile": True}, 51)
        check("[copy-wt] 写穿外部源文件", open(ext_file).read() == "write through edit\n")
        cur = text_of(sid, "rag_get", {"docId": doc_id}, 52)
        check("[copy-wt] 本地内容同步", cur == "write through edit\n", repr(cur))
        versions = json.loads(text_of(sid, "rag_file_history_versions", {"docId": doc_id}, 53))
        check("[copy-wt] 两次更新生成 v1/v2 历史", [v["version"] for v in versions] == [2, 1], versions)
        v1 = text_of(sid, "rag_file_history_get", {"docId": doc_id, "version": 1}, 54)
        check("[copy-wt] v1=第一次更新前", v1 == "copy source v1\n", repr(v1))
        v2 = text_of(sid, "rag_file_history_get", {"docId": doc_id, "version": 2}, 55)
        check("[copy-wt] v2=写穿前内容", v2 == "local only edit\n", repr(v2))
    finally:
        text_of(sid, "rag_file_delete", {"docId": doc_id}, 56)
        check("[copy-wt] delete 文件夹移除", not os.path.exists(os.path.join(FILES_DIR, doc_id)))

# ─────────── 可提取文档历史留存原始文件 + 查看源文件 ───────────
def test_history_raw_snapshot():
    sid = session()
    doc_id = "symtest-" + uuid.uuid4().hex[:8]
    os.makedirs(EXT_DIR, exist_ok=True)
    ext_file = os.path.join(EXT_DIR, "raw-scan.pdf")
    with open(ext_file, "wb") as f:
        f.write(b"%PDF-raw-original-bytes")
    folder = craft_meta(doc_id, "zzz-rawtest-scan.pdf", 1, "copy", ext_file,
                        file_type="PDF", raw_ext=".pdf")
    with open(os.path.join(folder, doc_id + ".md"), "w") as f:
        f.write("# cached extraction v1")
    with open(os.path.join(folder, doc_id + ".pdf"), "wb") as f:
        f.write(b"%PDF-raw-original-bytes")
    try:
        # copy 文档 MCP update 允许 → 快照 v1 应同时留存 .md 文本 + .pdf 原始文件。
        text_of(sid, "rag_file_update", {"docId": doc_id, "docContent": "# cached extraction v2\n"}, 60)
        hist = os.path.join(folder, "history")
        check("[hist-raw] v1 文本快照", os.path.isfile(os.path.join(hist, "v1.json")) and os.path.isfile(os.path.join(hist, "v1.md")))
        check("[hist-raw] v1 原始文件快照 (.pdf)", os.path.isfile(os.path.join(hist, "v1.pdf")))
        raw = open(os.path.join(hist, "v1.pdf"), "rb").read()
        check("[hist-raw] 原始快照字节一致", raw == b"%PDF-raw-original-bytes", raw[:30])
        rec = json.load(open(os.path.join(hist, "v1.json")))
        check("[hist-raw] record.rawExt=.pdf", rec.get("rawExt") == ".pdf", rec)

        versions = json.loads(text_of(sid, "rag_file_history_versions", {"docId": doc_id}, 61))
        check("[hist-raw] 列表透传 rawExt", versions and versions[0].get("rawExt") == ".pdf", versions)

        # 历史详情读取：originalPath 指向归档原始快照（查看源文件按钮条件）。
        st, res, text, obj = call(sid, "rag_get", {"docId": doc_id}, 62)
        check("[hist-raw] 当前内容=更新后", "# cached extraction v2" in text, text[:60])
    finally:
        text_of(sid, "rag_file_delete", {"docId": doc_id}, 63)
        check("[hist-raw] delete 文件夹移除", not os.path.exists(os.path.join(FILES_DIR, doc_id)))

if __name__ == "__main__":
    test_copy()
    test_symlink_text()
    test_symlink_extractable_cache()
    test_copy_write_through()
    test_history_raw_snapshot()
    # 清理外部目录
    shutil.rmtree(EXT_DIR, ignore_errors=True)
    print(f"\n== {len(PASS)} passed, {len(FAIL)} failed ==")
    if FAIL:
        print("FAILED:", FAIL); sys.exit(1)
