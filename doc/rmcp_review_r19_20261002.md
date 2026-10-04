# rmcp 复核第十九轮（2026-10-02 下午，R471-R490：5 代理全文件逐行 + 清理审计 + 亲核修复 + v19 套件 + 全量回归）

> 与第十八轮互补：本轮不再扫 diff，而是**全文件逐行**读此前只按 diff 复核过的面（pool/client/session_pool、lib/tray/db/auth、smart_routing 全部 6 文件、runtime_env+runtime 全 2450 行），另加迁移/清理完整性专项审计。

## 一、迁移与清理完整性审计（独立代理，全绿）

- 旧 dispatch 全家桶零残留：`dispatch_mcp`/`mcp_version`/`SESSION_STRATEGY`/`sse_channels`/`jsonrpc_response`/`strategy_for` 全仓 0 代码命中（仅 1 处历史注释）。
- 上游传输全 rmcp 化：stdio=RmcpStdio、streamableHttp=RmcpHttp、openapi=rmcp-openapi 单栈、sse=手写（有据保留）。
- **158 个 Tauri command 全部注册**，无孤儿；前端最新 14 条映射逐一有 invoke 对应。
- `#[allow(dead_code)]` 全仓 6 处逐一判定合理；`cargo check` 0 警告。
- 26 个 `migrate_vN` 与 `TARGET_VERSION=26` 一致；`migrations/*.sql` 系历史快照（运行时不读取）→ 补 README 钉死口径。

## 二、修复清单（9 项）

| # | 级别 | 位置 | 问题 | 修复 |
| --- | --- | --- | --- | --- |
| 1 | **Medium**（macOS 主退出路径） | `tray.rs` | 原生菜单 Quit（macOS ⌘Q/Windows File→Quit）用 `PredefinedMenuItem::quit` —— muda 源码实锤直接 `terminate:`/`PostQuitMessage`、**永不发 MenuEvent**，`on_menu_action("quit")` 的 `disconnect_all` 永不执行 → MCP npx/uvx 子进程树孤儿化 | 两平台改自定义 `MenuItem::with_id("quit")`（同 id 走统一 handler） |
| 2 | Medium | `commands/runtime.rs` | 复制的 `get_unix_path` 过滤器仍拒含空格 PATH 项（runtime_env 同源 bug 已修此处未同步）+ 缺 `=` 守卫 → 检测与真实 spawn 行为不一致 | 谓词对齐（空格合法、拒 `=`） |
| 3 | Medium | `commands/runtime.rs` | `get_enhanced_path` 无缓存：每次 list 命令同步 5s shell 探测阻塞 async worker | 优先读 `runtime_env::cached_enhanced_path()`（init 时已探测），调用点包 `spawn_blocking` |
| 4 | Medium | `commands/runtime.rs` Node 下载 | 累计无上限（伪造流式 body → OOM）+ **无任何校验和**（node 二进制将执行任意 MCP 代码，仅靠 TLS） | 300MB 硬顶 + 下载后拉 `SHASUMS256.txt` 校验 SHA-256（不匹配硬失败；SHASUMS 不可达响亮 warn 放行）；新增 `sha2`/`hex` 直依 |
| 5 | Medium | 全仓 6 处 | Windows `CREATE_NO_WINDOW` 缺失：tray `cmd /C start`、rag/service explorer×2、ocr tesseract×3、skill_service explorer —— 每次动作弹黑框 | 6 处补 `creation_flags(0x0800_0000)` |
| 6 | Low | `mcp/session_pool.rs` | 慢路径在 disable/delete 与 connect 完成竞态窗口内无条件 insert → 孤儿隔离 client 存活至 session 删除且继续服务死服务器 | insert 前重查 enabled |
| 7 | Low | `smart_routing/store.rs::keyword_search` | SQL 侧 `limit` 无 ORDER BY → 命中超 cap 时喂给 merge_hits 的子集不确定（同查询不同结果） | fetch cap 提至 10k，merge_hits 确定性剪枝 |
| 8 | Low | `db/migration.rs` | v2/v5/v6/v7/v11 的 ADD COLUMN 裸 `.ok()` 吞错（v10 教训自相矛盾：版本推进但列缺失永久失效） | 全部换 `add_column_if_missing` |
| 9 | Low | `migrations/` | 孤儿 .sql 工件误导（仅 12/26 有文件，运行时不读） | README 钉死「migration.rs 为唯一权威」 |

记录不修：`run_pending` 每步与 set_version 非原子 + v8 非幂等重跑风险（需 26 签名重构或事务化，窗口=恰好 v8 执行中崩溃，极窄）。

## 三、新套件 `rmcp_supplement_v19.py`（87 项）

- **bearer × 5 版本 × 4 通道**：无 token/错 token → 401（initialize 也受控）、带 key 全 lifecycle 公网IP（root/scope/group/REST//api）、REST 无 key 401；
- **prompts/resources × 5 版本**：list 形状、prompts/get 真实渲染（messages 非空）、resources/read 真实读取、未知 prompt -32602；
- **tasks 生命周期**：legacy（tasks/list 200；task-directed 创建 → -32021 = **rmcp ext-only 设计钉死**：rmcp 3.4.1 将 tasks 仅建模为 2026 extension，legacy core `capabilities.tasks` 不被识别）；2026 全链（CreateTaskResult 平铺形状、tasks/get 扩展形、轮询至 completed 内嵌 result、终态 update/cancel -32602 响亮失败、working 态协作 cancel + 终态复核、未声明扩展永不同步任务）；
- **subscriptions/listen**：2026 SSE ack（acknowledged+subscriptionId）+ 空 filter；legacy → -32601（listen 是 2026-only，legacy 走原生 GET SSE——设计钉死）；
- **group 边界**：非成员工具不泄漏（按 DB 真实成员判定前缀归属）、不存在 group 非 5xx、group 通道公网IP；
- **on-demand 唤醒**（环境无 on-demand 服务器时显式跳过）。

套件编写中钉死的坑（后续轮次必看）：`InputResponses` 是 **Map**（BTreeMap<String,Value>）不是数组；cancel ack 无状态注入 `resultType:complete` 需以 tasks/get 复核终态；`subscriptions/listen` 与 task-directed 创建均要求 per-request `_meta` ext 声明。

## 四、验证

- **23 套 E2E 1042/1042 全绿**（+v19 87 项）；
- `cargo check --lib` 0 错 0 警；`cargo test --lib` 92 passed；`npx tsc --noEmit` 0 错误；
- 全部修复在重建二进制上验证（tray 菜单/CREATE_NO_WINDOW 为 Windows/macOS 行为级，逻辑走查 + 编译验证）。

## 五、为什么每轮仍有产出（结构分析，诚实回答）

1. **只按 diff 复核 ≠ 全文件复核**：本轮全文件重读 pool/session_pool/runtime_env 才暴露跨文件一致性问题（runtime.rs 的复制体没跟上 runtime_env 的修复）。教训：**每次修复同类问题时必须全仓 grep 所有复制体**（本轮 CREATE_NO_WINDOW 就是全仓 grep 抓出 6 处漏网）。
2. **生命周期边界只在真实退出/取消路径暴露**：⌘Q 孤儿进程、in_flight 取消悬挂都是「正常路径永远测不到」的分支——需要专门的框架级知识（muda 预定义菜单行为）而非跑用例。
3. **rmcp 升级带来语义重定义**（tasks 仅 ext、InputResponses 是 Map）——文档写于旧实现时，修复前必须以 vendored 源码 + 既有套件实测双重定谳。
4. 收敛趋势：本轮 0 High、4 Medium（全部为「首次覆盖的面」或「跨文件失同步」，无新增协议层缺陷）；23 套件 1042 断言已把协议矩阵机器化，回归面封闭。
