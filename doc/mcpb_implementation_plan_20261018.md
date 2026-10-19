# MCPB（.mcpb 文件安装）完整实现计划

> 日期：2026-10-18
> 目标：参考 origin（mcphub-origin）的 MCPB 逻辑，在桌面端完整实现 .mcpb 文件上传→解压→安装为 stdio 服务器的全链路。
> 状态：**已实施（2026-10-19）**——Rust `services/mcpb_service.rs` + `commands/mcpb.rs`（命令 `upload_mcpb`）+ 前端 `McpbUploadForm.tsx` handleUpload 接线；8 单测全过；cargo check 0 错 0 警 / lib 186 passed / tsc 0 错 / build ✓。详见 AGENTS.md §3.11。

---

## 1. Origin 逻辑扫描结论（逐处核对）

### 1.1 后端 `src/controllers/mcpbController.ts`（212 行）

| # | 逻辑点 | 细节 |
| --- | --- | --- |
| 1 | 路由 | `POST /mcpb/upload`，`requireAdminMiddleware` + multer `upload.single('mcpbFile')`（routes/index.ts:419，admin-only 注释引用 GHSA-xf2m-3c3x-53vp：handler 会替换既有 stdio 配置指向的确定性路径下的可执行代码） |
| 2 | multer 存储 | 目标 `data/uploads/mcpb/`（不存在则递归创建）；文件名 = `sanitize(originalname)-{timestamp}.mcpb`（`path.basename` + `[^A-Za-z0-9._-]→_`，防路径逃逸） |
| 3 | 过滤与限额 | 仅 `.mcpb` 后缀；500MB 上限 |
| 4 | 纵深防御 | handler 内再次 `requireAdmin`（防经其他路径调用） |
| 5 | 上传路径复查 | `path.resolve(mcpbFilePath)` 必须以 `resolve(uploadDir)+sep` 开头，否则 400 "Invalid MCPB file location" |
| 6 | 解压 | `AdmZip.extractAllTo(temp-extracted-{timestamp}, true)`（zip 内先解到临时目录） |
| 7 | manifest 校验 | 根目录 `manifest.json` 必须存在；四必填：`manifest_version` / `name` / `version` / `server`，各自明确错误文案 |
| 8 | 名称校验 | `validateServerName`（serverNameValidation.ts）：trim 非空、smart-routing 保留名、≤128 字符、charset `[A-Za-z0-9._-]`、**拒绝 `..`**（名称用于文件系统路径）；错误文案固定 "Invalid manifest: name contains unsafe characters" |
| 9 | 目标目录 | `resolveMcpbServerExtractDir`：`server-{name}`，`path.relative` 复查不逃逸 base（`..` 开头或绝对路径即抛错） |
| 10 | 旧版本清理 | `cleanupOldMcpbServer`：遍历 upload 目录删 `server-{name}` 前缀的**目录**；失败仅 warn 不阻断安装 |
| 11 | rename 前复查 | cleanup 吞错，所以 rename 前再 `existsSync` → `rmSync`（Windows rename 到已存在目录 EPERM） |
| 12 | 收尾 | `renameSync(temp → final)` → 删除上传的 .mcpb → 返回 `{ success, data: { manifest, extractDir } }` |
| 13 | 错误清理 | 解压/校验失败：删 .mcpb + 删 temp 目录，500 + error.message |

### 1.2 名称校验 `src/utils/serverNameValidation.ts`（76 行）

- `SERVER_NAME_PATTERN = /^[A-Za-z0-9._-]+$/`，`SERVER_NAME_MAX_LENGTH = 128`。
- 理由：服务器名嵌入下游工具标识 `${serverName}${sep}${toolName}`，受 MCP 工具名 charset 约束。
- `..` 被 charset 允许但因用于 MCPB 上传目录而显式拒绝。
- 磁盘加载的存量名不拒绝（仅启动时 warn）。
- `slugifyServerName`：registry 反向 DNS 名 → charset 安全名（桌面 MarketPage 已有）。

### 1.3 安装语义 `src/services/mcpService.ts::addOrUpdateServer`（:3028）

- `allowOverride` 参数：存在且不允许覆盖 → "Server name exists"；覆盖时先 `closeServer`（清 keep-alive/连接）再 update。
- 桌面对应：`create_server` / `update_server` 命令已存在；前端用「重名 → ConfirmDialog → PUT 覆盖」流程表达同一语义。

### 1.4 前端 `frontend/src/components/McpbUploadForm.tsx`（462 行）

1. 拖拽/点击选择 `.mcpb`（后缀校验，错误 `mcpb.invalidFileType`）。
2. `handleUpload`：FormData（`mcpbFile` 字段）→ `POST /mcpb/upload` → 取 `{manifest, extractDir}` → 进入安装表单。
3. 安装表单：展示 manifest 信息（display_name/version/description/author/tools 列表）+ 可编辑服务器名（默认 `manifest.name`）。
4. `convertMcpbToMcpConfig`：**stdio 配置**——`command = mcp_config.command || 'node'`；`args` 逐项 `${__dirname}` → extractPath 替换；`env` 存在时整体拷贝并对 string 值做同样替换。
5. `handleInstallServer`：非覆盖时先 `GET /servers` 查重 → 存在则 ConfirmDialog（`mcpb.serverExistsTitle/Confirm`）→ 确认后 `PUT /servers/{name}` 覆盖；否则 `POST /servers` 创建。
6. 成功 → `onSuccess(serverConfig)`（ServersPage 刷新列表）。

### 1.5 DXT

`dxtController.ts` 仅 1 行（re-export/兼容 shim），DxtUploadForm 3 行——**无独立逻辑，MCPB 即全部**。

---

## 2. 桌面端现状

| 层 | 现状 |
| --- | --- |
| `frontend/src/components/McpbUploadForm.tsx` | **已存在，与 origin 逐字一致** |
| `frontend/src/pages/ServersPage.tsx` | 已接线（:9 import、:57 state、:265 上传按钮、:474 渲染） |
| `locales/*.json` | `mcpb.*` 23 键 ×4 语言已齐（与 origin 完全一致） |
| `frontend/src/utils/tauriClient.ts:461` | `POST /mcpb/upload` → **stub 拒绝**（"MCPB upload is not available in desktop mode"） |
| `src-tauri/` | **零 MCPB 实现** |
| 依赖 | `zip = "8"`（default-features=false, deflate）已在 Cargo.toml:159；`tauri-plugin-dialog` 已在 |
| 既有范本 | `skill_service::pick_directory`（原生对话框 + spawn_blocking）；`server_service::validate_server_name`（:144）；`runtime.rs` tar 逃逸守卫；`save_settings_json`（require_admin + 对话框命令形态） |

**结论：前端 UI/locales/安装路由全部就绪，唯一缺口 = 上传通道。桌面 webview 不支持 multipart FormData → 需原生文件选择器 + Rust 命令。**

---

## 3. 执行计划

### Phase 1 — Rust 后端（核心，~250 行）

新文件 `src-tauri/src/commands/mcpb.rs`，命令 `upload_mcpb`：

0. **门控（桌面差异，用户裁定）**：**登录用户（任意角色）和游客都可用**——不用 `require_admin`（那会拒绝非 admin 登录用户）。新本地 helper `require_authenticated`：skipAuth 开启（游客模式）→ 直接放行；否则校验 JWT 有效（任意角色，含 `user`），未登录 → "Not authenticated"。与 origin 的 admin-only 有意不同，记入差异清单。

1. **文件选择**：`tauri-plugin-dialog` 原生 `blocking_pick_file`（标题/`.mcpb` 过滤），`spawn_blocking` 包裹（镜像 `skill_service::pick_directory`）；取消返回 `Ok(None)`，前端静默。
2. **解压根**：`app_data_dir()/uploads/mcpb/`（对应 origin `data/uploads/mcpb`），不存在递归创建。
3. **安全加固**（origin 逻辑 + 桌面既有审计标准）：
   - 文件大小帽 500MB（选取后 `metadata()` 预检）；
   - **zip bomb 防护：解压累计解压尺寸帽**（origin 无，按 R20 OCR `decode_limited` / registry 32MB 流式帽同族标准补，建议 1GB）；
   - **zip-slip**：entry 名拒绝绝对路径 / 含 `..` 组件（对齐 `runtime.rs` tar 逃逸守卫的 ParentDir/RootDir 拒绝）；
   - manifest 四必填校验，错误文案逐字对齐 origin；
   - 名称校验复用/镜像 `server_service::validate_server_name`，对外文案 "Invalid manifest: name contains unsafe characters"；
   - 目标目录 `server-{name}` path-escape 复查（镜像 `resolveMcpbServerExtractDir`）。
4. **版本管理**：`cleanup_old_mcpb_server`（删 `server-{name}` 前缀目录，失败 warn 不阻断）→ rename 前 `exists` 复查 rm（Windows EPERM 语义）→ `fs::rename(temp, final)`；错误路径清理 temp。
5. **桌面差异（有意）**：不删除用户选取的原始 .mcpb——origin 删的是自己 upload 目录里的副本，桌面直接读用户文件，保留原文件。
6. 返回 `{ manifest: serde_json::Value, extractDir: String }`（`Option` 外层承载取消语义）；注册进 `lib.rs` invoke_handler + `commands/mod.rs`。

### Phase 1.5 — 类型校验与 uv 支持（2026-10-19 追加，已实施）

1. **未知类型异常提示**：`server.type` ∉ `{node, python, binary, uv}` → 上传即拒（"Unsupported server type '{x}' (supported: ...)"）；非字符串 type 同拒；
2. **缺 mcp_config 异常提示**：node/python/binary 缺非空 `mcp_config.command` → 明确报错（替代 origin 静默 `|| 'node'` 挂死）；无 type 但有 command 放行（origin parity）；
3. **uv 自动合成**：`type=uv` 且无 mcp_config（规范允许）→ 前端合成 `uv run --directory {extractDir} {entry_point}`（Rust 侧保证此分支必有 entry_point）。

### Phase 2 — 前端接线（~20 行）

1. `McpbUploadForm.tsx` 仅改 `handleUpload`：FormData fetch → `invoke('upload_mcpb')`；返回 `null`（取消）静默返回；其余流程（manifest 展示 / 服务器名输入 / 重名 ConfirmDialog / apiPost·apiPut /servers）**零改动**——已由 tauriClient 透明路由到 `create_server` / `update_server`。
2. tauriClient:461 的 mcpb stub 保留作防御性兜底（正常流不再触达）。

### Phase 3 — 验证

> 2026-10-19 补充：每类型单测已补齐——Rust 15 个（node/python/binary/uv 双形态端到端 + 全部负路径）+ 前端 vitest 11 个（转换逻辑抽为 `utils/mcpbConvert.ts` 纯函数，**覆盖率 100%**：语句/分支/函数/行全满）；`npm test` 脚本接入。

1. `cargo check` 0 错 0 警；`cargo test --lib` 全绿；
2. `npx tsc --noEmit` 0 新增错误；`npm run build` 通过；
3. 真机冒烟（构造测试 .mcpb：manifest.json + 假 node 脚本）：
   - 上传 → 安装 → 服务器列表出现 → `${__dirname}` 替换为绝对 extractDir；
   - 同名重装覆盖旧目录（旧文件不残留）；
   - 重名 → 确认框 → 覆盖成功；
   - 取消文件选择器 → 静默无报错。
4. 负路径：zip-slip entry / 超 500MB / 缺 manifest / 缺必填字段 / 非法名称（`..`、非法字符）各 1 例，均得明确错误且无残留 temp 目录；
5. 门控：skipAuth（游客）可上传；auth 模式下非 admin 登录用户可上传；未登录（无 token）被拒 "Not authenticated"。

### Phase 4 — 文档

- AGENTS.md §3 新增 MCPB 小节（落点/差异/安全加固）；
- 待办清单勾选「MCPB/DXT 文件安装」；
- 如随版本发布，`doc/upgrade/{version}.md` 补 changelog 条目。

---

## 4. 桌面与 origin 差异清单（实施后核对用）

| 差异 | 理由 |
| --- | --- |
| 门控：登录用户（任意角色）+ 游客可用（origin 为 admin-only） | 用户裁定：桌面本地工具语义，MCPB 安装不视为管理操作 |
| multipart HTTP 上传 → 原生文件选择器 + invoke | webview 不支持 FormData 上传；与 `save_settings_json` 同模式 |
| 不删除用户原始 .mcpb | origin 删的是服务端 upload 目录副本；桌面读用户文件 |
| 新增 zip bomb 累计解压尺寸帽 | 桌面安全审计标准（上限检查必须在分配之前） |
| 上传目录 `app_data_dir()/uploads/mcpb` | origin 用 `process.cwd()/data/uploads/mcpb`（服务端部署形态） |
| admin 门控 = `require_admin`（skipAuth 放行） | 桌面免登录单用户语义，全仓统一 |
