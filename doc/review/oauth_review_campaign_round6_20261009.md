# OAuth 复核战役 第六轮（2026-10-09 下午，三代理：commands 全量 / mcp 传输层 11 文件 / rag+fts 16000 行）

## 1. 覆盖面
- **a1**：commands/ 全部 22 文件（config redaction 二次复审 / runtime "." 修复验证 / 全门控对账）。
- **a2**：src-tauri/src/mcp/ 12 文件端到端（rmcp 3.4.1 vendored 源码逐 API 交叉验证）。
- **a3**：rag/ 全部 + fts_service（重点 git.rs / service.rs 近期修复区 / vectordb / extract / chunker）。

## 2. 修复清单（5M + 4L，全部亲核落盘）

### a1（commands）
1. **[M] fetch_python_versions 无超时** —— 自建 client 只设 UA，endoflife.date 响应挂起时 list_python_versions 永久挂死（同文件 fetch_node_lts 走 http_client() 有 15s/120s，唯 Python 路径漏）→ 补 connect/read timeout。
2. **[L] cloud 两命令无门控** —— list_cloud_servers/get_cloud_server_tools 花存储的 MCPRouter API key 发外部鉴权请求，skipAuth=false 时非 admin 可驱动（配额消耗面）→ 补 require_admin。

### a2（mcp 传输层，核心发现）
3. **[M] rmcp 连接级错误不再触发任何恢复** —— 迁移后 is_connected() 只在显式 connect/disconnect 翻转：子进程死亡/HTTP 流断开/OAuth token 过期从不降标志 → session_pool/on_demand 的逐出分支（含 Arc::ptr_eq 机器）与 mcp_manager session_rebuild 的 !s.connected 过滤（30s 循环）对 rmcp 传输全是死代码；共享 pool 只记日志不逐出 → **connected 改 AtomicBool + mark_conn_loss 钩子**：list_tools/call_tool_once/tasks-get 三类出口匹配连接级错误（connection closed/send message error/child process/channel closed/broken pipe/unexpected session state）时置 false，既有逐出与重建路径全部复活（SSE 手写 transport 原本正确不受影响）。
4. **[L] ClientLifecycleMode::Auto 固定 10s discover 探测** —— rmcp 3.4.1 DEFAULT_AUTO_DISCOVER_TIMEOUT=10s，legacy 服务器每次连接恒多 10s（重试 3 次即 ≥30s）→ 记录（SDK 无配置面暴露；语义正确仅延迟，已在传输日志可见）。

### a3（rag+fts）
5. **[M] gix::url::parse 错误未脱敏** —— clone_blocking 唯一漏网出口，parse 错误回显含 user:pass 的输入 → scrub_credentials 包裹。
6. **[M] refresh_persistent 失败泄漏 .persist-new-* 装载目录** —— 两个 clone 错误臂不清理，auth 失败 repo 每 auto-tick 累积一个 partial clone → 两臂补 remove_dir_all。
7. **[M] canonicalize_url 收养任意重定向为 repo 身份** —— SSO 网关 302 探测到 /login（无 /info/refs）时 split().next() 返回整个 login URL，repo_hash 永久翻转、clone/凭证全孤儿化且入 REDIRECT_CACHE → 仅当 final_url 仍含 /info/refs 才收养。
8. **[M] rag_file_update 改名可毁 meta** —— 改名到 `{x}.meta`（docId 由 list/search 公开）：content_path_for 候选 4 解析到 meta JSON，rename 用内容覆盖 meta → 文档永久销毁（或他文档 meta 被毁后 prune 删除）→ 更新路径加 .meta 名拒绝 + 候选 4 拒绝 *.meta。
9. **[M] 畸形 PDF 深度递归 SIGSEGV** —— pdf_oxide 0.3.77 parse_array→parse_object 无深度上限，`[[[[` 数千层栈溢出杀整个 app（spawn_blocking 也不可捕获）→ from_bytes 前字节级括号深度预扫（>512 拒绝）。
10. **[L] md5_of_file 无上限整读** —— fs::read 全量入 RAM，copy 源文件导入后增长则每 tick OOM 风险 → 流式 MD5（256KB 分块）。
11. **[L] 凭证 tmp 复用 0644** —— OpenOptions mode(0600) 仅对 create 生效，pre-fix 残留 tmp 被原样重用后 rename 成 world-readable → 先 remove_file 再 open。
12. **[L] reindex_all 全败仍清 NEEDS_REINDEX + 写 indexedModel** —— 失败文档永无自动重试 → done > 0 才清/写。

### 记录不修（有据）
- remove_persistent 最后文档删除在 import/batch 期间 defer 后无人补清（L1）、abort token create_dir_all 早退泄漏（L2）、reindex_doc 删后插失败 chunk_count 陈旧（L4）、keyword_search 紧帽无 ORDER BY（L6，smart 已有宽帽范式）、IMPORT_SESSION 无 CAS（L8）、OCR tmp 0644（L9）、Auto 探测延迟（a2-L4）——均记录排期。

## 3. 证伪/清洁确认
- redaction 边界（a[..eq+1] ASCII '=' 保证安全）、validate_version_arg 数字要求全 caller 数字形态、tar/symlink 守卫、全门控、settings-import round-trip、嵌套 BEGIN 修复。
- rmcp API 语义逐项对照 vendored 源码（CallToolResponse/TaskPayload/RequestMetaObject 透明序列化/ttl_ms Option/Auto lifecycle）全部如文档所述。
- fts_service 全净（clear_table_tx 唯一/BEGIN IMMEDIATE/OnceLock 无 leak/LIKE 转义/IN 分块）。
- canonicalize 13 处键点全走 canonical、META_LOCK 序、BATCH CAS+RAII、排除路径 Windows 归一、ensure_table 自愈。
- chunker AST 预分区阈值实为 64KB（AGENTS.md 记 256KB 系文档漂移，已留 Info）。

## 4. 验证
- cargo check 0 错 0 警；cargo test --lib **119 passed**。
- **60 套 E2E 全绿**（新二进制全量；round50c 首轮公网瞬态失败复跑 19/19）。

## 5. 复盘
1. **迁移引入的语义断链是最大盲区**：is_connected 契约从「SSE 手写的活性信号」变成「rmcp 的状态位」——签名没变、语义死了，所有消费者（逐出/重建）静默变死代码。**教训：替换底层实现时，逐一审计契约消费方是否仍被喂活。**
2. **split().next() 永远有值**：canonicalize 用 split("/info/refs").next() 当守卫，但 split 找不到时返回整个字符串——「以分隔符存在为前提」的解析必须先显式 contains 检查。
3. **保留名守卫按入口穷举**：create 路径封了 .meta，update 改名路径漏——同一保留名必须在「所有能到达文件系统的名字变换点」拒绝，不是「最先想到的入口」。
4. **外部库递归不可信**：pdf_oxide 与 chunker 同类问题（不可捕获 SIGSEGV）二度出现——引入解析库时 grep 上游递归形态，或入口深度预扫。
5. 收敛态势：a2 的 5200 行真读仅 1M+1L 且都源于迁移语义断链；a3 集中在 git/service 近期修复邻接区——「修复区邻接」仍是缺陷主产地（本轮 5M 中 4 项在 round19/20 修过的同一批函数内）。
