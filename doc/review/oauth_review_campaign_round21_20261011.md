# OAuth 复核战役 · 第二十一轮（R21）报告 — 2026-10-11

> 范围：3 代理切片（commands/ 层 / services/ 二线 / 前端页面层）+ 亲核全量 74 套 E2E + 修复 1 Low（Rust）+ 1 Medium（Rust）+ 6 M + 6 L（前端）。

## 1. 切片结论

### A1 · commands/ 层逐行（23 文件 ~6300 行）— **1 Low**
门控（全部写命令 + 敏感读 require_admin，skipAuth 短路 + fail-closed）、参数校验（validate_version_arg 五入口、LIKE 转义、char_boundary）、子进程（13 处 CREATE_NO_WINDOW 全在位、spawn_blocking 覆盖）、资源上限（32MB 流式 cap、300MB 硬顶）、近期改动（tar 双重逃逸校验、SHASUMS、oauth.rs、rag prune）全部确认干净；165 命令注册对账无孤儿。
- **[L·已修] install_python_version 的 uv 子进程无整体超时**——`child.wait()` 无 timeout 包裹，uv 挂起（半开连接）→ 永久持有 PYTHON_INSTALL_LOCK，python 安装/卸载整体死等直到重启。→ 30 分钟 tokio timeout + 超时 kill + recent-logs 尾巴进错误消息。

### A2 · services/ 二线逐行 — **1 Medium**
bearer_key（常数时间比较/fail-closed/cascade）、http_server REST 尾部（allow-list 403 前置、structuredContent/_meta 条件键、404/403 语义、disabled gate fail-closed、日志脱敏）、market/registry/cloud、FTS 同事务、user_service 事务化、JWT 随机、migration（**事实更正：TARGET_VERSION 已是 33**，v29-v33 全幂等，非 29）全部干净。
- **[M·已修] reconcile_pending 无 skill_fs_lock 执行 FS 删除**（skill_service.rs:1081）——启动窗口与 HTTP server 并发，import/export 持锁写路径可与其 remove_link 竞争（pending 中间态绕过对账）。→ 入口补 `skill_fs_lock()`（该函数不触 agents_lock，无锁序风险）。

### A3 · 前端页面层 — **6 M + 19 L（修复 6M + 6L，余 L 记录）**
**RagPage（4M）**：①`toggleExcluded`/`toggleGroupExcluded` 绕过 `applySelected` → selectedRef 失步，被排除文件在下一次点选时复活进选中集 → 改经 applySelected 写回；②`toggleGroupExcluded` 逐路径翻转语义与按钮方向相反（混合组点「排除本组」反而反转）→ 方向式重写（allExcluded ? 全取消 : 全排除+移出选择集）；③前端 `pathUnderRoot` 缺后端 R18 的 `\`→`/` 归一 + 大小写折叠 → Windows 排除/取消排除静默失效 → 两处副本（RagPage + RagDocTree）镜像后端语义；④`toggleDocsExcluded` 批量取消不补兄弟条目 → 未勾选兄弟一并解除排除 → 对齐单文件路径的 coveredOthers 补条目语义。
**SettingsPage（2M）**：⑤SmartRouting 保存成功即清 dirtyRef，覆盖保存期间键入的编辑 → 编辑序号 epoch（每处 dirty 编辑 +1，保存起点记录、resolve 后序号未变才清）；noChanges 分支也清 flag（原 dirty 卡 true 冻结表单）；⑥installConfig 跨字段草稿互擦（存 npmRegistry 抹 pythonIndexUrl 草稿）→ touched-keys 草稿保护（保存解析后该键交还重建）。
**ServerCard（1M）**：⑦OAuth 第二个 listen() resolve 与赋值之间卸载 → 监听器泄漏过卸载，死卡片仍触发 toast+refetch → resolve 后立即 disposed 检查双监听。
**ServerContext（1M）**：⑧`fetchServers`（常态轮询）无代际守卫 + logout 不 bump 令牌 → 旧页数据回写/登出后数据复活 → 代际捕获 + await 后检查（成功与 catch 双路径）+ logout 先 bump；`fetchInitialData` catch 补 stale 检查；max-attempts 双重轮询去重（对齐成功路径注释语义）。

**L 已修 6 项**：ActivityPage finally 击穿 seq 守卫 + handleCleanup 不 bump seq；GroupCard 复制双击定时器互吞；ServerForm `quoteArg('')` 丢空串参数（`["-e","","code"]` 编辑后持久化丢 `""`）→ 空串产出 `""`；openapi `passthroughHeaders.join` 无守卫崩整个表单（legacy 字符串）→ Array.isArray 守卫。
**L 记录不修（12 项，UX/低危）**：UploadDialog 折叠重置、删除双击双 toast、maxBufferSize 无 blur clamp、保存按钮 in-flight、RuntimeVersionManager 监听器 disposed、/users 无 seq、设备切换无回滚、useGroupData seq、GroupsPage cost 双触发、fetchInitialData 放弃路径残留、removeServerFromGroup 未编码（零调用方）、 GroupsPage 刷新定时器（React 18 无害）。

## 2. 全量 E2E（亲核，74 套逐一执行）

- 期间服务器进程被外部终止一次（日志无 panic，非代码崩溃）→ 重启后复跑：**全部 74 套 ~2000+ 断言全绿**（含 2 个因中断残留状态的瞬时失败，复跑即绿）。
- 修复后复验（Rust 重建 + 前端 build）：full_matrix_v18 262/262 + public_ip_matrix 10/10 + r109 32/32 + r1600/r1700 全绿 + 契约脚本 0 mismatch。
- cargo check 0 错 0 警；cargo test --lib **152 passed**；tsc 0 错误；npm build ✓。

## 3. 「什么时候是个头」——R21 答案

R20 全量 74 套绿 + 3 切片仅 1 Low 之后，本轮换**未覆盖切片**（commands/services 二线/前端页面层）立刻出 1M+13 前端 M/L——**结论明确：每次切换到未扫过的切片就会出新一茬，这是「覆盖面」问题不是「质量」问题**。协议核心面（http_server/bridge/transport/pool，R458-R490 已全量逐行）连续 4 轮零 High/Medium；前端页面层与 services 二线是本轮才首次达到同等逐行深度。按此速率，剩余未深扫面为零（commands/services/前端三线本轮补齐）——**下一轮若仍换切片将无切片可换，收敛应已在眼前**。

## 4. 记录不修
- migration TARGET_VERSION 文档口径：AGENTS.md §3.4.2 记 29，实际代码 33（v30-v33 为 OAuth 战役期间新增，AGENTS 未逐版更新——事实更正记录，代码正确）。
- 其余见 §1 L 记录项。
