# OAuth 复核战役第九轮报告（2026-10-09）

## 扫描面
1. **r9-a1-fixes**（code-review）：第八轮全部修复合的独立复审（mark_conn_loss variant 分类、pkg_locked、refresh_owner replay、$smart 四面 parity、tray spawn）。
2. **r9-a2-frontend**（code-review）：前端终态逐行（SettingsPage Smart Routing 全部 handler、OAuth dirty 链、ServerCard oauthConnected）。
3. **r9-a3-tests2**（general-purpose）：新套件 `rmcp_round_r900.py`（35 用例）+ 单测补批。

## 修复 5 项（含 1 项回退自纠）

### Rust（3 项）
- **[M] describe 对睡眠 on-demand 服务器死胡同**：search/call 都有 pin 兜底，唯 describe 活列表 miss 后直接 generic not-found——客户端按 search→describe→call 流程被卡死（call 通道可用）。修复：config-store fallback——disabled → 拒绝文案；enabled+睡眠 → 「先 invoke smart_route_call 唤醒后再 describe」文案（meta.rs ~785）。
- **[M] merge_limit 联合通道欠覆盖**：向量+关键词两通道合并可至 2×cap，先截断再过滤会丢真实命中。修复：filtered 路径传 `merge_limit = usize::MAX`（不截断，调用方 take 收口）（search.rs / meta.rs）。
- **[自纠·回退] bare-name 歧义补丁破坏 round-6 契约**：a1 报「多服务器作用域 bare 名按 pin 向量序歧义解析」并建议消歧——我实施后 pin_matrix A3 失败。复盘发现 a1 误读：`try_pin` 内 `use_prefix` 分支本就只匹配前缀形（round-6 已封 bare 名），bare 分支仅在单服务器作用域生效且两遍 exact→prefixed 已确定序。**回退消歧补丁，恢复 round-4/6 语义**；教训记入复盘。

### 前端（2 项）
- **[M] handleEmbeddingProviderPresetChange 不置 dirty**：preset 编辑会级联写 baseUrl/model 到 temp 但未标 `smartRoutingDirtyRef`——随后的立即写开关（如 serverDescriptionMode）用已存配置重建 temp，静默抹掉 preset 选择。
- **[L] handleSmartRoutingEnabledChange 成功后不清 dirty**：批量保存成功仍留 dirty ref → 表单被冻结（后续 saved-config 更新无法重建 temp）→ 成功时清 flag（镜像 handleSaveSmartRoutingConfig 模式）。

### tray（第 3 OS 分支收口）
- Linux `xdg-open` / Windows `cmd start` 全部改 spawn `&trimmed`（macOS 为第八轮）——三个 OS 分支全量对齐。

## r9-a3 新测试（全绿）
- **`rmcp_round_r900.py` 35/35**：A $smart 四面 pin parity（list/search/describe/call × pinned/unpinned/还原 14 例）；B OAuth metadata（protected-resource / authorization-server 6 例，含 grants/registration/secret_post）；C 版本×通道矩阵真实 IP 调用（2025-03/06/11 + 2026 stateless discover+call 11 例）；D 宽松成对（裸请求升格/畸形 JSON/text-plain 4 例）。
- **单测 +10**（merge_hits 去重加分/字节序 tiebreak/阈值含界/limit 截断 + upstream_meta_from 3 例 + is_meta_tool 3 例）→ `cargo test --lib` **133 passed**。

## 全量回归揪出的 High（修复后补记）
- **[H·round-8 回归] 空 pin 集合清空全部 search 命中**：round-8 四面 parity 在 bridge 无条件传 `pinned_ref = Some(&pinned_set)`——无任何 pin 时为空集，过滤语义把**所有**命中拒掉 → 根 `$smart` 无 pin 时 search 恒 0 结果、describe 恒拒绝（端点基本不可用）。r900 未抓到系其 unpin 后断言只查「该工具不在」（空集全拒恒真）——A14 补为「零 pin 时 search 必须有命中」。修复：`pinned_ref` 仅在 `!pinned_set.is_empty()` 时传 Some（门控仅在存在 pin 时激活）。连带修正两个把错误语义固化为期望的老断言（r900 A11/A13、r460 B 段 pin 继承）。修复后验证：MCP face search "ip" 返回 10 命中；r460 37/37、r900 全绿、pin_matrix/carpet/r800/r200/r461/r458/r109/r110 全绿；lib 133 passed。

## 证伪/记录不修
- a1 的 bare-name 歧义 Low **误报**（误读 try_pin 的 use_prefix 分支）——已回退其建议补丁。
- [Minor] describe 接受 bare pin 名而 call 要求前缀形（root $smart 客户端困惑面，call 侧为 round-6 安全契约，记录）。
- [Minor] `smart_route_describe` 未列出但可直调（标准模式未启用渐进披露时——callable-but-unadvertised，记录）。
- [Info] oauthConnected 事件/fetch 竞态实际不可达；[Info] OAuthDisconnectScope 后端忽略 scope（语义死代码）。

## 验证
- cargo check 0 错 0 警；cargo test --lib 133 passed（+10）；tsc 0 错误。
- 全量 59 套件回归：58 绿 + r460 2 例失败 → 定位为 High 空 pin bug（见上）→ 修复后 r460 37/37。
- 修复后专项复跑全绿：r900（含新 A14）、r460 37、pin_matrix 31、carpet_r300 122、r800 18、carpet_r200、r461、r458、sync_baseline_r109/r110。

## 复盘：为什么每轮总能跑出问题
1. **fix-of-fixes 是最大 bug 来源（本轮 4/5）**：本轮修复全部围绕第八轮修复合的自身缺陷（describe 漏 parity 第三面、merge_limit 半截、dirty ref 漏第二个 handler、tray 漏第三个 OS 分支）。模式：**按「面」修复而不是按「清单」修复**——修了 describe 忘了它也有睡眠态；修了两个 handler 忘了第三个同类 handler。
2. **「恒真断言」掩盖回归**：r900 的 unpin 段断言「工具不在搜索结果」在空集全拒 bug 下恒真——负向断言必须在「门控应活跃」与「门控应失效」两种状态下各设正/负两例（A14 即为此补）。
3. **代理报告必须亲核到分支级**：a1 的歧义 Low 是对 `use_prefix` 条件分支的误读——若未实测（A3 套件抓包）直接采纳，会把 round-6 安全契约倒退回 bare 名放行。**规则：代理建议的行为变更必须由套件断言背书后才落盘。**
3. **清单化同类点**：dirty ref（preset/enabled/save 三 handler）、OS 分支（mac/win/linux 三 spawn）、pin 面（list/search/describe/call 四 face）——每类修复点先 grep 枚举全量再逐个对账，是收敛的唯一路径。
