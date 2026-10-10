# OAuth 复核战役 第五轮（2026-10-09，W1 三代理 + 亲核修复 + 全量回归）

## 1. 覆盖面
- **a1**：第 4 轮全部 16 项修复的 fix-of-fixes 逐行复审。
- **a2**：前端 OAuth/Smart/pin 全链路（SettingsPage OAuth 区块、SettingsContext、ServerForm/serverFormPayload、ServerCard OAuth 流、ServerToolConfig/ToolCard、tauriClient oauth 映射、4 语言 locale 键位 432 项）。
- **a3**：低覆盖文件（settings_import / bearer_key_service / updater / http_server REST 尾部 / runtime_env+runtime / skill_service+skills）。

## 2. 修复清单（1M + 5L + 2 Info）

### a1（第 4 轮修复自审）
1. **[L] migrate_v21 部分拷贝残留仍被 DROP** —— 事务修复挡住了新部分状态，但存量部分拷贝（old 非空 + new 非空）仍走无条件 DROP 毁源行 → 改为「拷贝后 DELETE 源表 + 仅当源表计数为 0 才 DROP」，部分状态保留数据不再破坏。
2. **[L] redact_args_for_export 跨串字节偏移 panic** —— `a.find('=')` 的字节偏移用于 `lower[..eq]`，`to_lowercase()` 变长（如 İ）时非字符边界 panic → 改为对 `a[..eq]` 单独小写化。
3. **[Info] v30-v39 套件 shebang 被注入 helper 顶到第 7 行** → helper 移到 shebang 之后（10 文件）。

### a2（前端全链）
4. **[M] 桌面「断开 OAuth」不可达** —— Rust list_servers 不带 oauth 状态、`toFrontendServer` 恒无 `server.oauth`、`getServerOAuthStatus` 零调用者 → 连接成功后用户无 UI 路径清除上游凭证 → ServerCard 仿 pins 懒加载（tools tab 展开时拉 `/oauth/status`）+ `oauth-connected` 事件置 true，`supportsOAuthDisconnect` 接通。
5. **[M] OAuth 草稿被同区开关静默抹掉** —— 4 个立即写 toggle 触发 oauthServerConfig 变更重建 temp，丢弃未保存的 lifetime/TTL/scope 输入 → `oauthDirtyRef`（镜像 round-8 smartRoutingDirtyRef 模式）：输入置脏、重建 effect 跳过、保存成功才清。
6. **[L] updateOAuthServerConfigBatch 平铺合并丢嵌套** —— 部分传 `dynamicRegistration` 时本地态丢兄弟键（服务端深合并保留 → UI/持久化漂移）→ batch 显式深合并 dynamicRegistration。
7. **[Info] temp 初值硬编码与 Rust 默认相悖**（300/「read, write」→ 60/空）。

### a3（低覆盖文件）
8. **[M] validate_version_arg 接受 "."** —— `base.join(".")` 解析为 node-versions 基目录本身，install 残留清理/uninstall 直接 `remove_dir_all(base)` **删光全部已装 Node 版本** → 校验器加「必须含数字」（22.14.0/3.14/cpython-3.14.1 均不受影响）。
9. **[L] pick_directory 阻塞 async worker** —— 原生模态框可无限期占用 runtime worker（同文件 save_settings_json 已有 spawn_blocking 范本，此处漏）→ spawn_blocking 包裹。
10. **[L] 导出→导入静默丢 groups** —— export 写 groups、import 只读 mcpServers+users → 解析 groups 字段并在存在时 warn（明确告知组定义被跳过、组级 bearer scoping 需手动重建）。

## 3. 证伪/确认清洁
- reaper 语义（dynamic-only/NOT EXISTS 不被 stale token 污染/ttl 数学）、redaction 五形态全对、user_service 事务无嵌套、TTL round-trip、meta.rs live 抓取有界、pin 两遍解析借用与 parity 正确。
- 前端：11 个 OAuth 字段 save/load 与 Rust 契约逐项对齐；oauth 事件 payload 形状匹配；pin 链全对；locale 432 键零缺失。
- bearer fail-closed 全链、updater 单飞+资源释放、REST 尾部全路由门控、runtime tar/zip-slip/Symlink 逃逸守卫完好、skills 锁与 FTS 同事务全对。

## 4. 验证
- cargo check 0 错 0 警；cargo test --lib **119 passed**；tsc 0 错误。
- **60 套 E2E 全绿**（新二进制热重建后全量）。

## 5. 复盘
1. **「同类修复点穷举到零」仍是主教训**：save_settings_json 的 spawn_blocking 范本就在同文件，pick_directory 漏网——防护模式按「每个阻塞调用点」枚举而非按「修过的文件」记忆。
2. **Path::join(".") 语义**：join 不归一路径组件，"." 直接命中父目录本身——凡 join 用户输入后接 remove_dir_all 的，校验器必须含「解析后 == 父目录」思维（数字校验为实用近似）。
3. **前端死 UI**：prop 全部接通但数据源字段从未被填充 = 静默不可达——「supportsX 布尔」型门控要问数据从哪来。
4. **跨串字节偏移**：同一偏移量用于两个不同字符串是 char-boundary panic 制造机；转换只对小片段做。
5. 收敛态势：a1 复审 16 项修复仅 3 Low（1 项为上上轮遗留语义），a3 八文件仅 1M+2L——存量面持续收敛，本轮 Medium 全部位于「数据填充缺口」（前端 oauth 状态、版本校验）这类「结构存在但语义断链」处。
