# R491+ 复核第二十二轮报告（2026-10-02 深夜）

> 范围：鉴权/多用户（auth、bearer_key、user_service 密码路径）、配置深合并与导入导出 round-trip、前端登录态生命周期。3 个独立复核代理全量返回 + 亲核修复。

## 1. 修复清单（3 项）

| # | 位置 | 问题 | 修复 |
| --- | --- | --- | --- |
| H1 | `frontend/src/pages/LoginPage.tsx:96` | 默认密码警告弹窗永不可达——`login()` 置 `isAuthenticated=true` 与 `setShowDefaultPasswordWarning(true)` 同批，redirect effect 同帧 `navigate('/')`，LoginPage 在弹窗渲染前被卸载 | effect 加 `!showDefaultPasswordWarning` 守卫 |
| H2 | `services/settings_import.rs` | 导入丢弃 keep-alive/passthrough 三字段——`RawServerConfig` 无 `enableKeepAlive/keepAliveInterval/passthroughHeaders`，构造时硬编码 `None`（与文件内「导出产物导入不得静默丢弃连接相关字段」的注释自相矛盾；导出面已发这三字段） | RawServerConfig 补三字段（camelCase + snake_case alias）+ 透传；新增 3 个单测（round-trip / snake alias / 未知顶层字段容忍） |
| M3 | `frontend/src/utils/interceptors.ts:43` | 401 僵尸会话——只 `removeToken()` 不清 AuthContext 状态，ProtectedRoute 持续放行但所有请求静默失败直到手动刷新 | 401 时 `location.assign('/login')`（已在 /login 则跳过，防循环） |

**记录不修（此前已裁定）**：导出含 `groups` 但导入器忽略（R52 裁定功能增强单独排期）。

## 2. 证伪（代理疑点 × 亲核排除）

- dummy bcrypt 长度 60 正确、verify 错误统一 false（无错误字符串 oracle）
- bearer key 熵 ~122bit 充足；`mcphub_+uuid4.simple` 注释「32-byte」措辞不准（仅注释）
- allowed_servers 存于 key 自身行内 JSON，删 key 无跨行孤儿
- 深合并数组替换语义正确；null 写入语义一致（读侧 unwrap_or）
- mcpServer/routing 键位分裂无实际不一致（skipAuth/enableBearerAuth/exposeHttp/httpPort 各读各写同一处）
- LoginPage i18n 键 4 语言齐全

## 3. 新套件

`scripts/e2e/rmcp_supplement_v22.py`（16 项）——HTTP 面边界与方法语义：
- PUT/DELETE/PATCH × /rest + /api → 405 非 5xx（6）
- 未知服务器/未知 /api 路径 404（2）
- 无 Content-Type / text/plain / 空 body POST → 4xx 非 5xx（3）
- 严格模式只作用 /mcp（REST 面不受影响）+ 缺 Accept 406；宽松放行（3）
- 同会话 3 并发 ping 全 2xx（1）

套件调试沉淀：**legacy 会话 POST 响应是常开 SSE 流**——并发断言只能读状态行不能读 body；strictValidation 配置键在 `mcp`（非 `mcpServer`）命名空间。

## 4. 验证结果

| 项 | 结果 |
| --- | --- |
| cargo check --lib | 0 错 0 警 |
| cargo test --lib | **96 passed**（+3 settings_import 单测） |
| tsc | 0 错误 |
| **26 套 E2E** | **1120/1120 全绿**（重建二进制重启后全量重跑） |

## 5. 经验沉淀

1. **React 同批状态迁移的隐式时序**：`login()` 内部 setState 与调用方的 setState 同批 flush，调用方「先设标志再自己导航」的模式挡不住全局 effect——凡「状态迁移触发副作用」的 effect 必须考虑被调用方标志位参与守卫。
2. **注释与代码自相矛盾 = 高价值信号**：settings_import 注释明说「不得静默丢弃连接相关字段」而代码硬编码 None——复核时应 grep 注释中的承诺逐条对账。
3. **E2E 并发断言要知道传输语义**：legacy SSE 流不关，body 读取会挂——先弄清通道是否常开再写断言。
