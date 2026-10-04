# R491+ 复核第二十七轮报告（2026-10-03）

> 范围：修复侵蚀审计（27/27 在位）+ rag/vectordb+extract 全文 + commands/groups+http_server + group_service 重审 + 前端 GroupCard/CopyClientConfigDialog/mcpClientSnippets。3 个独立复核代理。

## 1. 修复清单（3 项）

| # | 位置 | 问题 | 修复 |
| --- | --- | --- | --- |
| M1 | `rag/vectordb.rs ensure_table` | 自愈缺口：`embedding` 列缺失或类型非 FixedSizeList 时 `need_recreate=false`——表被保留，`open` 报成功，之后每次 `add_chunks`/`search` 永久失败（RecordBatch schema mismatch），无自愈路径（tags 列的同类场景却是 recreate） | 两种情形均 recreate（对齐 tags 处理 + 日志说明） |
| M2 | `vectordb.rs read_chunks_by_doc` | embedding downcast 失败 `unwrap_or_default()` → 空 vec 喂给 `add_chunks` 的 `FixedSizeListArray::new`（内部 `try_new().unwrap()`）→ **panic** 打崩索引线程，远离真实成因 | downcast 失败返回 Err；`add_chunks` 增加 `embedding.len() != dim` 前置校验（Err 而非 panic） |
| L-M3 | `http_server.rs maybe_start` | `httpPort` 70000 `as u16` 截断为 4464 静默绑错端口；0 绑临时端口但 UI 报 0 | 范围校验（1-65535），非法值 log_to_db + 不启动 |

**记录不修**：`groups.builtin_prompts/builtin_resources`（v12 迁移列）Rust 侧零读写——完整接线（Group 模型 + CRUD + 组路由过滤）属产品功能决策，非缺陷修复；单列。

## 2. 证伪（三代理合计 19 项假设全排除）

vectordb SQL 注入（全部字面量 + 转义）、pdf 畸形页 panic、office 递归栈溢出（所有权树无环）、BOM/编码、连接泄漏（廉价 handle）、group 重名 FTS 碰撞（name UNIQUE + ref_id=id）、双 bind（handle mutex 串行化）、成员对象丢失、snippet 注入（JSON.stringify 全量转义）、剪贴板资源泄漏、分页 clamp。

## 3. 新套件

`scripts/e2e/rmcp_supplement_v27.py`（3 项）——RAG builtin 工具聚合存活 + `mcphub-desktop-rag_search` 真实检索调用 + 非法 httpPort 不致进程崩溃（DB 直改观测 + 还原）。

## 4. 验证结果

| 项 | 结果 |
| --- | --- |
| cargo check --lib | 0 错 0 警 |
| cargo test --lib | **102 passed** |
| tsc | 0 错误 |
| **31 套 E2E** | **1167/1167 全绿**（重建二进制重启后全量重跑） |

## 5. 经验沉淀

1. **自愈逻辑的不对称就是缺口**：ensure_table 对 tags 缺列 recreate、对 embedding 缺列保留——同函数内两种「列缺失」处理不一致本身就是信号，逐列对账即可发现。
2. **unwrap_or_default 跨边界是 panic 制造机**：静默降级空值会把可恢复的 schema 错误转化为远处构造器的 panic——数据离开本函数前必须满足下游构造器契约，校验放在边界。
3. **配置读取的 `as u16` 截断**：所有 `as` 数值收缩都要问「超范围值会发生什么」——静默错绑端口比拒绝启动危险得多。
