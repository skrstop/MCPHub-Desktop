# OAuth 复核战役第十轮报告（2026-10-09）

## 扫描面
1. **r10-a1-fixes**（code-review）：round-9 全部修复合复审 + 二阶自查（新死代码/panic/锁序/TOCTOU）。
2. **r10-a2-oauth**（code-review）：OAuth 四文件全文重读（provider/upstream/http_server oauth 路由/commands）+ 前端 OAuth 链 + 每个设置项 UI↔后端对账 + guest 门控。
3. **r10-a3-tests3**（general-purpose）：新套件 `rmcp_round_r1000.py` 71 用例 + merge_hits 单测 ×3。

## 修复 6 项（1H + 3M + 2L + 1Info）

### Rust
- **[H·round-9 修复自身引入] call lane 空 pin 反向死胡同**：round-9 把 list/search/describe 的空 pin 集改为「门控失效」，但 call 双 lane（直调 + meta 包装）的 `resolve_pinned_target` 独立门控无空集豁免——零 pin 时 search 推荐的工具全部被 call 拒绝，恰是 round-9 要消灭的死胡同类别在 call 面复活。修复：新增 `smart_pin_gate_active`（pin 注册表非空才激活），两 call lane 共用；四面在「有 pin/无 pin」两态下语义一致（无 pin=list 面仅 meta 工具，call/search/describe 全量——「call 宽于列表」为 round-8 既定 REST 方向）。
- **[M] describe fallback 对已连接服务器误诊**：工具从活列表消失（上游改名/删除而配置行残留）时误报「服务器未连接，先 call 唤醒」——call 会 tool-not-found，客户端被引导做不可能成功的事。修复：以活列表是否非空区分「已连接但工具消失（改名/删除文案）」与「睡眠（唤醒文案）」。
- **[L] resolve_tool pin 兜底越界**：对前缀未匹配的服务器也查配置注册表（多服务器裸名歧义）且不排除 meta 名 pin → 与 bridge 解析器语义分裂。修复：限定前缀匹配服务器或单服务器作用域 + `is_meta_tool` 排除对齐。
- **[L] search 热路径无谓 pool 拉 取**：round-9 为 description 覆写刷新把活列表拉取改为无条件——每命中服务器每次 search 都付全量列表 + apply_tool_filters。修复：`tool_fields` 为空且全库无 description 覆写（新 `any_description_override()` 单行探针）时整体跳过。
- **[Info] tray 打开链接留僵尸进程**：`spawn()` 返回的 Child 直接 drop——macOS/Linux 每次开链接累积一个僵尸。修复：分离线程 `wait()` 收尸。

### 前端
- **[M] disable 分支清 dirty 抹掉草稿**：round-9 给 `handleSmartRoutingEnabledChange` 成功清 dirty——但 disable 分支只持久化 `enabled`，清 flag 后重建 effect 用已存配置覆写 temp，静默丢掉用户翻转开关前输入的全部草稿。修复：disable 分支不清 dirty（enable 分支先批量保存草稿再清，安全）。

### a2 复核确认清洁（重点摘录）
- refresh_owner replay 契约、reaper dynamic=1、scope 真值表、PKCE S256、code 单用 BEGIN IMMEDIATE、常数时间比较、consent CSRF nonce、deny 分支防开重定向、无明文 token/secret 入日志——全部逐项验证通过。
- **设置项对账零孤儿**：11 个 UI 键全部有 Rust 消费方（provider_config），无后端-only 键；默认值 UI/Rust 一致。
- rmcp 3.4.1 `AuthorizationRequest` 无 resource 字段（RFC 8707 未支持）→ `ServerOauthConfig.resource`（UI 收集）无法干净接线，与静态 `refresh_token` 无消费两项目录为「UI 收集 Rust 无落点」既有产品待办（R10 裁定不加半成品透传）。

## r10-a3 新测试（全绿）
- **`rmcp_round_r1000.py` 71/71**：A 空 pin 门控态矩阵（零 pin 三面正例 → 单 pin 过滤 → unpin 还原，14）；B OAuth 11 个设置项 round-trip + HTTP 反射（metadata/register 门控，17 + 6 WARN 无 HTTP 观察面项）；C 版本×通道真实 IP 矩阵（4 legacy + 2026 stateless + REST + /api，11）；D 逐版本宽松/严格成对（8）；E 六字段逐个开关 × 真实载体（21）。
- **单测 +3**：merge_limit usize::MAX 不截断不溢出、跨通道去重加分 0.9 精确、空输入不 panic → lib **136 passed**。

## 全量回归
- 61 套件：60 绿 + r6xx 2 例失败 → 定谳为同类「unpin 后拒绝」旧期望（编码了空 pin bug 时代的语义）→ 修正 N-PU[b]/[c] 为门控失效正例 → 20/20。
- 同类旧期望修正累计：r900 A11/A12/A13、r111 C2、r6xx N-PU[b]/[c]——**pin 语义变更（round-8 引入）的断言面此前只改了一半，本轮四套件收口完毕**。
- cargo check 0 错 0 警；cargo test --lib 136 passed；tsc 0 错误。

## 复盘：为什么每轮总能跑出问题
1. **行为语义变更 = 断言面债务**：round-9 的「空 pin = 门控失效」改了 4 个 face 中 3 个的语义，call 面漏改（本轮 H）；改了产品语义却只改了 3 个套件的断言（本轮 r6xx/r111/r900 连锁）。规则：**语义变更先 grep 全部消费点与断言面，逐一列清单改齐**——本轮 6 处断言修正全部源于一个语义没改齐。
2. **对称性检查要双向**：round-9 修了「空集全拒→全放行」，没问「放行后 call 面是否还在拒」——修复一个方向时必须枚举同一状态在全部四个面的表现。
3. **fix-of-fixes 仍是主要来源（本轮 3/6 修复直接源于 round-9 修复）**，但独立复审代理 + 全量回归的组合把捕获时间压到一轮以内，High 缺陷存活周期从多轮缩短至 1 轮。
