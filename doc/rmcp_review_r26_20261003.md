# R491+ 复核第二十六轮报告（2026-10-03）

> 范围：修复侵蚀审计（29/29 在位）+ **R20-R25 修复点自我复核**（修复合自身引入的新 bug）+ subscription_hub/mcp_tasks 全文重审 + 前端 contexts/pages。3 个独立复核代理。

## 1. 修复清单（4 项，全部为既往修复合引入/伴生的回归）

| # | 位置 | 问题 | 修复 |
| --- | --- | --- | --- |
| R1 | `prompt_service.rs render_template` | **R25 修复合引入**：空占位符 `{{}}`（end==2 < 3）落入「无闭合括号」分支 → 整个剩余模板 verbatim 输出，后续所有 `{{x}}` 不再替换（顺序 replace 旧实现无此问题） | `Some(2)` 独立分支：保留字面量并前进；+1 单测 `render_empty_placeholder_does_not_kill_rest` |
| R2 | `SettingsPage.tsx` | **R25 修复合伴生**：dirty flag 无条件清——`updateSmartRoutingConfigBatch` 失败返回 false 不 throw，保存失败仍清保护 → 下一次立即写开关清掉失败草稿 | 仅 `ok !== false` 时清 flag |
| R3 | `ActivityPage.tsx fetchData` | 无乱序守卫：快速翻页/切筛选并发请求，旧响应后到覆盖新页数据（表内容与分页器脱节） | `fetchSeqRef` 单调 id，await 后 seq 不匹配即丢弃（含 error 路径） |
| R4 | `ServerContext.tsx fetchInitialData` | 启动轮询间隔 < 后端延迟时 tick 重叠：晚到的 tick 在他人已切换 normal 后再次 `startNormalPolling`（替换定时器）+ 旧 attempt 数据覆盖新状态 | 闭包内 `phase` 令牌：成功切换前置 'normal'，入口处 phase 不符即 bail |

## 2. 证伪（两代理合计 17 项假设全排除）

任务 sweep×get 竞态（终态保留 ≥5min）、ttl=0 语义、终态 map 无界（sweeper 双轴清）、订阅者泄漏（is_closed prune）、hub 锁跨 await（全同步段）、通知乱序、tasks/list cursor（spec 允许 null）、终态 cancel（-32602 对称守卫）、Failed 无 error（同锁原子）、skill 锁死锁（两独立锁无嵌套）、fts 双插（upsert 无 AppLog 调用点）、delete 二次 disconnect（map remove None 无害）、UTF-8 切片安全、args.clone 9MiB 成本（64KB 截断入列）等。

## 3. 新套件

`scripts/e2e/rmcp_supplement_v26.py`（12 项）——全协议快照终验：
- 4 legacy 版本 × root 通道公网IP 真实调用 + tools/list 形状（inputSchema 102/102）
- 2026 modern 直达公网IP
- 严格/宽松热切换基线（规范请求两模式均通过）
- prompts 渲染链回归（R25/R26 修复后无占位残留）

## 4. 验证结果

| 项 | 结果 |
| --- | --- |
| cargo check --lib | 0 错 0 警 |
| cargo test --lib | **102 passed**（+1 `{{}}` 边界单测） |
| tsc | 0 错误 |
| **30 套 E2E** | **1164/1164 全绿**（重建二进制重启后全量重跑） |

## 5. 经验沉淀

1. **修复合要自我复核**：R25 的单遍扫描器引入 `{{}}` 回归、dirty flag 引入失败清保护——每个修复合在一轮后必须以「它自己会不会引入新 bug」视角重读。本轮 4 项发现全部属于此类。
2. **分支穷尽于边界值**：`end >= 3` 的推理对非空名正确，但对 end==2 的空名场景只考虑了「非法」而没考虑「出现」——match 分支要列全输入域再写兜底。
3. **前端乱序是默认态不是例外**：所有 `useCallback + useEffect` 的 fetch 链都应有 seq/abort 守卫——本轮 3 个 fetch 面中 2 个缺。
