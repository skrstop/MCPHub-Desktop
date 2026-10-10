# OAuth 复核战役第十一轮报告（2026-10-10）

## 扫描面
1. **r11-a1-fixes**（code-review）：round-10 修复合复审（含 A14 零 pin 不变量手工推演、禁用工具泄漏证伪）。
2. **r11-a2-transports**（code-review）：mcp 传输层 8 文件全文逐行（rmcp stdio/http、手写 SSE、openapi、pool、session_pool/on_demand、progress），rmcp 3.4.1 / reqwest 0.13 vendored 源码逐点对照。
3. **r11-a3-tests4**（general-purpose）：新套件 `rmcp_round_r1100.py` 67 用例 + 单测 +3 + 测试基建修复。

## 修复 4 项（1H + 2M + 1L）

### Rust
- **[H·迁移回归] Authorization 头双 scheme**：rmcp 3.4.1 对 `auth_header` 走 reqwest `bearer_auth()`（无条件前置 "Bearer "）——携带 scheme 的值（OAuth 解析的 `Bearer x`、`Basic ...`）发到上游变成 `Bearer Bearer x`，认证全挂（迁移前 transport 原样透传，系迁移回归）。修复：值已带 scheme（Bearer/Basic/Digest 前缀，大小写不敏感）时改走 `custom_headers` 逐字透传（rmcp 先 bearer_auth 后 apply custom，custom 覆盖生效）；裸 token 仍走 auth_header。
- **[M] 用户配置 mcp-session-id 头致命循环**：SSE transport 有过滤而 rmcp HTTP transport 无——rmcp `validate_custom_header` 将其按保留头**逐请求**拒绝（ReservedHeaderConflict）→ 连接失败/请求循环失败 → ServiceError::TransportClosed 喂 mark_conn_loss → session_pool/on_demand 驱逐重建死循环。修复：镜像 SSE 过滤（大小写不敏感 + warn 日志）。
- **[L] $smart 门控调用 pin 注册表 3-4×重复读**：一次门控 meta 调用 smart_pinned_servers 跑 3-4 次（gate/resolve/pinned_set/兜底各一次，每次每可见服务器一条 DB 查询）。修复：每请求读一次——resolve_pinned_target 签名改为接收预计算注册表，gate 用 `!pinned_servers.is_empty()` 内联；TOCTOU 复核定谳良性（短路语义只会放宽 gate 自身陈旧读的窗口）。

### 前端
- **[M] OAuth 字段级保存清全节 dirty 抹跨字段草稿**：oauthDirtyRef 管全节，但 4 个字段级立即写 handler（数字 TTL ×2、allowedScopes、grantTypes）成功即清——用户未保存的 allowedScopes 草稿被随后保存的 TTL 触发的重建 effect 覆写。修复：字段级保存不再清 flag（flag 仅全节保存路径清除）；round-10 修的 toggle-vs-draft 与本轮修的 save-vs-draft 是同一重建 effect 的两个方向。

### a1/a2 证伪摘录
- **禁用工具泄漏疑虑证伪**：`any_description_override` 跳过拉取不会让禁用工具泄入 search——search.rs 独立 filter_disabled（:190-192）与 live fetch 无关。
- describe disabled 优先级、resolve_tool sleep+meta 边界、tray mut child、scope 归一、超时覆盖（5 条调用面全包 timeout_tool_call）——全部验证通过。

## r11-a3 新测试（3 连跑全绿）
- **`rmcp_round_r1100.py` 67/67**：A 版本回显精确一致 × 3 通道（root/服务器 scope/组）× 4 legacy + 2026 stateless 不降级（14）；B **每版本×每通道真实公网 IP 调用**（正确前缀形态 + 语义 IPv4，~25）；C 逐版本宽松成对 + 严格抽查（12）；D **真实 OAuth 完整生命周期**（动态注册 201 → consent nonce → approve 302 code → PKCE token 200 → 真实 refresh_token grant 200 → 伪造 code/缺 grant_type/伪造 RT 400 invalid_grant → metadata，10，无头完成零 SKIP）；E $smart describe/call 命名不对称行为钉死 + unpin 还原（8）。
- **单测 +3**：progress `-p`/`--`/锁后位置参数、group_member_pins 纯核抽取（smart_pin_gate_active 语义可无池单测）。
- **测试基建修复**：`#[tokio::test]` 各持 runtime——drop 后 sqlite::memory: 池死、下个测试开全新空库（隔离跑掩盖）+ oauth 双模块争抢全局池 → 共享 `db::ensure_test_pool()`（file-backed，每进程删）→ 3× 连续全绿 138 passed。

## 验证
- cargo check 0 错 0 警；cargo test --lib **138 passed**（×3 稳定）；tsc 0 错误。
- 全量 **61 套 E2E 全绿**（含 r1100 67、r1000 71、r6xx 20 复跑）。

## 复盘：为什么每轮总能跑出问题
1. **「对面已修，此面未同步」是迁移/语义变更的头号模式**：本轮 H（SSE 过滤 session-id 而 rmcp transport 没过滤）、M（字段级清 dirty 而全节语义）都是「同族代码两份实现只改一份」。规则：修任何一个过滤/守卫/语义，先 grep 同类实现逐一对账——本轮两个 M/H 全部可由「对面有、这边没有」的 diff 视角在写代码时发现。
2. **库封装的隐式转换必须到 vendored 源码核验**：bearer_auth 无条件前置 scheme 是 reqwest 文档级行为，封装层（rmcp auth_header）没有反转它——凡是「把用户值交给库 API」的路径，都要问库对值做了什么变换。
3. **测试基建 bug 与产品 bug 同样值得复审**：内存池跨测试死亡让「138 passed」在隔离跑下是假象——3 连跑 + 共享池修复后才可信；「隔离绿 ≠ 并行绿」与「单跑绿 ≠ 全量绿」同构。
