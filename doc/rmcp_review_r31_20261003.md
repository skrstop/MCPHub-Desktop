# rmcp 复核第三十一轮（R31，2026-10-03）：删除中连接 fail-open + RagPage 排除/选择三连缺陷

> 复核范围：mcp_manager / config_service / progress / app_logger / server_tool_config_service / time.rs（Rust）；
> RagPage.tsx 全 5800 行（前端）。
> 方法：侵蚀审计全在 → 2 个并行逐行代理 → 亲核验证 6 项 → 修复 → v31 套件 → 全量回归。

## 修复（6 项：High×3，Medium×1，Low×2）

### R1 [M] enable-path 重检 fail-open → 删除中的服务器永久在线
`toggle_server` spawn 的连接任务完成后重检 DB：行已删（连接期间删除）→
`Ok(None).unwrap_or(true)` =「仍启用」→ 不断开。结果：活着的 MCP 进程无 DB 行，
session-rebuild 不碰它（list_all_enabled 不返回）、HTTP scope 过滤不查 DB——直到重启
无人回收。disable 分支（181 行）同场景 `unwrap_or(false)` 正确——**同函数内两个分支
对同一「行缺失」语义相反就是缺陷信号**。`Err(_) => true` 同样 fail-open。**修复**：
两个 arm 都改 false（fail-closed，DB 抖动时断开比留孤儿进程安全）。

### F1 [H] persistExclusionDiff 永远无法持久化「取消排除」
`removed = [...excludedBase].filter(p => !excludedUnion.has(p))`——excludedUnion =
base ∪ local，是 base 的**超集**，差集恒空 → removed 恒 []。用户在弹框内点击已持久化
排除项取消排除，UI 显示已取消、注册表永远不更新（base 在弹框内被原地删过元素，对它
diff 无意义）。**修复**：新增 `excludedOriginalRef` 保存打开弹框时的注册表原始快照
（不可变参照），removed 对它 diff。

### F2 [H] 排除的文件仍被上传
`selectedPickedFiles` 只按 `sel.has(f.path)` 过滤——排除某文件不移出选择集：行渲染
灰了、组计数减了，但上传管线仍把它送进去（用户明确标「不导入」的文件被静默导入）。
且 useMemo deps 缺 excludedUnion——null 哨兵未物化时切换排除根本不重算。**修复**：
filter 加 `!excludedUnion.has(f.matchPath || f.path)` + deps 补 excludedUnion；
toggleExcluded 同时从 selectedPaths 移除该路径（双保险）。

### F3 [H] toggleGroupExcluded 逐个 toggle 读陈旧闭包
`for (p of paths) toggleExcluded(p)`——循环内每次读渲染闭包的 excludedBase/local
快照，整集合替换式 setState：取消排除 {p1,p2} 时第 2 次迭代用旧快照覆盖第 1 次的删除
→ p1 复活，只有最后一个文件真正取消。**修复**：单快照批量计算 nextBase/nextLocal
后各 set 一次；全取消分支同步清 selectedPaths。

### L1 [L] OCR 预检提示重复渲染两次（copy-paste 残留）→ 删一块。
### L2 [L] git-clone/preview 进度监听器在 unmount 早于 listen() resolve 时泄漏
→ disposed flag：cleanup 置位，.then 内已 disposed 立即 f() 注销。

## 清洁
- config_service（BEGIN IMMEDIATE 事务/合并/fail-closed 全对）、progress.rs（字节切
  割 ASCII 安全）、app_logger（截断循环终止性正确）、server_tool_config_service、
  time.rs——零缺陷
- session-rebuild 双连接竞态已被 per-name mutex + starting 重入检查防护（核过）
- RagPage 的 docReqId/reqId 竞态守卫、全部 Set 更新、applySelected 模式——均验证无恙

## 新套件 rmcp_supplement_v31.py（9 项）
- 4 legacy 版本回显逐版本断言（重建后回归）
- 真实公网 IP 调用 ×3（2025-06-18 / 2025-11-25 + 2026 stateless，IPv4 Body 断言，
  2026 走 SEP-2243 base64 Mcp-Name）
- 2026 discover（resultType/supportedVersions×5）+ /health
- 注：R1 属竞态时序（连接期间删除），E2E 难以确定性构造，由 fail-closed 语义 + 代码
  审查保证；RagPage 三项为 UI 内部状态流，由 tsc/build + 修复逻辑正确性保证

## 验证
- `cargo check --lib` 0 错 0 警；`cargo test --lib` 102 passed；cargo build ✓
- `tsc` 0 错误；`npm run build` ✓
- 35 套件 **1206/1206 全绿**（v31 9/9 新计入；重建二进制热跑）

## 教训
- **同函数内两个分支对同一缺失语义处理相反（unwrap_or(true) vs unwrap_or(false)）
  是 fail-open 缺陷的最强信号**——R1、R27 vectordb 不对称 recreate、R20 组 allow-list
  两 face 不一致，全是这个模式。
- 差集/并集方向要想清楚再写 filter：union ⊇ base 时 `base.filter(!union.has)` 恒空
  ——一行类型推理就能避免一个「永远无法持久化」的静默缺陷。
- 循环内调用读取渲染闭包 state 的 setter 函数 = 每次迭代用同一陈旧快照 → 批量状态
  变更必须单快照计算后一次提交。
