# OAuth 复核战役 · 第十九轮（R19）报告 — 2026-10-11

> 范围：R18 修复合自身复审 + R18 SKIP 用例验证 + 新套件弱断言审计 + 亲核修复 + 修复后全量复验。
> 前置：R18（doc/oauth_review_campaign_round18_20261011.md）——#1300 SSRF guard、#1303 ui:// 镜像、LoginPage 无 SSO 通知。

## 1. 切片与结论

### A1 · R18 修复合复审（代理 + 亲核）— **零产品缺陷**
- `Policy::none()` 重定向语义核实：reqwest 0.13.5 在 policy 拒绝时**原样返回 3xx 响应**（非错误）→ fetch_spec 的 `!resp.status().is_success()` 产生干净的 "HTTP 30x" 失败；与 `resolve_to_addrs` DNS 锁定无交互。
- reqwest 用户 URL 客户端全仓扫描：仅 openapi_transport fetch_spec 拉取用户提供的 URL（已受 SSRF guard 保护）；progress.rs（npmjs/pypi 固定 host）与 cloud.rs（admin 配置 baseUrl）为记录性低危。
- LoginPage 无 SSO 通知块 JSX 复核干净。

### A2 · R18 SKIP 用例验证（代理）— 环境绑定成立，新增套件
- 新套件 `rmcp_round_r1900.py` **23/23**（+1 SKIP）。
- 经验证的拾取约束：DB 直插服务器行**运行时永不入池**——连接仅经启动 start_all / Tauri 命令（update_server/toggle_server）/ session_rebuild tick（仅迭代 `pool::get_all_statuses()`，mcp_manager.rs:55-65）。SSRF E2E 同理受限于 connect 时 fetch_spec → 单测（4 个 ssrf_guard 测试）覆盖逻辑面；SKIP 为环境绑定非缺口。

### A3 · 新套件弱断言审计（代理）→ A4 加固（9 项全部落盘）
- `rmcp_round_apps_ui.py`：try 作用域、inits==1、port 0、外部资源重试/跳过。
- `rmcp_round_r1800.py`：A0 空洞通过修复、A3b 命中增量断言、基线还原契约、port 0。
- `rmcp_round_r1700.py`：pin 还原（非清除）、文档计数预清理 + **API_DELETED_IDS 自实现检查**。

## 2. 【产品缺陷·真】rag_file_delete 的 SQL 镜像行因 SQLITE_BUSY 永久滞留（本轮亲核发现）

**现象**：MCP `rag_file_delete` 返回 `{"deleted": true}` 且内容文件/meta/向量全部删除，但 `rag_docs` SQL 行 + `fts_rag_docs` 行**80s+ 永不清理**（app_log 实锤：`remove_doc_sql for {id} failed: database is locked (code: 5)`，4/4 探针全部复现）。

**根因**：`remove_doc_sql` 无 busy 重试——池配置 5s busy_timeout，但活动日志写突发持锁可越过窗口。同文件 `upsert_doc_sql` 已有 4 次退避重试模式（此前修「tag 下拉为空」时引入），`remove_doc_sql` 是漏网的同族点。

**缓解层存在但延迟**：`prune_stale_sql_docs`（list_docs / search_docs_paged 入口）会清 `.meta` 丢失的孤儿行——最终自愈，但 MCP 面的 rag_search 不走该入口 → 孤儿行可长期滞留（FTS 搜索命中死行）。

**修复**：`remove_doc_sql` 重命名为 `remove_doc_sql_inner`，外层新 `remove_doc_sql` busy-retry 包装（镜像 upsert 模式：4 次 × 150ms×attempt 退避，仅对 `database is locked`/`code: 5` 重试）。

**验证**：修复后探针 **0.0s 行删除**（此前 40s+ 不删）；r1700 套件 42/42（B4 cleanup 行清理由 SKIP 转真 PASS）；cargo check 0 错 0 警；cargo test --lib rag 34 passed。

**教训**：①带 SKIP 的「cleanup row-prune confirmation (deferred prune / db-lock)」写法把真缺陷掩盖了 17 轮——cleanup 类断言降级 SKIP 时必须同时给出自愈路径证据（prune 入口可达性），否则应保持 FAIL；②同文件内同类操作一有一无重试模式就是缺口信号（与 R27/R31/R36 同族教训第 4 次）。

## 3. 修复后全量复验（新二进制重启实测）

| 验证 | 结果 |
| --- | --- |
| rmcp_round_r1700.py | **42/42**（SKIP 3） |
| rmcp_round_apps_ui.py | 7/7（SKIP 2） |
| rmcp_round_r1800.py | 21/21（SKIP 5） |
| rmcp_round_r1900.py | 23/23（SKIP 1） |
| rmcp_public_ip_matrix.py | **10/10**（公网IP 221.225.50.167 真实返回） |
| rmcp_full_matrix_v18.py | **262/262** |
| cargo check | 0 错 0 警 |
| cargo test --lib (rag 过滤) | 34 passed |
| py_compile 全部改动套件 | 通过 |

## 4. 记录不修
- prune_stale_sql_docs 不覆盖 MCP rag_search 路径——busy-retry 落地后主路径已即时一致，prune 是防御性兜底（保持现状）。
- cloud.rs baseUrl 无 SSRF guard（admin-only 配置面，与 #1300 范围裁定一致）。
