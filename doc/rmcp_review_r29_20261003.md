# rmcp 复核第二十九轮（R29，2026-10-03）：tray 菜单监听器累积 + ServerContext 跨代际竞态 + 围栏注入

> 复核范围：rag/extract 全 4 文件、lib.rs、tray.rs（Rust）；ServerContext 正常轮询路径、RagDocTree、FileTypeRenderer、fileIcon（前端）。
> 方法：侵蚀审计（20+ 修复簇全在）→ 2 个并行逐行代理 → 亲核验证 → 修复 → v29 套件 → 全量回归。

## 修复（5 项：Medium×2，Low×3）

### T1 [M] tray.rs 全局菜单事件监听器累积
`rebuild_menus` 每次调用（init + 每次语言切换 `set_menu_language`）都
`app.on_menu_event(...)`——Tauri 2 的该 API **追加**监听器不替换。切 3 次语言后点一次
「检查更新」触发 4 次 `updater://check-update`（About 弹框叠层、重复网络请求）；quit
路径 `disconnect_all()` + `exit(0)` 并发 N 次。注释里"safe to register repeatedly"前
提错误。**修复**：`MENU_HANDLER_REGISTERED: AtomicBool` 门控，全局 handler 只注册一次。

### T2 [L] resolve_lang 的 localStorage 探针是死代码
setup 时主窗口导航未提交，wry `eval_with_callback` 走 pending-scripts 分支**丢弃回
调** → `rx.recv()` 恒 Err → 启动菜单语言恒 "en"（覆盖链实际是 override → en）。前端
`set_menu_language` 启动调用已覆盖真实语言。**修复**：删除探针块（消除主线程阻塞
webview 回调的脆弱模式），注释钉死原因。

### F1 [M] ServerContext 陈旧 effect 代际闭包覆盖轮询定时器/页码
effect 依赖含 `currentPage/serversPerPage/isInitialLoading`——启动中改页码会重建整个
effect，但旧 run 在飞的 `fetchInitialData` 闭包的 `phase` 守卫只在单次 invocation 内
生效：旧请求后到 → 用**旧页参** setState + `startNormalPolling({immediate:false})`
（旧闭包）→ `clearTimer()` 杀掉新 run 的 startup interval、装上钉死旧 page/limit 的
轮询，且可能永不自愈。**修复**：`pollRunRef` 单调代际令牌——effect body 递增并捕获
`myRun`，异步续体（含 maxAttempts 分支）在触碰 state/timer 前 `isStaleRun()` bail。

### F2 [L] 启动成功路径双重首轮请求
成功路径 `setIsInitialLoading(false)` 触发 effect 重跑（isInitialLoading 是 dep）→ 新
run `!isInitialLoading` 分支 `startNormalPolling()` immediate——与旧 run 成功路径的
`startNormalPolling({immediate:false})` 背靠背两对相同请求。**修复**：成功路径不再起
轮询（只翻 isInitialLoading），由重跑统一接管。

### F3 [L] FileTypeRenderer 代码围栏注入
`'```' + lang + '\n' + content + '\n```'`——内容含 ``` 行（嵌入 markdown、docstring）
时外层围栏提前终止，剩余内容按任意 markdown 渲染。**修复**：扫描内容中最长反引号
run，围栏长度 = max(3, run+1)（编辑器标准做法）。

## 证伪 / 清洁
- rag/extract mod/pdf/office/text 四文件清洁（错误哨兵一致、字节切片边界安全、
  spawn_blocking 正确）
- lib.rs 清洁（DB init 通道处理、close-to-tray cancel、open_external_url 校验）
- RagDocTree 清洁（Set 全部不可变更新、imperative handle 无 stale 闭包、key 无碰撞）
- fileIcon 清洁

## 新套件 rmcp_supplement_v29.py（10 项）
- 4 legacy 版本回显逐版本断言（重建后回归）
- **真实公网 IP 调用 ×4**（3 legacy 版本各一次 + 2026 stateless 一次，Response Body
  IPv4 断言；2026 走 SEP-2243 base64 Mcp-Name 头）
- 2026 discover（resultType/ttlMs 3600000）
- /health

## 验证
- `cargo check --lib` 0 错 0 警；`cargo test --lib` 102 passed；cargo build ✓
- 前端 `npx tsc --noEmit` 0 错误；`npm run build` ✓
- 33 套件 **1190/1190 全绿**（v29 10/10 新计入；重建二进制热跑）
- 公网 IP 断言真实返回（legacy ×3 + 2026 stateless）

## 教训
- 「safe to register repeatedly」类注释必须对照 SDK 源码验证语义（追加 vs 替换）——
  本轮 High 级别用户可见 bug 的根因就是一条想当然的注释。
- effect 内的 phase/flag 守卫只保护单次 invocation；deps 变化重建 effect 时，在飞异步
  续体必须用 ref 代际令牌跨 run 拦截（与 R26 phase token 同类 bug 的第二形态）。
