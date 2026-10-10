# rmcp 复核第三十三轮（R33，2026-10-03）：Sidebar 条件 Hook（High）+ stdio 更新检查未处理 rejection

> 复核范围：commands/bearer_keys/groups/config/logs/market/registry/cloud + auth + db/mod（Rust）；
> ServersPage / Header / Sidebar / UserProfileMenu / StatusDot（前端）。
> 方法：侵蚀审计全在 → 2 个并行逐行代理 → 亲核验证（含 od -c 字节级证伪）→ 修复 2 项 → v33 套件 → 全量回归。

## 修复（2 项：High×1，Medium×1）

### F1 [H] Sidebar 条件 Hook 调用
`const userCanManageUsers = auth.user?.isAdmin && usePermissionCheck('x')` —— Hook 被
短路条件包裹。auth.user 首渲染为 null（auth 请求未返回），resolve 后翻转为 admin →
Hook 调用顺序在两次渲染间改变 → React 抛 "Rendered more hooks than during the
previous render" **卸载整棵组件树**。触发路径必然发生（admin 用户首次加载 Sidebar 必
挂载期间经历 null→值 翻转）。**修复**：无条件调用 Hook，布尔组合放在 Hook 之后。

### F2 [M] stdio 更新检查 try/finally 无 catch
`handleCheckStdioUpdates` 中 `apiPost` 失败会 **throw**（fetchInterceptor），try/finally
无 catch → unhandled promise rejection + 用户零反馈（spinner 静默停止）。代码自定义的
错误 toast（`result?.message || t(...)`）只覆盖 `success:false` 响应路径，throw 路径全
部绕过。**修复**：补 catch 分支（同款错误 toast）。

## 证伪（2 项，同一伪影家族第 4/5 次）
- cloud.rs `format!("******", api_key)`（代理报 High：Authorization 发字面量星号）——
  **od -c 字节级核验为终端脱敏伪影**，实际源码 `format!("Bearer {}", api_key)` 完好。
  这是 read-path 打码第 4 次误报（R14 openapi_transport、R22 git.rs、R25 前端 snippet
  之后）。**所有含 `******` 的代理发现必须 od -c 定谳后才可修**——修复伪影会真损坏源
  码。
- bearer_keys.rs `"Bearer key '{}' not found"` 同伪影，字节核验完好。

## 清洁
- bearer_keys/groups/config/logs/market/registry/cloud 七命令文件零缺陷（registry 的
  content_length 与 bytes 间隙有 body.len() 后置检查兜底）
- auth（每进程随机 JWT secret、HS256-only 无 alg 混淆）、db/mod（含空格路径经 sqlx
  FromStr 验证可解析、pool OnceLock 时序正确）
- Header/UserProfileMenu/StatusDot 零缺陷；ServersPage 既有修复（搜索竞态守卫/B1
  守卫/safePage）全部完好

## 新套件 rmcp_supplement_v33.py（9 项）
- 4 legacy 回显 + 公网 IP ×3（2025-06-18 / 2025-11-25 + 2026 stateless，IPv4 Body 断言）
- 2026 discover（ttlMs 3600000）+ /health

## 验证
- 本轮 Rust 零改动（两项修复均前端）→ 无需重建二进制；`tsc` 0 错误；`npm run build` ✓
- 37 套件 **1225/1225 全绿**（v33 9/9 新计入）

## 教训
- **条件渲染的 Hook 调用是 React 的「看起来能跑」陷阱**：`cond && useXxx()` 在条件
  恒真/恒假的开发环境全绿，真实用户流（auth 异步解析）必翻车——Hook 规则 lint 之外，
  代理扫描指令已加入「Hook 调用点必须无条件」家族。
- `try { throw } finally {}` 不是错误处理：finally 只清状态，rejection 仍悬空。凡
  await 可 throw 的调用必须有 catch 或由调用方保证捕获。
- 伪影纪律已固化为流程：代理报「源码含 `******`」→ 一律 od -c 字节核验 → 完好则记
  证伪不修。本轮两次 High 级误报若直接修复会真损坏 Authorization 头。
