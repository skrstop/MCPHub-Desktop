# OAuth 基线复核战役第二轮（2026-10-09，W1-W2 六代理 + 全量回归）

> 第一轮见 `oauth_review_campaign_20261008.md`。本轮 = 修复自身复审 + 新面（v32 迁移 / hub REST 新路由 / OAuth 设置逐项契约）。

## 一、修复清单（14 项：H×4 M×9 L×1）

### W1（迁移 + 新 REST + 设置契约）
| 级 | 发现 | 修复 |
|---|---|---|
| M | migrate_v32 崩溃窗口：DROP 与 CREATE 之间崩溃 → 重跑时表缺失且 early-return 跳过重建 → 永久 no such table | 去掉 early-return，改 `CREATE TABLE IF NOT EXISTS` 自愈 |
| H | hub 新 OAuth 管理路由仅查 skipAuth——skipAuth+enableBearerAuth 部署下 LAN 任意主机可 disconnect（销毁凭据）/反复 connect（强制开浏览器） | hub_admin_gate：bearer/oauth provider 开启时强制 check_bearer_auth |
| M | hub_admin_gate 复用 is_skip_auth_enabled 的 fail-open（读失败放行）——与同文件 check_bearer_auth 的 fail-closed 政策矛盾 | 直接读 config，读错误 401 |
| L | NoOauthSupport 映射 500（实为客户端配置错误类） | 400 |
| — | 逐项契约审计：11 项配置 7 OK / 3 broken / 2 UI-only | 见下 |
| M | protected-resource metadata 广告 `["read","write"]` 但 AS 实际按 effective_scopes 校验——客户端按广告请求必被拒 | 广告 effective_scopes |
| M | 三个 lifetime 前端接受 0/小数，Rust `filter(>0)+as_i64` 静默回退默认——UI 显示与运行时背离 | 前端保存拒绝 ≤0/非整数 |
| M | `dynamicRegistration.requiresAuthentication` UI-only 死键 | 接入 register 门控（无凭据机制 → 诚实语义=拒绝开放注册并明示） |
| 记录 | `dynamicRegistration.clientTtl` UI-only（注册客户端永不过期）——实现过期需产品决策，维持已记录项 | 不修（记录） |
| L | migrate_v8 全部 `.ok()` 吞错（文件自身在其他 vN 注释里认定此为缺陷类） | 传播错误 |

### W2（修复自身复审）
| 级 | 发现 | 修复 |
|---|---|---|
| H | scope_allowed 硬编码 mcp:all 恒放行——受限配置下请求 `"read mcp:all"` 完全逃逸（scope_to_allowed_servers 见 mcp:all 即无限制） | mcp:all 仅在 allowed_scopes 为空时放行 |
| M | 并发 refresh 重放绕过检测：pre-tx replay 探针在 winner 提交前运行看到空，tx 内 SELECT miss 后直接返回 unknown——复用判定死分支 | tx 内 miss 分支补 refresh_used 标记检查 → 族吊销 |
| M | 轮换 flip 先提交、issue_tokens 后执行——issue 失败=旧 token 已标记 used 且无新对 → 会话永久丢失 | issue 失败补偿性回滚 flip |
| M | 客户端上限统计含 revoked 行——200 行后动态注册永久阻塞且报错信息给的「先吊销」无效 | `WHERE revoked = 0` |
| H | disconnect/重启无法停止已过 select! 的完成任务——`upsert_token_row` 无条件 INSERT 复活已删行（凭据复活 + 幽灵事件） | 三个写入点前 still_owns_flow（ptr_eq）所有权检查 |
| M | 无 code/error 的回调请求（favicon/CORS preflight/手动访问）消费单消费者 slot → 真回调到达前流程被判死 | 仅 code/error 请求投递 |
| M | 手配非 loopback redirect_uri（或固定端口≠临时端口）→ 浏览器重定向到无人监听的地址，流程必挂到 600s 超时 | 配置校验：非 loopback / 端口不匹配 / path≠/ 直接报错 |
| M | search 用 pool 裸列表（连接时快照，未应用 per-tool 覆盖）优先于索引描述——覆盖过描述的工具 search 显示原文、describe 显示覆盖值 | live 列表先过 apply_tool_filters |
| M | smart_route_call 对睡眠 on-demand 服务器（空 pool 缓存）的 pin 恒 "not available"——直呼 lane 有 registry 回退而 meta lane 没有 | resolve_tool miss 后查 server_tool_config pin 注册表回退 |
| M | 新字段/新语义零单测 | +3 单测（scope_allowed 混合作用域 / provider_config 容错；requiresAuthentication 走 E2E G 段） |

## 二、E2E 亲核补充

- **smart 六字段逐字段 DB 级验证**：toolDefinitionFields = [] / 单字段×6 / None 逐项切换，`$smart` describe/search 观察注入/剔除（公网IP 工具本身无可选字段 → 换 Idea 工具验证，r461 22 项已覆盖双向断言，本轮 DB 直改复跑一致）。
- **carpet_r300 120 项**：5 版本 × 3 上游传输 × 4 通道真实公网 IP 调用 + 版本回显 + 宽松/严格 + 2026 新特性 + OAuth 全配置项。
- **修复后全量**：cargo test --lib **119 passed**；tsc 0 错；build ✓；**59 套 E2E 全 PASS**。

## 三、复盘：为什么每次都能跑出问题（第二轮答案）

1. **修复自身是新缺陷的最大来源**（W2 全部 8 项 = 复审昨日修复合）：notify_one 只对「还在 await 的任务」有效——语义边界（past-select! 窗口）不写进注释就必然漏；单消费者 slot 的「第一个投递者赢」让无关请求变成 DoS。
2. **同一语义在多个 lane 复制时必然漂移**：search/describe 两处取描述、直呼/meta 两处解析 pin——单一归一点原则之外的新 lane 忘记同步（本轮 2 项 M 全属此类）。
3. **fail-open 与 fail-closed 混用即漏洞**：同文件两个 gate 对同一 config-read 失败采用相反默认——复制门控时必须连失败语义一起复制。
4. **UI 语义与运行时能力错配**：开「requiresAuthentication」却无凭据机制可校验 = 撒谎的开关；诚实做法是关死 + 明示，而非假装发秘密。

## 四、基线同步涉及功能/范围/影响点（本轮增量）

- **范围**：OAuth（上游+Provider 全链）、smart toolDefinitionFields/fullSchemaTopN、pin 双 lane、v32 迁移、hub REST 面扩展。
- **影响点**：`scope_allowed` 收紧（受限配置下 mcp:all 不再搭车——依赖旧行为的客户端需改）；requiresAuthentication 从「发秘密」改为「关闭开放注册」；redirect_uri 手配值校验收紧；**用户需重启**使迁移/路由/语义生效。
