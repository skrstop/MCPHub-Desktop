# rmcp 复核第三十五轮（R35，2026-10-03）：$smart 关键词重复词评分失真 + CONNECT_LOCKS 无界泄漏

> 复核范围：mcp/pool.rs + smart_routing/{search,store,index}.rs 全量重读（多轮修复后最新状态）；
> LogViewer / UpdateCheckContext / ServerInstallProgressContext / PromptsPage / ResourcesPage / Markdown / LoginPage（前端）。
> 方法：侵蚀审计全在 → 2 个并行逐行代理 → 亲核验证 4 项 → 修复 → v35 套件 → 全量回归。

## 修复（4 项：Medium×2，Low×2）

### S1 [M] $smart 关键词评分对重复查询词失真
`matched` 按词计不按去重词计——查询 "deploy deploy"（重复词）时两词都命中同一文档 →
matched=2/2=1.0，与全匹配 "deploy things" 的文档同分：部分匹配绕过一切
score_threshold，排序失真。docstring 声称「matched query terms / total query terms」
而代码对重复输入不实现该契约。**修复**：BTreeSet 去重后按 distinct 计分。

### F1 [M] ServerInstallProgress 终态清除定时器删除新下载条目
`done`/`error` 后 1.5s 清除条目——定时器只防终态→终态：`done` 后 1.5s 内新的
`downloading`（重连/重装）进来，定时器照样把它删掉 → 进度条中途消失不回来。**修复**：
非终态事件到达时先 clearTimeout。

### P1 [L] CONNECT_LOCKS 无界增长
per-name 连接锁条目从不删除（delete/rename 均不清）——长期会话按名字累积进程生命周期
。**修复**：新增 `pool::forget_connect_lock`，server_service delete 与 rename 旧名分支
调用。**不挂在 disconnect 上**：并发 connect 持 Arc 期间移除条目会让新 connect 铸新锁
并行运行，破坏 per-name 串行化——这是修复时要想清楚的边界。

### F2 [L] LoginPage loadAuthProviders 未处理 rejection
getPublicConfig 失败（后端不可达，恰是 isServerUnavailableError 的场景）→ unhandled
rejection。**修复**：catch 兜底重置 socialProviders 默认值。

## 清洁
- pool.rs 连接守卫覆盖 check→insert→终态（无重入覆盖）、disconnect/connect 同锁无复活
  竞态、无锁跨 await
- smart_routing store/index：esc/esc_like + ESCAPE 全应用、EMBED_WRITE_LOCK 无重入死锁
  、ghost 对账含 builtin
- LogViewer（搜索竞态守卫）、UpdateCheckContext（cancelled-flag 监听器模式）、
  Markdown（无 raw HTML、urlTransform 剥 javascript:）、Prompts/Resources 页（reqId 守
  卫 + safePage 收敛 + hooks 全无条件）——零缺陷

## 新套件 rmcp_supplement_v35.py（9 项）
- 4 legacy 回显 + **$smart REST 重复词查询端到端**（dedup 修复的协议面验证：/api/$smart
  /search?query=ip%20ip%20ip 响应形状断言）+ 公网 IP ×2（2025-06-18 + 2026 stateless）
- 2026 discover（ttlMs 3600000）+ /health
- 套件调试过程：GET 参数名是 `query` 非 `q`（对齐既有套件）；响应是 MCP content 包装
  内嵌 JSON

## 验证
- `cargo check --lib` 0 错 0 警；`cargo test --lib` 102 passed；cargo build ✓
- `tsc` 0 错误；`npm run build` ✓
- 39 套件 **1243/1243 全绿**（v35 9/9 新计入；重建二进制热跑）

## 教训
- **计分/统计公式与 docstring 的契约要逐条对照**：S1 的公式对「无重复输入」正确、对
  重复输入静默失真——docstring 是契约，代码是实现，两者漂移就是缺陷。
- 资源清理点必须选在「无并发持有者」的位置：forget_connect_lock 挂 delete/rename 而非
  disconnect，因为 disconnect 期间并发 connect 持 Arc，移除条目会让新 connect 铸新锁
  并行运行——修复本身能引入更糟的竞态（R26 教训的延续）。
