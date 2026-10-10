# OAuth 复核战役第十二轮报告（2026-10-10）

## 扫描面
1. **r12-a1-fixes**（code-review）：round-11 修复合复审。
2. **r12-a2-services**（code-review）：services 核心切片全文逐行（mcp_manager/config_service/bearer_key_service/list_freshness/group+server_service/server_tool_config/skill）。
3. **r12-a3-tests5**（general-purpose）：新套件 `rmcp_round_r1200.py` 45 用例（bearer 矩阵/禁用工具门全 face/组 allow-list/清理审计）+ progress/bridge 单测 +3。

## 修复 7 项（1H + 2M + 3L + 1 事故重应用）

### Rust
- **[H·fail-open] 腐坏 system_config 静默关闭全部 HTTP 鉴权**：`config_service::get()` 把腐坏 JSON 映射为 `Ok({})`（保护磁盘数据的合理选择），但 `check_bearer_auth` 以 Ok/Err 为 fail-open/closed 边界——`{}` 使 enableBearerAuth/oauthServer.enabled 均 false → 比特腐坏即全裸奔。新增 `get_strict()`（腐坏=Err、NULL=空对象保全新装语义），接入**三道安全门**（check_bearer_auth、hub_admin_gate、mcp_route_gate）。
- **[M·a1] 保留头过滤不全**：round-11 只过滤了 mcp-session-id，rmcp RESERVED_HEADERS 还有 `accept`/`last-event-id` 同样逐请求拒绝 → 用户配置即连接死循环。修复：三个被拒保留头统一过滤 + warn。
- **[L·a2] session_rebuild 拆毁慢启动服务器**：tick 过滤 `!s.connected` 含 starting=true——慢握手（npx 下载 >30s）期间每个重建 tick 在连接锁上排队后把刚建好的连接拆掉重连。修复：过滤补 `!s.starting`（镜像 on-demand 跳过）。
- **[L·a2] save_agents 绕过 agents_lock**：create/delete custom agent 是持锁 RMW，save_agents 整组替换无锁——并发丢失更新。修复：save_agents 持锁。
- **[L·a2] 空串 description 覆写探针/过滤器语义分裂**：upsert 可存 `Some("")`，apply_tool_filters 会应用而 any_description_override 探针排除 `''` → 热路径跳过拉取后结果不一致。修复：过滤器对空串视为无覆写 + 探针删 `!= ''` 子句（两侧对齐）。
- **[L·a1] scheme 型 Authorization 头非法值静默丢弃**：HeaderValue 解析失败直接丢——请求裸奔只在上游 401 显形。修复：warn 对齐通用分支。

### 前端
- **[M·a1] oauthDirtyRef 永久闩死 + 注释虚构清除点**：全节无 Save 路径，字段级保存又不清 flag（round-11 修复）→ flag 首次输入后永真，重建 effect 整个会话死亡；且 4 处注释声称「全节保存时清除」——该路径不存在。修复：注释改为如实描述（有意不清、remount 再同步）+ TTL 字段保存成功后把归一化值回写 temp（消除 7200.7 vs 7200 显示分歧）。

### 事故与重应用
- **[事故] round-11 bridge refactor 被并发代理整文件写回覆盖**：r11-a3-tests4 在跑期间读旧文件、追加单测后整体写回 rmcp_bridge.rs，把 round-11 的 precompute-param refactor clobber 掉（a1 通过 git diff 发现签名未变——独立复审抓回丢失修复）。本轮重新应用全部 5 处编辑 + 注释对齐。

## r12-a3 新测试
- **`rmcp_round_r1200.py` 45/45**：A bearer 全矩阵（6 通道 × 无 token 401/无效 401/有效 2xx + 真实 IP 调用 + allow-list 收窄）；B 禁用工具门全 face（list absent/scope 拒/REST enabled:false/call 4xx/api 拒/$smart 搜索不泄）；C 组 allow-list + 清理审计断言；D 补充负路径。单测 +3（progress `-p`/`--` 形态、group_member_pins 门控语义）。
- lib **138 passed**；cargo check 0 错 0 警；tsc 0 错误。

## 验证与回归注释
- 全量 62 套回归：首跑 5 套失败（v19/v20/v22/v23/r1200）——**全部定谳为 r1200 bearer 段运行中的瞬态状态传导**（401 级联；bearer 现值已还原 false），逐套干净复跑全绿 + r1200 45/45。教训：bearer/strict 类开关型套件在全量串行跑中是状态污染源——开关套件应自带进程内独占保证或放最后跑。

## 复盘：为什么每轮总能跑出问题
1. **并发代理写同一文件 = 修复丢失**：round-11 的 refactor 被测试代理整文件写回 clobber，若非本轮 a1 以 git diff 独立核验就永久丢失。规则：**多代理并行时，任何代理不得整文件写回它未全程持有的文件**——单测追加必须用精确锚点 patch 而非读-改-写全文件。
2. **fail-open 的边界是「谁把腐坏映射成了合法值」**：get() 的 Ok({}) 对 update 是保护、对鉴权读取是漏洞——同一个 helper 服务两种风险等级的读者时，必须给高风险读者一个 strict 入口。
3. **开关型 E2E 套件是串行回归的状态污染源**：本轮 5 套失败全部源于一个 bearer 开关窗口——全量回归的套件顺序应把改全局开关的套件隔离到最后，且断言「初始基线卫生」提前失败。
