# R491+ 复核第二十四轮报告（2026-10-03）

> 范围：R23 教训落地（契约 diff 脚本化）+ 日志/FTS 写路径 + runtime/skill 服务。2 个独立复核代理 + 套件首测即抓到一个迁移丢失级回归。

## 1. 修复清单（5 项）

| # | 位置 | 问题 | 修复 |
| --- | --- | --- | --- |
| H1 | `services/rmcp_bridge.rs execute_tool_call` | **迁移丢失回归（High）**：rmcp 切换后 `/mcp` 通道 tools/call 完全不写活动日志——`write_activity` 只在 /rest、/api 路径，Activity 页面对全部 /mcp 流量盲。v24 套件首测即抓到（30 次调用 0 行写入） | 三条返回路径（builtin/isolated/shared）全部插桩 `log_call_activity`（duration/status/args/output/error/source_ip），source_ip 经 Parts 提取；`client_ip_of` 提为 pub(crate) |
| H2 | `services/fts_service.rs` | **日志热路径 O(N)**：`sync_upsert_tx` 每次写日志先 `SELECT rowid FROM fts_app_log WHERE ref_id=?`——UNINDEXED 列全表扫描，app_log 15 天 3 万行，写日志本身成为最慢环节 | 新增 `sync_insert_tx`（纯插入跳过 rowid 查找），add_log（新 UUID 恒新增）切换；可更新表保持 upsert |
| M3 | `commands/runtime.rs:663` | Node 安装「校验失败」分支不清理 dest——重试时复用路径只查文件存在不查健康 → 永久误报已安装 | 失败分支 `remove_dir_all(&dest)` 对齐其他失败路径 |
| M4 | `services/skill_service.rs` | agents 读改写无锁——并发 create/delete agent 整数组覆盖，后写吃前写 | `agents_lock()` tokio Mutex 包住 create/delete_custom_agent |
| M5 | 同上 | export copy 与 uninstall/delete 同路径并发 remove_link vs copy_dir_recursive → 截断安装标记 ok | `skill_fs_lock()` 包住 export_to_agents/uninstall/delete_skill |

## 2. 契约 diff 脚本（R23 教训固化）

`scripts/check_frontend_rust_contract.py`：提取 Rust ServerConfig 字段 × 前端 buildServerPayload 键，camel↔snake 对账，静默丢弃即 FAIL（'type'→server_type 重命名、'oauth' R10 裁定项白名单）。当前 0 mismatch。CI 可直接挂。

## 3. 新套件

`scripts/e2e/rmcp_supplement_v24.py`（12 项）：
- 日志写路径吞吐：60 次 MCP 调用计时 + activity 行数增长（修复后 avg 1ms/调用）
- FTS 一致性：fts_app_log == app_log 行数（30459=30459）
- 2026 特性终验：discover ttlMs=3600000/public、未知版本 -32022、tools/list ttlMs=30000/private
- 宽松回归：4 legacy 版本缺 Accept 放行+精确回显

套件调试沉淀：**会话是 path-local**（root 会话打 scope 通道秒失败——rmcp 行为）；ping 不写活动日志、tools/call 才写。

## 4. 验证结果

| 项 | 结果 |
| --- | --- |
| cargo check --lib | 0 错 0 警 |
| cargo test --lib | **96 passed** |
| tsc | 0 错误 |
| **28 套 E2E** | **1148/1148 全绿**（重建二进制重启后全量重跑） |

## 5. 经验沉淀

1. **「活动日志」是迁移丢失的探测器**：rmcp 大迁移后 UI 可观测面（Activity 页）与协议面脱节——对每个用户可见面写一条「操作→应出现 X」的断言，比只断言协议响应更能抓迁移丢失。H1 潜伏 5 轮、11 套件 1000+ 用例未发现，v24 第一次断言「调用应产生日志」就命中。
2. **热路径的自指性能陷阱**：写日志路径依赖 O(N) 扫描，日志越多写越慢——高频路径的每条 SQL 都要问「这个查询在最大体量下的成本」。
3. **复制粘贴的失败分支**：runtime 安装 4 个失败路径 3 个清理 1 个不清理——失败分支集合要用 grep 列全再逐个对账。
