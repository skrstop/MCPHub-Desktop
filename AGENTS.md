# MCPHub Desktop (Tauri) — Agent 开发文档（精简版）

> 供 AI Agent / 开发者续接工作的核心参考。原完整版已备份为 `AGENTS.md.bak`（含逐轮实现细节、复核记录），本文只保留架构、关键差异、MUST FOLLOW 规则与同步基线。

> ⚠️ **核心约束（MUST FOLLOW）**
> 1. **禁止修改 `mcphub-origin/` 下任何原始源文件**（git 子模块，仅作参考与 diff 来源）。
> 2. 所有修改只能在 `frontend/`、`src-tauri/`、`locales/` 内进行。
> 3. 较大修改后必须更新本文档记录。
> 4. 涉及大量未提交改动时**严禁 `git checkout/stash` 丢弃工作树**（vite sourcemap `sourcesContent` 是应急恢复源，见 §3.6 教训）。

---

## 1. 项目概览

### 1.1 原项目（mcphub-origin）

| 属性 | 值 |
| --- | --- |
| 包名 | `@samanhappy/mcphub` |
| 技术栈 | Express.js + TypeScript ESM + React/Vite + Tailwind CSS |
| 认证 | JWT + bcrypt + Better-Auth（OAuth/OIDC） |
| 数据存储 | JSON 文件（`mcp_settings.json`）或 PostgreSQL |
| i18n | react-i18next，翻译在 `locales/` |

### 1.2 桌面端（mcphub-desktop）

| 属性 | 值 |
| --- | --- |
| Tauri v2，Rust crate | `src-tauri/`，应用标识 `app.mcphub.desktop` |
| 前端 | `frontend/`（origin frontend 的有改造副本） |
| 数据存储 | SQLite（`$APPDATA/mcphub.db`，sqlx 0.8，bundled libsqlite3 无条件启用 FTS5） |
| 认证 | jsonwebtoken 9 + bcrypt 0.15 |
| 异步 | tokio 1 full；HTTP 客户端 reqwest（rmcp-openapi 侧 0.13、项目主 0.13，gix-transport 0.12 blocking 共存） |

### 1.3 目录结构（关键路径）

```
mcphub-desktop/
├── frontend/                    # origin 副本（有改造，见 §4.2 清单）
│   └── src/{pages, components/{layout,ui,icons}, utils/, contexts/, services/}
├── locales/                     # en/zh/fr/tr
├── mcphub-origin/               # git 子模块，仅参考
├── src-tauri/
│   ├── tauri.conf.json / Cargo.toml
│   ├── migrations/              # 0001~0025 sql（供 sqlx::migrate! 兼容）
│   ├── runtimes/skill/install.json   # 已知 agent catalog（include_str! 编译进二进制）
│   └── src/
│       ├── lib.rs               # 插件注册、setup、invoke_handler
│       ├── auth/ models/ db/{mod,migration}.rs
│       ├── mcp/                 # client, stdio/sse/http/openapi transport, pool,
│       │                        # session_pool（per-session 隔离）, on_demand（按需启动）,
│       │                        # progress（下载进度/更新检测）
│       ├── services/            # mcp_manager, server/user/group/config/log/bearer_key/
│       │                        # http_server（内置 Axum HTTP）, runtime_env, fts_service,
│       │                        # settings_import, market_service …
│       ├── commands/            # 各 Tauri command 模块（auth/servers/groups/config/logs/
│       │                        # runtime/rag/market/registry/cloud/http_server/cost …）
│       └── rag/                 # service.rs（核心）, git.rs（Git 数据源）,
│                                # chunker.rs, vectordb.rs, extract/{pdf,office,image,text,ocr}.rs
├── servers.json                 # 本地 MCP 市场数据
└── doc/upgrade/{version}.md     # changelog（CI 取为 latest.json 的 notes）
```

### 1.4 数据流

```
React (frontend) ── isTauri() ? invoke() : fetch() ──▶ Tauri IPC
  ──▶ commands/（= 原 controllers） ──▶ services/
        ├─▶ db/（SQLite via sqlx）        ├─▶ mcp/（stdio/sse/http/openapi 连接池）
        └─▶ runtime_env/（Node/Python 版本隔离）
```

---

## 2. 开发环境与命令

```bash
# 前端
cd frontend && npm run dev        # 开发
cd frontend && npm run build      # 构建
npx tsc --noEmit                  # 类型检查（基线 24 个存量错误，新改动零新增即通过）

# Rust（sparse 协议已配置在 src-tauri/.cargo/config.toml）
cd src-tauri && ORT_SKIP_DOWNLOAD=1 cargo check
cd src-tauri && cargo test --lib

# Tauri
npm run dev                       # 开发模式（frontend dev server + Tauri 窗口）
npm run build                     # 生产构建
```

关键规则：

- **sqlx**：只用 `sqlx::query()` 非宏 API（禁 `query!` 宏，需 DATABASE_URL）；迁移走 `db::migration`（见 §3.4.2），不用 `sqlx::migrate!()` 运行时。
- **cargo 源**：`src-tauri/.cargo/config.toml` 强制 sparse registry（`registry = "sparse+https://index.crates.io/"`，末尾斜杠必需）。
- **性能回归测试教训（MUST）**：测试桩与生产实现的成本特征必须同量级（桩 tokenizer µs 级 vs 真实 BPE ms 级差 1000 倍时，"测试绿"对生产无证明力）。真实模型 bench 需 `CARGO_PROFILE_RELEASE_PANIC=unwind`。

---

## 3. 桌面端自定义功能（与 origin 的差异，同步时不可覆盖）

> 完整实现细节见 `AGENTS.md.bak` 对应章节；本节保留接口/落点/关键不变量。

### 3.1 前端基础差异

- **IPC 层**：`utils/tauriClient.ts`（isTauri/mapRestToCommand/invokeMapped + REST→invoke 路由映射）；`utils/fetchInterceptor.ts` 在 Tauri 下把请求路由到 invoke；`services/configService.ts` 的 `getPublicConfig` 用 `apiGet`。
- **免登录**：`AuthContext` skipAuth 模式自动创建 guest admin（默认启用）；`AuthState.skipAuth` 字段供 SettingsPage 等判断。
- **ServerForm**：hub-* 样式；上游 3 分区布局（#1034/#1055）；隐藏 visibility（桌面全部公开，默认 public）；保留 OAuth2 完整配置；不镜像 cookieSession UI；stdio 专属「按需启动」checkbox + idle timeout 输入。
- **ServerCard**：隐藏可见性列；下载进度条、更新角标 +「更新到」菜单项、版本号小字（见 §3.5.1）。
- **Header/LoginPage/Sidebar**：GitHub 链接指向 `skrstop/mcphub-desktop`、移除文档按钮、admin 只读 + 默认密码提示、Logo 用 `/assets/logo.png`。
- **SettingsPage**：隐藏未实现模块（Smart Routing/OAuth Server/Better Auth 等）；RuntimeVersionManager（Node/Python 版本管理）；HTTP 端口设置（`exposeHttp`/`httpPort` 默认 23333）；免登录下隐藏「修改密码」、导出 JSON 直接用 Rust 返回的 pretty 字符串、「下载 JSON」走 `invoke('save_settings_json')` 原生对话框。
- **Dashboard/ActivityPage**：隐藏 SMART/Docs/用户列。
- **Splash**：`frontend/index.html` 内嵌 CSS 动画 + 内联 i18n 脚本（`navigator.language`）；`main.tsx` 挂载后 300ms 淡出移除；StrictMode 已禁用（桌面 dev 不需要，且曾引发 updater effect 双调用问题）。

### 3.2 自动更新（Tauri updater）

- **签名密钥直接明文存仓库**（开源项目，不用 GitHub Secrets）：`src-tauri/updater/mcphub.key`（base64 编码）+ `.key.pub`；公钥复制到 `tauri.conf.json` 的 `plugins.updater.pubkey`。CI（release.yml）用 Python 解码 base64 并 **`.strip()` 尾部空白**后设 `TAURI_SIGNING_PRIVATE_KEY`；Windows runner 需 `PYTHONIOENCODING: utf-8`。
- **bundles 配置（MUST GET RIGHT）**：macOS 必须 `app,dmg`（仅 dmg 不生成 updater 产物）；Windows 必须含 `nsis`；Linux `deb,rpm` 不支持自动更新。
- **平台策略**：macOS/Windows 走 Tauri updater（`canAutoUpdate=true`）；Linux 回退 GitHub `latest.json` 版本号提示手动下载。
- **启动更新检查**：根级 `UpdateCheckContext`（不依赖登录态），检测到新版本自动弹「关于」+ 红点；已移除「忽略此版本」；effect 故意不加 run-once 守卫（StrictMode 相关历史坑，StrictMode 现已禁用）。
- **安装进度可视化**（AboutDialog）：`DownloadEvent` 流驱动 `installPhase`（downloading/installing/done/error）+ 下载百分比/速度（EMA）；安装阶段无百分比事件，用 indeterminate spinner。
- **release notes**：`Markdown.tsx`（react-markdown + remark-gfm）渲染 `latest.json` notes（= `doc/upgrade/{version}.md` 全文）。
- **changelog 格式规范（MUST FOLLOW）**：`doc/upgrade/{version}.md`，分节固定「新功能 → 修复 → 限制（可选）→ 基线同步（仅含 origin 同步时）」，面向用户、关键特性名加粗。

### 3.3 内置 HTTP 服务器与安全端点

- `services/http_server.rs`（Axum）：MCP Streamable HTTP（JSON-RPC）、Bearer Key 认证、`/mcp`、`/mcp/$smart`、`/mcp/{group}`、`/mcp/{server}`、`/rest/*`、`/health`、`/.well-known/oauth-protected-resource`。
- **OpenAPI 兼容端点（/api/*，2026-09-24）**：镜像 origin openApiController——`GET /api/openapi.json`、`/api/openapi/servers|stats`、`/api/{name}/openapi.json`、`GET|POST /api[/{name}]/tools/{server}/{tool}`；query 参数 origin 全量 parity（`title/description/version/serverUrl/includeDisabled/group/servers`）；简单参数走 GET query、复杂走 POST body；operationId 裸名 + 跨服务器同名冲突时 `{server}_{tool}` 消歧。桌面差异（有意）：bearer 鉴权更严格（`enableBearerAuth` 开启时 /api 全路由受控）、`.yaml` 返回 JSON、无限流、无 UI-only 工具过滤。Dashboard 有 OpenAPI 端点卡片。
- **安全语义**：禁用工具 gate（REST `/rest/*` 与 MCP JSON-RPC 路径同源，#1178 镜像）；活动日志记录客户端 IP（`source_ip`）；工具调用载荷 input/output/error_message 各截断 64KB。

### 3.4 Rust 后端关键机制

#### 3.4.1 免登录与鉴权

- `get_public_config` 返回 skipAuth/permissions；`require_admin`（config.rs）在 skipAuth 下直接放行（`is_skip_auth_enabled` 提为 `pub(crate)`）。⚠️ `bearer_keys.rs` 有独立副本 `require_admin`，未放行（按需修）。
- `save_settings_json`：tauri-plugin-dialog 原生另存为 + 写盘；取消返回 `Err("cancelled")`，前端静默。
- Tauri webview 不支持 blob-URL 下载——下载类功能必须走原生对话框。

#### 3.4.2 DB 版本化迁移（MUST FOLLOW）

- `db/migration.rs`：`schema_version` 表 + `run_pending()` 幂等执行；兼容旧 `_sqlx_migrations`。当前 `TARGET_VERSION = 25`（以源码为准）。
- 新增迁移步骤：①递增 `TARGET_VERSION`；②新增 `migrate_v{N}`；③`apply_migration` 加 match 分支；④配套 `migrations/000N_xxx.sql`；⑤更新文档。
- 迁移前自动备份：有 pending 时 `VACUUM INTO` 生成 `mcphub.db.bak`（回滚：关应用 → .bak 覆盖 mcphub.db → 删 -wal/-shm → 重启）。
- 迁移保证 schema 完整后，service 层 SQL 直接引用全部列，**不需要运行时列检测**。

#### 3.4.3 Windows 打包与静默执行

- NSIS `installMode: "both"`（当前用户/所有用户）。
- 所有 `std::process::Command` / `tokio::process::Command` 在 Windows 加 `#[cfg(windows)] { c.creation_flags(0x0800_0000) }`（CREATE_NO_WINDOW）；需导入 `std::os::windows::process::CommandExt`（tokio 经 Deref 继承）。覆盖：stdio_transport、runtime_env、commands/runtime 全部子进程调用点。
- 捆绑运行时：Python 3.14 / Node 24.18.0 / uv 0.11.24（`scripts/download-runtimes.{sh,ps1}`）。

#### 3.4.4 SQLite 全文索引（FTS5，`fts_service.rs`）

- 7 张 FTS5 虚拟表 `fts_{servers,groups,rag_docs,skills,prompts,resources,app_log}`（`ref_id + zh + py + ini`）；分词 charabia（**严禁** `chinese-normalization-pinyin` feature）+ pinyin 0.10（`plain`）。
- 查询路由：CJK → zh 短语 OR；纯 ASCII → py/ini/zh 前缀 OR；token 转义 `"`→`""`；空/纯符号 → None。
- **一致性铁律**：源表写与 FTS 同步必须在**同一事务**（`*_tx` 版本函数）；`clear_logs` 曾因用 pool 版 clear_table 造成死锁（已删 pool 版，只剩 `clear_table_tx`）。
- 搜索：`search_ref_ids_weighted`（逐 token 命中数计数、稳定排序保留原生序 tiebreak）；FTS 零结果/空表/Err 一律降级 LIKE。命令层（如 `search_servers`）不得用按名重排覆盖 weighted 序。
- SQL 字符串经 OnceLock 缓存为 `&'static`，不得每次 `format!.leak()`（曾致真实内存泄漏）。

#### 3.4.5 其他后端机制

- **日志**：`app_logger::log_to_db` 是唯一收口（截断 4000 字符）；前端 `log_event` command 写 `[update]` 等前缀日志，日志页可按来源过滤。保留 15 天 + 定时（6h）/手动清理 + VACUUM。
- **上下文占用**：`get_server_costs`/`get_group_costs`（约 4 字符 = 1 token）。
- **stdio stderr**：`stderr_tail` 32KB 滚动缓存，连接失败拼进 error 便于排查；单行日志截断 2000 字符。

### 3.5 MCP 连接机制（桌面端独有/镜像）

#### 3.5.1 stdio 包下载进度 / 更新检测 / 非阻塞连接

- 保存类命令（update/reload/reinstall/toggle）**后台 spawn 连接**，立即返回 `starting`，不被连接阻塞。
- `mcp/progress.rs`：`server://install-progress`（下载中/done/error，stderr 行匹配 + 300ms 节流）；`server://update-available`（连接成功后对 npx/uvx 跑一次，**非定时巡检**）。事件 payload 必须 `#[serde(rename_all="camelCase")]`（曾踩坑）。
- 更新检测对比**持久化的已安装包版本**（`config_json.packageVersions`），不用 serverInfo.version；`mark_reinstalled` 防重装后重复提示。已记录版本可改 DB 模拟更新（详见 .bak §3.6.5）。
- 前端 `ServerInstallProgressContext` 监听两事件，ServerCard 显示进度条/角标/菜单项。

#### 3.5.2 per-session client 隔离（origin #985 镜像）

- `perSessionClient: true` 时仅 HTTP MCP `tools/call`（有 `mcp-session-id`）走 `mcp/session_pool.rs` 独立 client/进程；REST 与 Tauri 命令走共享 pool。DELETE session 时 `cleanup_session`。与 startOnDemand **互斥**（create/update 校验）。

#### 3.5.3 on-demand 按需启动（origin #1012 镜像）

- `startOnDemand: true`（仅 stdio）：启动插「睡眠」占位不连接；首调冷启动（`mcp/on_demand.rs`）；空闲超时（默认 5 分钟）自动关进程但保留工具缓存。自动重连循环跳过 on-demand。#1164 镜像：调用开始前 bump `last_used` + 重 arm idle timer。

#### 3.5.4 编辑服务器避免无谓重连（origin #1055 镜像）

- `update_server` 先 `has_connection_relevant_change` 比对（序列化 ServerConfig → 剔除 id/name/description → 60000 超时视为未设置 → strip nulls → 深比对；`enabled` 保留在比对内）；无连接相关变更仅持久化、保留实时连接。
- `proxy`（Proxychains4）已持久化（DB v20）作 round-trip，**运行时未消费**（未接 proxychains）。

### 3.6 RAG 功能族（桌面端独有，量大，此处为索引）

| 特性 | 关键落点 | 要点 |
| --- | --- | --- |
| MCP 文件工具（`rag_file_create`/`rag_file_update`） | `rag/service.rs`, http_server dispatch_mcp | create 用 `{docName}.{docType}` 命名、`content_path_for` 兼容多命名；update 对 symlink 只读 |
| 导入方式 symlink/copy + md5 更新检测 + 批量更新 | `DocMeta{method, original_path, md5}`（meta JSON，无 DB 迁移） | `classify_original` 兼容旧 meta；**META_LOCK 顺序锁**（锁序恒为 meta → runtime，严禁反向 ABBA）；`write_meta_atomic` tmp+rename；坏 meta skip 不拖垮列表 |
| 自动定时更新 + 文档/分片分页 | RagSettings{autoUpdateEnabled, interval(≥60s), docLoadChunkKb} | 自动 tick 与手动批量共用 `BATCH_UPDATE_RUNNING` CAS；空转放行条件含 added/removed；空转但有 lost 时发 `rag://docs-invalidated` 轻量刷新 |
| PDF/Office 提取 + 图片 OCR | `rag/extract/{pdf,office,image,text,ocr}.rs` | pdf_oxide + office_oxide；OCR 自研三平台（macOS Vision / Windows WinRT / Linux tesseract chi_sim+eng）；提取文档强制 `method=copy`；扫描件无文字层报 `EXTRACT_FAILED` |
| 数据源统一 + Git 数据源 + 树形视图 | `rag/git.rs`（gix 0.73）、`RagDocTree.tsx` | 支持 https/http/SSH（含密码 askpass wrapper、BatchMode、setsid、stderr 过滤 wrapper）；凭证存 `<app_data>/rag/git-credentials.json` 0600（弃用 keyring——会弹授权框打断无人值守流程）；`canonicalize_url`（`.no_proxy()`，处理 http→https 重定向剥凭证）；数据源级同步（新增导入/删除移除，双重条件防误删）；删除后引用归零自动清 clone+凭证 |
| Git 失效提示 | `GitSourceError{url,branch,auth,message}` | auth=true → 引导重输账密；进度弹框仅在确有失败时显示 ⚠ |
| 文件排除 | `config_json.rag.excludedPaths`（目录条目前缀匹配） | 排除 ⇄ 勾选互斥；不阻断手动上传更新 |
| 数据源级手动更新/排除/别名 | `refresh_rag_source` / `preview_rag_source_update` / `set_rag_source_alias` | 树形数据源/目录行刷新按钮走批量同流程（确认框→进度弹框）；tauriClient 路由 `/rag/source-update` 分支须 `segs.length === 2` |
| 导入会话守卫 | `IMPORT_SESSION: AtomicBool` + `rag://import-cancel-requested` | 导入期间 git 刷新/自动 tick 全 defer；**关窗 = 取消剩余导入**（`CloseRequested` 请求取消，完成的文件保留） |
| 列表⟺向量一致性 | `vectordb.rs::count_rows` + `start()` 检查 | lancedb 空表但有已索引 meta → 判定向量丢失，重走 needs_reindex |
| 分片路由与防挂死 | `chunker.rs` | 路由：提取产物(md)→Markdown、`.md`→Markdown、代码→CodeSplitter、其余→Text；**无定长窗口、无降级、无外部格式化**（linthis 已撤回）。防挂死三件套：①`TokenChunkSizer` 快速否决（len > capacity×64）；②超长 chunk 安全阀硬切（`split_oversized_chunks`）；③CodeSplitter **AST 预分区**（>256KB 按最浅可分节点切分区，release 实测 mermaid.min.js 3.3MB 从数小时挂死 → 37s）。单条 `embed()` 与 `embed_batch` 均按 max_context 截断 |
| 嵌入性能 | `gguf.rs::embed_batch` 长度分桶 | 按长度排序分桶 forward（零语义变化），padding 浪费 6x→1.1x；`GIT_REFRESH_TTL_SECS=600` 防 git 刷新风暴 |
| 导入进度 UI | `rag://upload-progress` + `batchTiming` | 字符/分片/嵌入多进度条 + 耗时显示；字符条分母 = chunk 字符和（与累加口径一致）；分片期显示「分片中…」占位 |
| 文件类型图标/查看 | `utils/fileIcon.tsx`、`FileTypeRenderer.tsx` | Markdown/代码高亮/纯文本按类型渲染；tree 渲染顺序 = 直属文件在前、子目录在后 |
| 前端勾选状态 | `RagPage.tsx` 统一 `applySelected` 入口 | **Set/Map state 必须不可变更新**（先 `new Set()` 复制再增删）——原地突变会被 React `Object.is` 吞掉更新（曾深挖钉死）；ref 同步 + 绝对写入双保险 |

**事故教训（MUST REMEMBER）**：曾因 `git checkout -- RagPage.tsx` 丢失 9 天未提交改动，靠 Vite sourcemap `sourcesContent` 恢复。大范围 JSX 重组应小步替换 + 每步 tsc 验证。

### 3.7 Skills / agents catalog

- `runtimes/skill/install.json`：65 条（对齐上游 vercel-labs/skills 77 agent，同路径合并复合名，含桌面自定义 `.agents/skills` 与 `.config/agents/skills`）。
- agents 列表持久化在 `config_json.skills.agents`，`list_agents` 仅在键缺失时回退 catalog——**catalog 变更需迁移回填**（migrate_v25 幂等追加缺失 id）。

---

## 4. 上游 mcphub-origin 同步记录（重要）

### 4.1 同步策略

1. `mcphub-origin/` 是 git 子模块，仅作参考与 diff 来源，**永不修改子模块内容**。
2. `frontend/`、`locales/` 是 origin 的**有改造副本**：大部分文件与 origin 一致；desktop 改造文件（§4.2 清单）同步时**必须手动合并保留差异**。
3. Rust 后端不直接同步 Node 代码，但需逐 commit 评估安全/重要 fix 是否镜像。
4. `package.json`、lockfile、docs/、Docker/docker-compose 等不同步。

### 4.2 桌面端自定义文件清单（同步时不可直接覆盖）

| 文件 | 自定义内容 |
| --- | --- |
| `components/ServerCard.tsx` | 去 sponsor/wechat/discord；下载进度条/更新角标/版本号（3.5.1）；`startOnDemand` prop；`/api/` 端点 chip；duplicate/client-config 对话框 |
| `components/ServerForm.tsx` | hub-* 样式、隐藏 visibility、默认 public、保留 OAuth2、3 分区布局、按需启动字段 |
| `components/ui/StatusDot.tsx` | `startOnDemand` + 💤 Sleeping |
| `components/LogViewer.tsx` | source 类型 string[]、去 source filter、滚动方向 |
| `components/layout/{Header,Sidebar}.tsx` | GitHub 链接、Logo 图标 |
| `components/ui/UserProfileMenu.tsx` | 去 sponsor/wechat/discord；更新检查上移根级 provider |
| `components/ui/AboutDialog.tsx` | Desktop 标识、canAutoUpdate、Markdown notes、安装进度可视化、去「忽略此版本」 |
| `components/ui/Markdown.tsx` | 桌面端新增（react-markdown+remark-gfm） |
| `contexts/AuthContext.tsx` | skipAuth/guest 模式 |
| `contexts/SettingsContext.tsx` | httpPort/exposeHttp；SmartRoutingConfig 字段随上游演进 |
| `contexts/UpdateCheckContext.tsx` | 桌面端新增：启动更新检查 + 全局 AboutDialog |
| `contexts/ServerInstallProgressContext.tsx` | 桌面端新增：安装进度/更新事件 |
| `App.tsx` | 包入 ServerInstallProgressProvider + UpdateCheckProvider |
| `types/index.ts` | `Server.version`、`AuthState.skipAuth`、`startOnDemand`/`idleTimeoutMs`、RAG 相关类型 |
| `services/configService.ts` | getPublicConfig 用 apiGet |
| `services/changelogService.ts` | Tauri 中 changelog 桩；`buildChangelogFromTauriUpdate`；去「忽略此版本」 |
| `utils/version.ts` | `checkForAppUpdate(source)` + `logUpdateEvent` 全流程 `[update]` 日志 |
| `utils/tauriClient.ts` | 桌面端新增（REST→invoke 路由）；`serverVersion` 映射 |
| `utils/serverFormPayload.ts` | 按 serverType 构建 config；perSessionClient/startOnDemand 携带 |
| `utils/fetchInterceptor.ts` | isTauri() 拦截 |
| `pages/SettingsPage.tsx` | 隐藏未实现模块、RuntimeVersionManager、HTTP 端口、免登录适配 |
| `pages/LoginPage.tsx` | admin 只读 + 默认密码提示 + Logo |
| `pages/Dashboard.tsx` | 隐藏 SMART/Docs；OpenAPI 端点卡片 |
| `pages/ActivityPage.tsx` | 隐藏用户列、createdAt 处理 |
| `frontend/index.html` | Splash（同步时不可覆盖） |
| `locales/*.json` | 保留桌面自定义键（runtime*/rag*/server.downloading 等），只增 origin 新键 |

### 4.3 同步操作 SOP

```bash
# 1. 更新子模块
cd mcphub-origin && git fetch origin && git checkout origin/main && cd ..
# 2. 列出待同步提交（基线 = §4.4 的 SHA）
cd mcphub-origin && git --no-pager log --oneline <last-sync-sha>..HEAD
# 3. 生成 patch + dry-run
git --no-pager diff <last-sync-sha>..HEAD -- frontend/ locales/ > /tmp/origin_frontend.patch
patch -p1 --dry-run --batch --forward --no-backup-if-mismatch -F 5 < /tmp/origin_frontend.patch
# 4. 逐文件处理（禁止批量覆盖！无自定义 → cp；有自定义 → 手动合并；locales 只增不删桌面键）
# 5. 评估 Node 后端 commit 是否需 Rust 镜像
# 6. 验证：npm run build / npx tsc --noEmit（24=基线）/ cargo check / locales json 校验
# 7. 更新 §4.3 基线 + §4.4 条目（末尾写「影响功能点与结果」，MUST）
```

### 4.4 最近同步基线

> 版本号规则：跟随 origin 最新 tag 的 `{{version}}`，桌面端为 `{{version}}xxx`（xxx 从 001 递增）。

| 项 | 值 |
| --- | --- |
| **当前已同步到 origin commit** | `f8615ab`（origin/main，v1.0.40 tag 之后 1 个未发布提交） |
| **对应 origin tag** | `v1.0.40` |
| **桌面端版本号** | `1.0.40001` |
| **同步执行日期** | 2026-09-24 |

> 下次同步以 `f8615ab` 为基线起点（`git log --oneline f8615ab..HEAD`）。

### 4.5 同步记录

#### 2026-09-24（第二轮）：`8ed6478` -> `f8615ab`（2 commit，v1.0.40 + 1 未发布）

- `984028e` #1205/#1206：pg 连接池加固 —— **无需镜像**（桌面 sqlx SQLite 本地，无 pg-pool）。
- `f8615ab` #1209：Smart Routing 配置键白名单 —— **无需镜像**（Smart Routing 未实现；`config_service::update` JSON 深合并透明 round-trip）。
- 无 frontend/locales/Rust 改动；版本 `1.0.39001 → 1.0.40001`；changelog `doc/upgrade/1.0.40001.md`。影响功能点：无。

#### 2026-09-24（第一轮）：`6b1fdb7` -> `8ed6478`（43 commit，跨 v1.0.36~v1.0.39）

**前端/locales 已同步**（节选）：
- `b3d0dcb` #1143 tsconfig 去 baseUrl；`451102d` #1142 GroupCard 复制用 group 名；`ae5962c` #1153 log-stream 重连退避重置；`fb60844` #1160 slogan 措辞。
- `319e191` #1129 per-user credentials：**部分采用**（locales/类型/ServerConfig.resources/ServerForm credentials slots）；跳过 CredentialsPage 路由与入口（桌面免登录单用户）。
- `1772c7a` #1175 分组可见性：**仅类型 + locales**（桌面无用户体系，跳过 UI）。
- `629ff61` #1180 `/api/` 端点 chip（ServerCard/GroupCard）；`ee124ff` #1186 包版本展示类型（运行时用自研 §3.5.1）。
- `a793e38`/`47cd1fc` #1190/#1193 服务器复制到预填表单：完整镜像（`serverDuplicate.ts` + Add/Edit/Servers/ServerCard 接线 + 竞态守卫）。
- `93fdb6f` #1191 per-client MCP 配置预设：完整镜像（`clipboard.ts`/`mcpClientSnippets.ts`/`CopyClientConfigDialog.tsx`）。
- `5ac617f` #1184 OAuth client TTL：前端镜像（桌面无 OAuth server 后端）；`aa46861` #1199 Smart Routing 索引面板：代码保留对齐（Tauri 下隐藏）。

**Rust 镜像**：#1178（REST disabled-tool gate）、#1164（on-demand 长调用保活）、#1182（npx scoped 缓存清理 `clear_npx_cache_for_specs`）。

**评估无需镜像**（节选）：#1157（桌面有 is_starting 守卫）、#1156（独立 spawn 天然隔离）、#1149（session 策略 fallback 天然成立）、#1162（暂不镜像，已有 staggered startup）、#1135/#1155 等限流（无登录端点）、OAuth/BetterAuth/per-user credentials 后端（无链路）、依赖/CI/文档类。

- 版本 `1.0.35003 → 1.0.39001`；changelog `doc/upgrade/1.0.40001.md`（合并单文件）。
- **影响功能点**：Servers/Group 卡片复制与端点 chip、客户端预设对话框、SettingsPage OAuth TTL + Smart Routing 面板（隐藏）、logService 退避、Rust 三处安全/稳定性修复、locales +82 键。
- **结果**：新增服务器复制/预设复制/OpenAPI 端点展示；修复禁用工具 REST 调用漏洞、on-demand 长调用误关、npx 重装波及、日志流退避、编辑竞态。**用户需重启生效**。

#### 2026-09-08：`980ab4a` -> `6b1fdb7`（8 commit，v1.0.35）

- `6043a1f` #1133 Smart Routing 字段 provider-neutral 更名（前端完整镜像 + 4 locales）；`6b1fdb7` #1141 `.btn-primary` 边框（index.css）；#1137/#1138/#1140 无需镜像（multipart/OpenAPI 序列化由 rmcp-openapi 库处理；skill 元数据）。
- 版本 `1.0.34003 → 1.0.35001`；桌面运行行为不变。

#### 2026-09-07：`a67165e` -> `980ab4a`（1 commit，MRL 维度透传）

- #1131/#1132 前端完整镜像（`embeddingDimensionsApiPassthrough`）；后端无落点（Smart Routing 未实现）。版本 `1.0.34001 → 1.0.34002`。

#### 2026-09-07：`8030868` -> `a67165e`（纯文档，无代码落点）；2026-09-06：`40e7c74` -> `8030868`（3 个 Node 后端修复均无需镜像：startOnDemand 持久化桌面已有专属列、OAuth 重连无链路、multipart 由库处理）。版本 `1.0.33102 → 1.0.34001`。

#### 2026-09-01：`41d34be` -> `0f59780`（两轮合并发布 `1.0.33002`）

- #1113 embedding provider presets + dimensions（前端完整镜像，10 locales 键/语言）；#1115 slogan 措辞（locales）。
- 安全修复评估无需镜像：activity/log REST 加 admin（桌面无该 REST 面）、SSRF redirect 校验（无 CIMD/出站拉取链路；注：reqwest 默认跟 10 跳，未来引入「从不可信 URL 拉取」需评估 SSRF）。

#### 历史记录

> 2026-07-24 ~ 2026-08-25 共 11 轮同步已精简，逐 commit 评估见 `git log` 对应提交的 AGENTS.md 版本或 `AGENTS.md.bak`。

---

## 5. 已知问题与解决方案

| 问题 | 解决 |
| --- | --- |
| Cargo 访问 crates.io 慢/卡 | sparse 协议（`src-tauri/.cargo/config.toml`） |
| sqlx `query!` 宏需 DATABASE_URL | 全部用 `sqlx::query()` 非宏 API |
| SSE 连接失败 | sse_transport 正确跟踪 event 类型，支持多种 endpoint 格式 |
| MCP 用系统 Node/Python | runtime_env 版本隔离 + `resolve_command()` |
| FTS 同事务 / clear_logs 死锁 | 只用 `clear_table_tx` 同事务版本（pool 版已删） |
| React state 原地突变被 Object.is 吞更新 | Set/Map 不可变更新（复制后增删） |

---

## 6. 当前状态与待办

### 已完成（节选，全量见 .bak §7）

基础架构 / 全部 Tauri 命令 / 前端适配器 / 托盘 / 免登录 / 运行时版本管理 / 内置 HTTP 服务器 / Bearer Keys / Prompts & Resources / Activity Log / Market / Registry & Cloud Proxy / SSE 改进 / DB 版本化迁移 / OpenAPI 传输 / starting 状态 / 日志清理 / 工具禁用同步 / Context Footprint / 系统日志面板 / Splash / stdio 下载进度与更新检测（3.5.1）/ 启动更新检查 + Markdown release notes + 安装进度可视化（3.2）/ 版本号四源同步 / on-demand 按需启动（3.5.3）/ stderr 诊断（3.4.5）/ 无谓重连避免 + proxy 持久化（3.5.4）/ FTS5 全文索引 + 活动日志筛选 + 日志检索（3.4.4）/ RAG 全功能族（3.6）/ OpenAPI 兼容端点（3.3）/ agent catalog 补齐（3.7）

### 待办

- [ ] Smart Routing（智能路由）
- [ ] OAuth Server / Better Auth 集成
- [ ] Tool Result Compression
- [ ] MCPB/DXT 文件安装
- [ ] Templates（配置模板）
- [ ] CI/CD 打包配置
- [ ] proxy（Proxychains4）运行时接入
- [ ] RAG 树形虚拟滚动（量大时）

### 版本号四源（发布时必须一致）

`src-tauri/tauri.conf.json`（也是 `import.meta.env.PACKAGE_VERSION` 来源）、`src-tauri/Cargo.toml`、根 `package.json`、`frontend/package.json`（+ `Cargo.lock`）。当前 **1.0.40001**。
