# OAuth 复核战役 第八轮（2026-10-09 晚，2 复审代理 + 1 补测试代理）

## 1. 覆盖面
- **a1**：第 7 轮全部修复 fix-of-fixes（mark_conn_loss 变体/refresh_owner replay/pkg_locked/body cap/中间件覆盖/.meta 守卫/tray/mv）——8 项 7 项完全正确。
- **a2**：rmcp_bridge 2300 行 + smart_routing 全 6 文件全文重读（conn-loss 交互、pin、meta 三 lane）。
- **a3**：**补测试**——+4 Rust 单测、新 E2E 套件 rmcp_round_r800.py。

## 2. 修复清单（1H + 2M + 3L + 1I）

### a2（bridge/smart —— 本轮核心）
1. **[H] $smart search 暴露未 pin 工具但所有 call lane 全拒** —— 搜索→describe→call 广播工作流在 search 自己推荐的工具上死锁（search 只按 allow-list+group 门控，无 pin 过滤；call 两 lane 走 resolve_pinned_target）→ **四面 parity**：handle_search_tools/handle_describe_tool 增加 pinned 参数，bridge 按 scope 计算 pinned set 下传；MCP $smart 面 search/list/describe/call 一致（REST 面保持既定「search=call=全量」语义并补注释）。
2. **[M] smart_route_call 丢请求级 _meta** —— handle_call_tool 走 pool::call_tool 无 meta，MRTR inputResponses 无法经 meta lane 恢复（直呼 lane 正常，与 bridge 自己的 parity 注释矛盾）→ threading upstream_meta → call_tool_with_meta。
3. **[M] group-gate over-fetch 被截断击穿** —— merge_hits 先 truncate(search_limit) 再 gate 过滤：top 命中全被排除时下方合法工具不可见 → search() 增加 merge_limit 参数，gate/pin 过滤场景保全部候选过 merge 再过滤。
4. **[L] describe 对睡眠 on-demand pin 工具死路** —— call 可冷启动但 describe 的 live list 为空报 not-found → 记录（建议从索引 metadata 供 schema；本轮未实现）。
5. **[I] conn-loss 交互**：隔离/on-demand 自愈、共享 pool 不自逐出（靠 30s rebuild）——确认为有意边界。

### a1（tray 补漏）
6. **[L] tray open-url trim 只修了 macOS** —— Linux xdg-open / Windows cmd start 仍传原始 url（校验用 trimmed）→ 三平台统一 &trimmed。

### a3（补测试）
7. **+4 Rust 单测**：extract_package_name npx 全分支（pkg_locked 语义 11 例）、mark_conn_loss 变体分类（McpError 含 "connection closed" 文本不误杀）、max_bracket_depth（从 extract() 提取为可测函数）、refresh_owner replay 标志 —— cargo test --lib **123 passed**。
8. **新套件 rmcp_round_r800.py 18/18 + 1 skip**：refresh grant 客户端认证（错 id/错 secret→400、正确→200、replay→400+家族吊销）、17MB body→413（raw-socket）、版本回显×3+2026 stateless、宽松裸请求升格、$smart 可达性。

## 3. 套件适配（行为变更的断言面更新）
- **5 个老套件因 $smart search pin-parity 失败**，全部播种 pin 适配：r6xx（N-PU[c] 断言反转为「describe 也拒绝」四面一致语义）、r200（B/C/D/J 分组播种+作用域还原，C 组补 pin 4 个 browser_* 工具满足 ≥3 命中）、r460（A1 拆双查询：ip 查 ip-server、宽查询取 optional-field 载体；A/B 播种 3 pin）、r461（播种 analyze_calls/search_text/ip 三 pin）、r458（G2 播种 ip pin）。

## 4. 验证
- cargo check 0 错 0 警；cargo test --lib **123 passed**（+4）。
- **61 套 E2E 全绿**（60 既有 + rmcp_round_r800 18 项；新二进制）。

## 5. 复盘
1. **「一个面改了、另一个面没跟」是 pin 特性最大的债**：tools/list 收窄（round40）、call 门（round41+）、search/describe 漏到 round-8 才补——**新特性加门控时，同一数据面的所有暴露点（list/search/describe/call/REST）必须同轮闭环**，否则 LLM 工作流在中间断裂。
2. **修 bug 的 bug（refresh replay 提前返回）被 36 项 oauth 套件当场抓住**——断言密度高的套件是修 bug 的安全网，值得持续加密度（a3 本轮 +4 单测 +18 E2E）。
3. **行为变更必须 grep 断言面**（round-4 复盘教训再现）：$smart search 语义变更击中 5 个老套件——本轮先全量回归再逐一适配，未漏。
4. a2 复审在 5800 行真读中仅 1H+2M+1L 且全部位于 round-4~7 新改动区——存量面连续四轮零 High，收敛态势保持。
