# OAuth 复核战役第十三轮报告（2026-10-10）

## 扫描面
1. **r13-a1-fixes**（code-review）：round-12 修复合复审（git diff 亲验当前树——防 clobber 复发）。
2. **r13-a2-frontend**（code-review）：近期未深读页面全量逐行（ServersPage/ServerCard 非 OAuth 面/GroupCard+表单/MarketPage/Dashboard/ActivityPage/RagPage 高频区/LogViewer）。
3. **r13-a3-tests6**（general-purpose）：新套件 `rmcp_round_r1300.py` 46 用例 + 单测 +5。

## 修复 7 项（1 Critical + 2M + 3L + 1L 自纠）

### Rust
- **[Critical·round-12 修复自身引入] agents_lock 自死锁**：round-12 给 save_agents 无条件加锁，但 create/delete custom agent **已持锁**调用它——tokio Mutex 不可重入，两个 Tauri 命令首次调用即永久挂死并堵死后续全部 agent 操作。修复：拆 `save_agents_locked`（约定调用方持锁）——create/delete 走 locked 变体，外部命令路径走加锁包装；grep 确认唯一外部调用点（commands/skills.rs）无锁持有。
- **[L·自纠] any_description_override 探针与过滤器对称性方向搞反**：round-12 让过滤器跳过空串覆写、却把探针的 `!= ''` 子句删了——对称性应保持（空串=无覆写）。恢复子句。
- 单测 +5（a3，锚点式插入无整文件写回）：get_strict 三态（NULL→Ok({})/合法 round-trip/腐坏→Err）、pool connect-lock 共享/forget 语义。

### 前端（a2 全部修复）
- **[M] ServerCard 端点复制未编码**：`${baseUrl}/mcp/${server.name}` 原样插值——CJK/空格名复制出不可用 URL（GroupCard 同路径早有编码+注释，同族未同步又现）。修复：encodeURIComponent ×2 对齐。
- **[M] 批量标签无错误处理**：循环 await setTags 无 catch——第 N 个失败时前面已改、rejection 悬空、零提示、对话框假成功悬留。修复：try/catch + 已完成计数 toast + 不关闭对话框。
- **[L] 排除规则持久化失败静默**：console.warn 吞掉——用户排除项下次 update-check 静默回滚。修复：非阻塞错误 toast。
- **[L] 清空活动无反馈**：success:false 与 throw 都只 console.error。修复：双分支 toast。
- **[L] Registry 已安装态仅会话内**：跨会话重装撞重名报错 toast。修复：初始集合由真实服务器列表 slugify 播种。
- locales +2 键 ×4（batchTagsPartialFailed/exclusionPersistFailed）。

## r13-a3 新套件（46/46）
- **A**：CJK 服务器/临时中文组 `Test-中文-13` × 3 版本真实 IP 调用（组行 finally 删除零残留）。
- **B**：JSON-RPC id 边界（string/超大/负 round-trip、float→202 SSE 流、null→202 空、重复 id 双应答）。
- **C**：重复 initialize→200 新协商（rmcp 行为打印）、DELETE→复用 4xx、cancelled 未知→2xx、batch→415 清洁。
- **D**：tasks 双代矩阵——legacy task-directed → -32021（rmcp ext-only 设计，与既有套件文档一致）、2026 无扩展→同步、有扩展→CreateTaskResult→轮询→result 真实 IP、终态 cancel/未知 update → -32602。
- **E**：prompts 真实渲染零 `{{` 泄漏、resources/read 远程 png 200、$smart prompts/resources 空 contract。
- **F**：宽松 chaos——SSE-only Accept 200、charset 200、CRLF 200、BOM 415、纯空白 415（全 4xx 带无 5xx）。

## 验证
- cargo check 0 错 0 警；cargo test --lib **146 passed**；tsc 0 错误。
- 全量 **63 套 E2E 零失败**（含 r1300 46、r1200 45、r1100 67、r1000 71 复跑）。

## 复盘：为什么每轮总能跑出问题
1. **修锁类缺陷前必须画「谁已持有」图**：round-12 的死锁是把「无锁路径缺锁」当成「该函数该有锁」——加锁修复的正确形态是区分外部入口（加锁）与内部约定（locked 变体）。修复并发缺陷时先枚举同锁全部持有者再决定锁放在哪一层。
2. **对称性修复的方向要先问哪边是权威语义**：round-12 探针删子句是把过滤器对齐到探针（错），正确方向是把探针对齐到过滤器（本轮纠正）——「让 A 靠拢 B」前先判断谁是契约方。
3. **同族 UI 代码的编码/校验/反馈差异是稳定缺陷源**（本轮 5 个前端修复中 3 个是「GroupCard 有、ServerCard 没有」类）：横切 UI 约定（编码、错误 toast、状态播种）值得一份 checklist 化的专项扫描，靠逐轮 review 兜底效率低。
