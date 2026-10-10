# OAuth 复核战役第十四轮报告（2026-10-10）

## 扫描面
1. **r14-a1-fixes**（code-review）：round-13 修复合复审（git diff 亲验当前树）。
2. **r14-a2-commands**（code-review）：commands/ 层全量逐行 + **程序化枚举全部 #[tauri::command] 逐个核 require_admin** + invoke_handler 双向 diff。
3. **r14-a3-tests7**（general-purpose）：新套件 `rmcp_round_r1400.py` 99 用例。

## 修复 6 项（2H + 1M + 1L + 2 修自纠）

### Rust
- **[H·门控] list_servers / get_server / search_servers 无门控返回全量凭据**：ServerInfo 携带完整 ServerConfig（env/headers/oauth clientSecret/accessToken/refreshToken/proxy）原样序列化——与 export_settings/get_system_config 门控的是同一载荷；skipAuth=false 模式下任意非 admin 会话（甚至无会话）可读全部上游凭据。修复：三个读命令补 require_admin（round-32 先例；skipAuth 桌面默认短路，单用户 UI 无感）。
- **[H·门控] get_server_config_for_copy args 未脱敏**：doc 注释声称「敏感值已脱敏」，但 args 原样透传——stdio args 惯常内联凭据（`--header "Authorization: …"`），export 路径有 redact_args_for_export 而复制路径漏。修复：补 require_admin（注释与行为对齐）。
- **[M] is_skip_auth_enabled DB 读失败 fail-open**：Err → true 使 auth-enabled 部署中任意瞬时 DB 错误把全部 require_admin 门敞开。修复：fail-closed（skipAuth 桌面 DB 不可读时命令本来就全挂，不引入新锁死）。
- **[L] change_password 错误预言机**：verify 错误（腐坏 hash）原样 e.to_string() 可与「密码错误」区分——与 login 路径的防枚举决策矛盾。修复：unwrap_or(false) 对齐。
- **[自纠·a1] MarketPage 播种空集无效**（见前端）。
- **[自纠·a1] 探针/过滤器对称**（round-13 已修，a1 复核确认在位）。

### 前端（a1 两项 + i18n）
- **[H] installedRegistryServers 播种在首渲染恒空**：round-13 用 useState initializer 从 localServers 播种，但 localServers 来自 useMarketData 异步 fetch（首渲染恒 []），且无 effect 再播种——修复无效。改 **useEffect union-merge 再播种**（不 clobber 会话内安装）。
- **[M] 键不一致**：播种用 slugify 名、安装成功 add 原始 registry 名、查找也用原始名——非 slug 名（大写/空格/下划线）跨会话永不匹配。统一三处 slugifyServerName。
- tr `exclusionPersistFailed` 措辞修正（"Hariç tutma kuralları kaydedilemedi"）。

### a2 复核确认清洁
- invoke_handler 注册双向 diff **零孤儿**；validate_version_arg（"." 拒绝）、SHASUMS 校验、tar/symlink 逃逸守卫、CREATE_NO_WINDOW、delete_server TOCTOU 序、spawn_blocking 覆盖、RAG/skills 门控与 validate_doc_id/valid_dir_name 全量在位、registry/cloud 流式 32MB 帽 + 30s 超时。
- 记录不修：log_event 无速率限制（既定前端埋点设计）、错误串透传 webview（Info 泄露面，待集中 helper 批次）、get_http_server_status 未门控（低敏）、activity 批次打点。

## r14-a3 新套件（99/99）
- **A 设置逐项 round-trip 31/31**：SettingsContext 全部用户可见键（routing×6/httpPort/exposeHttp/mcp.strictValidation/smartRouting×15/nameSeparator/enableSessionRebuild/activityLog.storeToolPayload/install×3）——写→读回→还原零差异。
- **B 路由开关矩阵 10/10**：global/group 两开关独立语义对齐 origin（global 管 /mcp+$smart、group 只管组路径——初版套件期望错误已按实现+origin 契约校正）。
- **C 严格/宽松全负矩阵 48/48**：6 种畸形输入 × 4 版本严格 4xx / 宽松放行成对。
- **D TTL freshness 4/4 + E json_response/subscriptions 6/6**（2026 CJK SEP-2243 base64 Mcp-Name 真实 IP）。
- **孤儿键 WARN 清单（8 个，均为既定产品决策）**：similarityThreshold（待裁定）、embedding 前缀类 ×4（origin UI-first）、install 三镜像键。

## 验证与回归
- cargo check 0 错 0 警；cargo test --lib 146 passed；tsc 0 错误。
- 全量 64 套回归：63 绿 + r158 E4 1 例 → 定谳为**时序漂移**（IP 任务先于 tasks/result 完成 → 终态载荷=工具结果本体，R50c 既定契约；套件期望缺该形态）→ 断言扩展后 24/24。
