# rmcp 复核第三十六轮（R36，2026-10-03）：on_demand 并发调用互踩（计数器化）+ 迁移 panic 循环 + 日志降级丢过滤器

> 复核范围：mcp/session_pool + on_demand + services/mcp_tasks + subscription_hub 全量重读；
> db/migration.rs 1700 行 + log_service + config_service + server_tool_config_service 重读。
> 方法：侵蚀审计全在 → 2 个并行逐行代理 → 亲核验证 7 项 → 修复 → v36 套件 → 全量回归。

## 修复（7 项：Medium×3，Low×4）

### O1 [M] on_demand `in_flight` 是 bool 不是计数器——并发调用互踩断连
两个并发调用 A/B 同服务器：A 先完成把 flag 清 false，B 还在跑；idle 定时器看到
`in_flight==false && last_used==快照` → 移除条目 disconnect——**B 的上游调用被打断**，
正是该 flag 要防的场景（600s 调用 vs 300s idle）。**修复**：改 `u32` 计数器，手动清
除/守卫 Drop 双路径 `saturating_sub`，定时器条件 `in_flight == 0`。

### O2 [L] flag 置位与守卫构建之间有 await——取消窗口泄漏计数器
`entry.in_flight=true` 后先 `schedule_idle().await`（两处 await）再构建 InFlightGuard
——future 在此窗口被取消则手动清除和守卫 Drop 都不跑 → 计数器永久泄漏（迁移自 bool 的
窗口）。**修复**：守卫构建后再 schedule_idle（await 全部进入守卫保护范围）。

### M1 [M] 迁移 v13/v14/v25 `skills` 非对象 → IndexMut panic 永久崩溃循环
`config.get("skills").is_none()` 只挡缺失——`"skills"` 是字符串/数组（config 深合并可
达）时 `config["skills"]["agents"]=…` **panic**；panic 发生在 set_version 之前 → 版本
不推进 → 每次启动重跑再 panic（`.bak` 已被替换，无恢复路径）。**修复**：三处改类型检
查（非对象覆写为 `{}`）。

### M2 [M] log_service LIKE 降级静默丢弃 level/server_name 过滤器
FTS 路径应用两个 WHERE，三条降级路径（空表/零命中/Err）的 like_search_logs 只按
message 过滤——「按级别筛选 + 搜索」降级时返回混级结果。**修复**：like_search_logs 接
收过滤器参数，QueryBuilder 条件拼接（与 FTS 同语义）。

### M3 [L] server_tool_config update_description 读-插竞态
exists 检查与 INSERT 无事务——并发同键双双「不存在」→ 第二个撞 UNIQUE 报错（upsert
路径早为此用了 ON CONFLICT）。**修复**：无条件 `INSERT … ON CONFLICT DO UPDATE`。

### M4 [L] v22 索引 `IF NOT EXISTS` 静默 no-op
v21 已建同名索引（不同列表）——v22 的 created_at 排序键从未真正落地。**修复**：
DROP INDEX IF EXISTS 后重建。

### M5 [L] v11 DROP COLUMN `.ok()` 吞真实错误
无 DROP COLUMN 支持的 SQLite / I/O 错误也被吞 → 版本推进但 server_name NOT NULL 仍在
→ 后续 INSERT 全失败。**修复**：pragma_table_info 先查存在，DROP 传播错误。

**记录不修**：v8 UTC→localtime 破坏性非幂等（R14 已定谳「历史已应用」，重跑窗口极窄，
需 26 签名重构）。

## 清洁
- session_pool（ptr_eq 驱逐/enabled 复查/同锁无复活）、mcp_tasks（owner 门控 6 方法一
  致/u64 溢出/sweeper 幂等）、subscription_hub（lane 对称/非阻塞剪枝）、config_service
  、migration v1-v7/v9/v12/v16-v21/v23-v24/v26——零缺陷

## 新套件 rmcp_supplement_v36.py（9 项）
- 4 legacy 回显 + 公网 IP ×2（2025-11-25 + 2026 stateless，IPv4 Body 断言）
- /api/openapi/stats 存活（服务层重构后回归）+ 2026 discover + /health
- 注：O1 属并发时序（需 300s+ 长调用窗口），由计数器语义 + 代码审查保证；M1 由 cargo
  test 的迁移路径覆盖

## 验证
- `cargo check --lib` 0 错 0 警；`cargo test --lib` 102 passed；cargo build ✓
- 40 套件 **1252/1252 全绿**（v36 9/9 新计入；重建二进制热跑）

## 教训
- **互斥标志 vs 计数器的选择由并发度决定**：bool 标志隐含「最多一个并发者」假设，写代
  码时的假设没写进注释就是缺陷温床——O1 的 docstring 只推理了单调用场景。
- panic 在 set_version 之前的迁移 = 崩溃循环：任何迁移体里的 panic 路径（哪怕来自
  serde IndexMut 这种「不会失败」的 API）都要问「重跑会发生什么」。
- 守卫的覆盖范围从它**构建**那一刻开始——构建前的 await 是无保护窗口（O2 与 R35LoginPage
  同族：async 边界窗口）。
