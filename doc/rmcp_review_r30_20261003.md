# rmcp 复核第三十轮（R30，2026-10-03）：FTS 重建快照竞态 + 更新安装监听器竞态 + 资源名称守卫

> 复核范围：fts_service / market_service / bearer_key_service / group_service / resource_service（Rust）；
> AuthContext / SettingsContext / AboutDialog / version.ts / changelogService / mcpClientSnippets（前端）。
> 方法：侵蚀审计全在 → 2 个并行逐行代理 → 亲核验证 7 项 → 修复 → v30 套件 → 全量回归。

## 修复（7 项：Medium×2，Low×5）

### R1 [M] AboutDialog 更新检查失败后卡「检查中」spinner
catch 块用**陈旧闭包** `updateInfo` 判断是否重置——第二次检查失败时（首次已成功、
updateInfo truthy）不重置，state 停在 `source:'checking'`，280 行条件
`isChecking || source==='checking'` 永真 → spinner 永转。**修复**：catch 无条件设置
error 状态（与开头无条件设 checking 对称）。

### R2 [M] version.ts 安装结果监听器注册竞态
`listen('updater://install-result')` 的异步注册**未 await** 就 `invoke('install_update_
cancelable')`——Rust 任务可立即失败（无效 rid/updater 配置）并在注册落地前发终态事件，
Tauri 不缓冲事件 → promise 永不 settle，对话框永久卡 "downloading"；invoke 本身
reject 时监听器直接泄漏。**修复**：先 `await listen(...)` 再 invoke；settle 统一经
unlisten 清理；`finished` await 后补 unlisten 兜底。

### R3 [L] fts_service rebuild 快照-清空竞态仍丢行
rebuild_one / backfill_app_log_if_empty 的「SELECT 放进事务内」修复不完整——WAL 下
deferred 读事务持**快照**，并发写者仍可在快照与 in-tx DELETE 之间提交：清空删掉新行
的 FTS 条目而插入来自陈旧快照 → 行永久不可搜（启动时 rebuild_all 与用户写并发可达）。
**修复**：两处 `pool.begin_with("BEGIN IMMEDIATE")` 先取写锁再快照。

### R4 [L] resource name 清空 → FTS ref_id "" 死行
`BuiltinResourcePayload.name: Option<String>`，update 传 None 时旧名行删除后 upsert
按 `unwrap_or("")` 插 ref_id="" 的行——不可解析、两个 NULL 名互相偷行、rebuild_one
跳过 NULL 名行造成永久漂移。**修复**：create/update 入口拒绝空/缺失 name
（与重名拒绝同语义）。

### R5 [L] market_service 解析失败静默空目录
`from_str(SERVERS_JSON).unwrap_or_default()` 吞掉 shape drift → Market 页面全空无任
何日志。**修复**：`unwrap_or_else` 打 `log::error!` 后降级默认。

### R6 [L] group create 返回伪造 created_at
返回 `chrono::Utc::now().to_rfc3339()`，与 DB DEFAULT 生成的格式/值不一致——create
响应与 list/update 的后续读取不同。**修复**：commit 后 `find_by_name_or_id` 重读
（对齐 update 的做法）。

### R7 [L] exportMCPSettings 未 encodeURIComponent
serverName 含 `&`/`#`/空格时查询串损坏 → Copy Client Config 恒失败无解。
**修复**：`encodeURIComponent(serverName)`。

## 清洁
- bearer_key_service（常数时间比较/长度折叠/首匹配逻辑全对）
- fts_service 分词/转义/OnceLock SQL 缓存/AssertSqlSafe LIKE 路径——无 SQL 注入
- AuthContext、changelogService、mcpClientSnippets（JSON 全走 stringify、TOML 引号
  合法、shellQuote 正确）——零缺陷
- SettingsContext 共享 state 更新函数逐渲染重建（无 stale closure）、httpPort 写入键
  与 Rust 读取端一致

## 新套件 rmcp_supplement_v30.py（7 项）
- /api/openapi/servers 存活（market 解析不再静默空）
- 真实公网 IP 调用 ×3（2024-11-05 / 2025-03-26 + 2026 stateless，IPv4 Body 断言）
- initialize 会话 mint ×2 + /health
- 注：group/resource CRUD 是 Tauri-command-only（无 REST 路由，router 已核），R6/R4
  由 cargo check + 单测覆盖

## 验证
- `cargo check --lib` 0 错 0 警；`cargo test --lib` 102 passed；cargo build ✓
- `tsc` 0 错误；`npm run build` ✓
- 34 套件 **1197/1197 全绿**（v30 7/7 新计入；重建二进制热跑）

## 教训
- 「把读放进事务里」不等于竞态修复：WAL deferred 事务是快照语义，读-清-写三步要防
  并发写者必须 BEGIN IMMEDIATE 先取写锁。
- 事件驱动 promise（event → resolve）必须先注册监听器再触发事件源——任何 async 注册
  与事件发射之间的窗口都是永久悬挂。
