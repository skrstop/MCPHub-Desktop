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

### 3.8 Smart Routing 本地化移植（Phase 1-4，桌面端独有，2026-09-26）

> 复刻 origin Smart Routing 到本地模型栈（无 API provider/pgvector 依赖）。设计与五轮复核记录：`doc/smart_routing_port_plan_20260925.md`（§6 各 Phase 落地记录 + §10.4 复核表）。

- **mv 共享运行时**（`src/mv/`，Phase 2）：进程单例 GGUF 嵌入模型 + lancedb 连接，消费方注册制（`rag`/`smart`）——任一消费方开启即运行，全部关闭才 drop + mimalloc 归还。模型选择单写 `mv.model`（读取回退 `rag.model` 兼容存量）；`mv.device`（AUTO/GPU/CPU）经 `load_embedder_with_user_platform` 生效（env > user > deploy.json）。`rag::start/stop` 改为 mv 消费方化，`rag.indexedModel` 新键修复同维度换模型不重嵌缺口（reindex_all 完成后回写）。
- **smart_routing 模块**（`src/smart_routing/`，Phase 3）：lancedb `smart_tool` 表（每工具+每服务器一行，cosine）；toolSetHash **origin 逐参数复刻**（scrypt N=2048/r=8/p=1，上游描述免疫——`{name, inputSchema, description: null}` 归一形状）；skip-check 四条件（count+contentIds+hash+server 行）；生命周期钩子（connect→后台索引 / delete/rename/disable→清行 / update→re-save / boot→reindex_all）。混合检索 `merge_hits` 纯函数（vw+kw 加权、阈值、max_results；**不用** origin 动态阈值）。
- **$smart 端点**（Phase 4）：`/mcp/$smart(/{group})` 只暴露元工具 `search_tools`/`describe_tool`/`call_tool`（origin 逐字 description）；call 经共享 pool（on-demand 唤醒天然生效）；`/api/$smart/openapi.json` + `/search`、`/describe`、`/call` REST（bearer 鉴权）。`$smart` 范围 prompts/resources 返回空。RAG builtin server 不入 smart 索引（与 origin 一致）。
- **前端**：设置页【模型和向量】卡片（真实维度 `RagStatus.embedDim`/`mvRunning`）+ Smart Routing 区块桌面化（检索设置滑条+权重联动、serverDescriptionMode、桌面隐藏 API provider/dbUrl/维度/MRL/嵌入前缀）+ Dashboard/AccessUrlDialog SMART 端点行 + SmartRoutingIndexPanel 经 tauriClient 映射接线。
- **Tauri 命令**：`smart_routing_status` / `smart_routing_reindex` / `smart_routing_performance`。
- **已知边界**：设备变更在模型下次启动时生效（与 UI 提示一致）；`replace_server` 非事务（失败自愈重建）；LIKE 通配符 %/_ 不转义（与 RAG keyword_search 一致）；12 项 RAG 冒烟 + MCP 客户端全链 E2E 待用户真机验证（清单在计划 §8.2）。

### 3.9 MCP 协议 2026-07-28 支持（策略模式新策略 + dispatcher 落点，桌面端独有，2026-09-26）

> 「现代（modern）」无状态修订版。规范核实来源：`modelcontextprotocol.io/specification/2026-07-28`（changelog / versioning / server/discover / patterns/subscriptions）。**类库评估结论**：桌面端下游 MCP 服务端（`http_server.rs` dispatch_mcp）为手写 JSON-RPC，无 rmcp 核心 SDK 可复用 → 按既有策略模式自实现。

- **策略**（`services/mcp_version.rs`）：新增 `V2026_07_28`（registry 第 5 个）。trait 新增 `is_stateless()` 钩子（默认 false，仅 2026 为 true）。capabilities：core tools/prompts/resources + **tasks 移入 `extensions["io.modelcontextprotocol/tasks"]`**（不再是核心 `tasks` 键）。`requires_version_header()` 返回 false——无 session 可约束，版本走每请求 `_meta`。
- **版本路由**（`http_server.rs::dispatch_mcp`）：请求 `params._meta["io.modelcontextprotocol/protocolVersion"]` 非空时——支持的版本（2026-07-28）路由到该策略（无状态，不查 session 策略表）；不支持的版本返回 **UnsupportedProtocolVersionError（-32022）**，`data` 带 `supported`/`requested`。legacy 客户端不会带该键，既有 session 路径零改动。
- **resultType 注入**：`jsonrpc_response_s(id, result, stateless)` 统一给无状态路径的**所有成功 result** 注入 `"resultType": "complete"`（幂等 `or_insert`）；dispatch_mcp 内全部成功响应改走 `ok()` 闭包。legacy 路径不注入。
- **`server/discover`**（规范强制）：返回 `supportedVersions`（全部注册版本）+ capabilities + `_meta["io.modelcontextprotocol/serverInfo"]`（MCPHub Desktop / CARGO_PKG_VERSION）+ `ttlMs: 3600000` + `cacheScope: "public"`（静态信息）。
- **CacheableResult**：`with_cache_hint()` 给 `tools/list` / `prompts/list` / `resources/list` / `resources/read` 成功结果加 `ttlMs: 30000` + `cacheScope: "private"`（内容随 bearer key/scope 变化；hub 不广播 listChanged，hint 偏保守）。
- **`subscriptions/listen`**（替代 GET server-push 流 + resources/subscribe）：最小实现——校验后先发 `notifications/subscriptions/acknowledged`（SSE `message` 事件，`_meta["io.modelcontextprotocol/subscriptionId"]` = 请求 id，honored filter 为空 `{}`——hub 未广播 listChanged、builtin 资源不变），随后流保持打开仅 25s keep-alive comment，客户端断开即结束。通知永不流动（能力未声明，合规退化实现）。
- **`ping`**：2026 已移除——无状态路径回 `-32601 Method not found`（legacy 保持空结果）。
- **initialize 兼容**：混合客户端仍可能发 initialize——`strategy_for` 正常协商出 2026 策略时正常回 capabilities/serverInfo，但**不 mint `mcp-session-id`**（无状态）。
- **语义边界**：无 session → `tools/call` 走共享 pool（perSessionClient 天然不适用）。
- **2026 扩展版 tasks（`io.modelcontextprotocol/tasks`，2026-09-27 补全）**：
  - **Server-directed 建任务**：2026 请求在 per-request `_meta["io.modelcontextprotocol/clientCapabilities"].extensions` 声明该扩展后，`tools/call` 一律返回 **CreateTaskResult（平铺**：`resultType:"task"` + `taskId/status/createdAt/lastUpdatedAt/ttlMs/pollIntervalMs`，ext-tasks v1 schema；⚠️ 须在 `ok()` 前预置 `resultType:"task"`，否则被 or_insert 默认 complete 覆盖——E2E 踩过）；未声明的 2026 客户端永不同步结果（规范：Never return a task to a client that did not declare support）。
  - **`tasks/get` 扩展形**：平铺 + `ttlMs`/`pollIntervalMs` 命名；终态 completed 内嵌 `result`（含 resultType:complete）、failed 内嵌 `error{code,message}`（`mcp_tasks::get_ext`）。
  - **`tasks/update`（新增）**：params 须带 `inputResponses`；hub 任务永不 input_required，按规范 ack 空 result + 忽略 payload；未知 id -32602。
  - **`tasks/cancel`**：2026 回空 result（cooperative ack）；2025-11 保持回 task 快照。
  - **代际门控**：2026 下 `tasks/result`/`tasks/list` → -32601（扩展重设计已删）；`tasks/update` 仅 2026（legacy -32601）。2025-11 核心版四方法完全不变。
- **十轮代码级复合复核（2026-09-28）**：R4 修复 NATIVE_PEERS 无界增长（Peer 无稳定 id，重连累积 clone——加 128 容量上限）；R9 全量回归 41/41（脚本补 json_response 解析兼容）。**新改进 `json_response=true`**：2026 modern 请求直接回 application/json（无 SSE keepalive 空帧），修复简化客户端（codemoss-ide）JSON.parse 崩溃；legacy 有状态会话仍 SSE（rmcp 硬编码），客户端须按 Content-Type 分流（规范要求）。详见测试报告 §12。
- **复核第十轮（2026-09-30，R101-R107：7 个独立逐行扫描代理 + 亲核修复落盘 + 第 16 套件）**：7 代理分段扫 http_server.rs 全文/rmcp_bridge.rs 全文/传输层 6 文件/pool·session_pool·on_demand + 迁移清理终审（rmcp 3.4.1 vendored 源码逐一对照）；**修复 15 项**：①[H] rmcp_stdio_transport stderr drain 死代码——`TokioChildProcessBuilder::spawn` 无条件覆盖 Command 的 stdio（SDK child_process.rs:150-157，默认 inherit）→ 下载进度事件/32KB stderr_tail/日志截断三项桌面不变量全静默失效 → builder 补 `.stderr(Stdio::piped())`；②[H] rmcp_http_transport 常用路径（无 request_meta）落 SDK `Peer::call_tool` 对 `CallToolResponse::Task` 硬报 UnexpectedResponse（SEP-2663 上游可自主返回任务）→ 改 `call_tool_once`+`map_call_response_once` 恢复旧 A1/A2 契约；③[H] SEP-2243 Mcp-Name 推导漏 prompts/get 与 resources/read|subscribe|unsubscribe（rmcp NAME_FROM_NAME/NAME_FROM_URI，header≥2026 强制校验；三代理独立交叉确认）→ name_source 扩展三分支；④[H] discover 回归——切 SDK StreamableHttpService 后落默认 from_server_info（ttlMs:0+private）→ HubBridge 覆盖 discover 恢复 ttlMs:3600000+public；⑤[M] 组 allow-list 在 `/api/{group}/tools/...` 与 `/rest/group/{group}/call` 不强制（spec 生成有过滤，同边界两 surface 不一致）→ scoped 分支强制（空=fail-closed）；⑥[M] Smart Routing meta 工具经 `/rest/{server}/call` 可达（R52 F2 只修 group 路径）→ builtin+meta 404；⑦[M] loopback watch「已停止」分支经 report_loopback_hijack(false) 复活 running:true 假状态 → 只清 flag；一次性检查补 seed 防重复上报；⑧[M] is_2026_session 忽略请求级 _meta.protocolVersion（与 SDK strip 决定可矛盾）→ 对齐 `context.protocol_version()`；⑨[M] 不可应答 input_required（inputRequests/requestState 双缺，手写 SSE transport 可折叠出）→ 回退 Complete+is_error；⑩[M] 2026 头声明缺 _meta 时注入缺 protocolVersion → 已知版本 echo header；⑪[M] pool 握手失败分支不 disconnect 半建 client（npx/uvx 孙进程孤儿，timeout 分支与 session_pool/on_demand 均有清理唯此处遗漏）→ 补 disconnect；⑫[M] 握手后 list_tools 无超时（占位永久 starting）→ 60s timeout；⑬[L] parse_body_limit "kb" 缺 saturating / start() 锁窗口（100ms sleep 释放状态锁可双 bind）/ parse_progress_pct 数字段 `?` 放弃整行 / stderr_tail 重连不清 / on_demand ptr_eq 拒绝驱逐时仍 mark_sleeping / openapi 16MiB 注释失实 / pool `let _ = request_meta` 误导注释 / 死依赖 tokio-stream 移除 / completion 未宣告却返回空 result → 覆盖 complete -32601。**迁移完整性终审**：client/stdio/streamable-http/下游全 rmcp-native；OpenAPI rmcp-openapi 已 0.31→0.32 统一单栈 rmcp 3.4.1（1.8.0 移除、actix-web 0.13，16 套件回归全绿）；SSE 手写为唯一有据遗留（SDK 无 transport-sse-client feature 实证）；mcp_version.rs repo 级零残留。**新套件 `rmcp_gap_matrix_r107.py` 43 项**：补齐「每版本×每上游传输类型（stdio=codegraph / streamable-http=Idea / openapi=公网IP）×每通道」真实调用交叉（此前 15 套件真实调用只打 openapi）+ tasks 双代全链路 + SEP-2243 修复回归 + bearer×版本 + 边界/严格对照。**版本协商语义澄清**：rmcp 3.4.1 设计即 2026-07-28 无 initialize 握手（命名 2026 按规范回退最新 legacy 2025-11-25，service.rs 注释明示）——9-27 报告 TC-23「协商 2026」系旧手写实现行为。16 套 **600/600 全绿**；cargo check 0 错 0 警；cargo test --lib 86 passed。环境注记：本轮工作区存在并发 IDE 编辑（auth/skill_service/servers/log_service/前端多文件），套件失败先排除 tauri dev 热重建窗口再判真伪。详见 `doc/rmcp_review_r101_r107_20260930.md`。
- **复核第十九轮（2026-10-02 下午，R471-R490：5 代理全文件逐行 + 清理审计 + 亲核修复 + v19 套件 + 全量回归）**：本轮转全文件逐行（此前只按 diff 复核的面：pool/client/session_pool、lib/tray/db/auth、smart_routing 全 6 文件、runtime_env+runtime 全 2450 行）+ 迁移清理专项审计（旧 dispatch 零残留、158 command 全注册、26 迁移一致、6 处 allow(dead_code) 全合理——全绿）。**Medium×4**：①原生菜单 Quit（macOS ⌘Q 主退出路径）用 muda `PredefinedMenuItem::quit`——源码实锤直接 terminate:/PostQuitMessage **永不发 MenuEvent**，`disconnect_all` 永不执行 → MCP 子进程树孤儿 → 两平台改自定义 `MenuItem::with_id("quit")`；②runtime.rs 复制的 `get_unix_path` 谓词未同步（仍拒含空格 PATH 项、缺 `=` 守卫）→ 对齐；③runtime.rs `get_enhanced_path` 无缓存每次 list 同步 5s shell 探测阻塞 async worker → 优先读 `runtime_env::cached_enhanced_path()` + spawn_blocking；④Node 下载累计无上限（OOM）+ **无校验和**（node 二进制执行任意 MCP 代码）→ 300MB 硬顶 + SHASUMS256.txt SHA-256 校验（不匹配硬失败/不可达响亮 warn），新增 sha2/hex 直依；⑤全仓 grep 抓出 6 处 `CREATE_NO_WINDOW` 漏网（tray cmd、rag explorer×2、ocr tesseract×3、skill explorer）。**Low×4**：session_pool 慢路径竞态 insert 孤儿隔离 client（insert 前重查 enabled）；keyword_search SQL cap 无 ORDER BY 不确定子集（fetch cap 10k 交 merge_hits 确定性剪枝）；早期迁移 v2-v11 裸 `.ok()` 吞 ALTER 错误 → 全换 `add_column_if_missing`；migrations/*.sql 历史快照口径 README 钉死。记录不修：run_pending 每步非原子 + v8 非幂等重跑（窗口极窄，需 26 签名重构）。**新套件 `rmcp_supplement_v19.py` 87 项**：bearer×5 版本×4 通道（401/带 key 公网IP 全 lifecycle）、prompts/resources×5 版本真实渲染、tasks 全生命周期（**rmcp ext-only 设计钉死：legacy core tasks 能力不被识别，task-directed 创建 -32021；InputResponses 是 Map 不是数组**；终态 update/cancel -32602 响亮失败）、subscriptions/listen（2026 SSE ack；**legacy -32601=2026-only 设计**）、group 成员泄漏判定、on-demand 唤醒。**验证**：23 套 **1042/1042 全绿**；cargo check 0 错 0 警；cargo test --lib 92 passed；tsc 0 错误。详见 doc/rmcp_review_r19_20261002.md。
- **复核第十八轮（2026-10-02，R458-R470：6 代理逐行 diff 扫描 + E2E 断言审计 + 亲核修复 + 全量回归）**：**High×1**：`update_doc` 漏 `validate_doc_id`（R408 全入口接入时遗漏——`rag_file_update` 公网工具可 `fs::rename` 任意文件搬移/销毁）→ 入口补校验。**Medium×8**：①on_demand `in_flight` 在 future 被取消（600s 超时）时永不清除→空闲定时器永久跳过拆除、npx/uvx 进程孤儿 → `InFlightGuard`（Drop spawn 清除，ptr_eq 守卫）；②smart `esc_like` 丢 `'` 转义（查询含撇号 → $smart keyword 通道 SQL 错误整体失败）→ 补 `''`；③`embed_batch` 缺 eos 保留截断（embed() 有、批路径无，文档分块检索退化）→ 逐行 pop/truncate/re-push；④login dummy bcrypt 59 字节（bcrypt 微秒级拒绝不做 KDF，防枚举修复实际未生效）→ 补 60；⑤clear_cache uv 半边仍删废弃共享目录（uvx 清缓存无效）→ 枚举 `uv-cache-*`/`uv-tools-*`；⑥`get_activity_filter_options` LIKE 无 `ESCAPE '\\'`（全仓唯一漏网，键含 `_` 下拉恒空）→ 补；⑦改名/删除级联缺 `bearer_keys.allowed_servers`（key 静默失权）+ `server_tool_config`（禁用工具改名后复活）→ 同事务补两 cascade；⑧$smart 面漏禁用门（面板禁用的 smart_route_* 仍可调）+ meta.rs 内层 `unwrap_or_default()` fail-open → 桥接层补 fail-closed 门 + Err 即拒绝。**Low×3**：openapi servers/stats 含睡眠 on-demand（spec parity）；rag 内容写 tmp+rename 原子化 ×2。**假阳性钉死**：openapi_transport `Authorization: ******` 两个代理+首轮检查均误报真损坏，od -c 逐字节定谳为读取路径对 `Bearer {}` 打码——`*` 字样疑点必须 od -c 定谳；tasks/result 2026「-32601 已删」系过时文档（R158 语义=2026 客户端可用 modern 字段名），照旧文档补门控破坏 4 套件后回滚。**E2E 断言审计**（独立代理全量 21 套）：round50d bearer 用打码字面量（真 key 未上线）、round50e E5.2 优先级恒真、matrix.py 两处 tautology+discover 假探测、strict_matrix 500 也算「拒绝」、round100 IP 子串回退——6 处修复。**新套件 `rmcp_full_matrix_v18.py` 262 项**：5 版本 × 7 通道（root/scope/group/$smart/REST×2//api）× 宽松/严格 × 真实公网IP（严格 IP 校验）全矩阵 + discover/CacheableResult/ping/tasks 门控/-32022/2024 退役/裸请求升格；SEP-2243 线格式钉死 `=?base64:?…?=`（RFC2047 形式严格模式被 rmcp 拒，vendored mcp_headers.rs:194 核实）。**验证**：22 套 **955/955 全绿**；cargo check 0 错 0 警；cargo test --lib 92 passed；tsc 0 错误；build 通过。详见 doc/rmcp_review_r458_20261002.md。
- **复核第二十七轮（2026-10-03，rag/vectordb+extract + groups/http_server 命令 + 前端 group 组件）**：侵蚀审计 27/27；3 代理。**修复 3 项**：①[M] vectordb ensure_table 自愈缺口——embedding 列缺失/类型错时 need_recreate=false，表永久保留但 add_chunks/search 永久失败（tags 同类场景却 recreate，同函数不对称即信号）→ 两种情形均 recreate；②[M] read_chunks_by_doc embedding downcast 失败 unwrap_or_default → 空 vec 喂 FixedSizeListArray::new（内部 unwrap）→ panic 打崩索引线程 → downcast 返回 Err + add_chunks 前置 len!=dim 校验；③[L-M] maybe_start httpPort as u16 截断（70000→4464 静默错绑）→ 1-65535 校验 + log_to_db 不启动。**记录不修**：groups.builtin_prompts/builtin_resources v12 列 Rust 零读写（接线属产品功能决策）。**证伪 19 项**：SQL 注入/pdf 畸形页/office 递归/编码/连接泄漏/group 重名 FTS/双 bind/成员丢失/snippet 注入/剪贴板泄漏等。**新套件 rmcp_supplement_v27.py 3 项**：RAG builtin 聚合存活 + rag_search 真实调用 + 非法 httpPort 不崩进程。**验证**：cargo check 0 错 0 警；cargo test --lib 102 passed；tsc 0 错；**31 套 E2E 1167/1167 全绿**。教训：同函数内对同类「缺失」处理不一致就是自愈缺口信号；unwrap_or_default 跨构造器边界是 panic 制造机；所有 as 数值收缩都要问超范围行为。详见 doc/rmcp_review_r27_20261003.md。

- **复核第二十六轮（2026-10-03，修复合自我复核 + hub/tasks 重审 + 前端竞态）**：侵蚀审计 29/29 在位；3 代理专查既往修复合引入的新 bug + subscription_hub/mcp_tasks 全文重审（17 项假设全证伪）+ 前端 contexts/pages。**修复 4 项（全部为既往修复合引入/伴生）**：①[M] R25 单遍扫描器引入——空占位符 {{}}（end==2）落「无闭合」分支 → 整个剩余模板 verbatim、后续替换全失效（旧顺序 replace 无此问题）→ Some(2) 独立分支保留字面量前进 + 边界单测；②[L] R25 dirty flag 无条件清——updateSmartRoutingConfigBatch 失败返回 false 不 throw，保存失败仍清保护 → 仅成功清；③[M] ActivityPage fetchData 无乱序守卫——快速翻页/切筛选旧响应后到覆盖新页（表与分页器脱节）→ fetchSeqRef 单调 id 丢弃陈旧响应（含 error 路径）；④[M] ServerContext 启动轮询 tick 重叠——晚到 tick 在他人已切 normal 后再次 startNormalPolling 替换定时器 + 旧 attempt 数据覆盖新状态 → 闭包 phase 令牌 bail。**证伪**：任务 sweep×get/终态 map 无界/订阅者泄漏/hub 锁跨 await/通知乱序/终态 cancel/skill 锁死锁/fts 双插/UTF-8 切片等 17 项。**新套件 rmcp_supplement_v26.py 12 项**：4 legacy 版本 root 公网IP + tools/list 形状（102/102 inputSchema）+ 2026 modern 直达 + 严格/宽松热切换基线 + prompts 渲染链回归。**验证**：cargo check 0 错 0 警；cargo test --lib 102 passed（+1）；tsc 0 错；**30 套 E2E 1164/1164 全绿**。教训：修复合一轮后必须以「自身是否引入新 bug」视角重读（本轮 4 项全属此类）；match 兜底分支要列全输入域（end==2 空名被当非法而实际会出现）；前端 fetch 链 seq/abort 守卫是默认要求。详见 doc/rmcp_review_r26_20261003.md。

- **复核第二十五轮（2026-10-03，侵蚀审计 + prompt 模板 + SettingsPage 全文）**：3 代理（market/cost 全净、prompt/resource、SettingsPage 4524 行）+ 新增修复侵蚀审计。**侵蚀审计**：29 项 R19-R24 修复签名逐一 grep 验证 29/29 在位（9 个 MISS 全为脚本转义伪象）——并发编辑环境下每轮固定开头步骤。**修复 7 项**：①[M] render_template 顺序 replace 模板注入（参数值展开其他占位符 + HashMap 序不确定）→ 单遍扫描重写（首版 end>=4 守卫错，{{a}} 的 }} 在 index 3，单字符占位符全落 verbatim——单测抓到修为 >=3）；②[M] required 参数从不校验，缺失泄漏 {{placeholder}} 进 LLM 内容 → validate_required_args + bridge/commands 双接入（invalid_params）；③[L] prompt/resource LIKE 反斜杠未自转义（声明 ESCAPE 却只转 %/_）→ 补链首 ×2；④[M] 导出 JSON 快照首次 fetch 后永不过期 → 每次打开重取；⑤[M] bearer key 保存失败仍关编辑器丢输入 → throw 传播仅在成功关闭；⑥[M] Smart Routing 同节立即写开关重建 temp 清空未保存草稿 → smartRoutingDirtyRef 守卫 + Save 清 flag。**新套件 rmcp_supplement_v25.py 4 项**：prompts 端到端（list 形状/填参渲染无残留/值含 {{token}}/required 校验 SKIP→Rust 单测）+ 5 个新渲染单测。**验证**：cargo check 0 错 0 警；cargo test --lib 101 passed（+5）；tsc 0 错 + build 通过；**29 套 E2E 1152/1152 全绿**。教训：新实现先配最小输入单测（单字符 {{a}} 即崩的守卫）；if(!state) 守卫的 fetch 要问数据是否过期；每轮开头 grep 历史修复签名防回退。详见 doc/rmcp_review_r25_20261003.md。

- **复核第二十四轮（2026-10-03，日志/FTS + runtime/skill + 契约脚本化）**：2 代理 + v24 套件首测即抓到迁移丢失级回归。**修复 5 项**：①[H] rmcp 迁移丢失——/mcp 通道 tools/call 完全不写活动日志（write_activity 只在 /rest、/api；Activity 页对全部 /mcp 流量盲，潜伏 5 轮 1000+ 用例未发现）→ execute_tool_call 三返回路径全部插桩 log_call_activity（duration/status/args/output/error/source_ip），client_ip_of 提为 pub(crate)；②[H] 日志热路径 O(N)——sync_upsert_tx 每写一条日志先 SELECT rowid WHERE ref_id（UNINDEXED 全表扫，app_log 3 万行写日志自拖慢）→ 新增 sync_insert_tx 纯插入路径（新 UUID 恒新增），add_log 切换；③[M] Node 安装校验失败分支不清理 dest → 复用路径只查存在不查健康，永久误报已安装 → remove_dir_all 对齐；④[M] skill agents 读改写无锁（整数组覆盖后写吃前写）→ agents_lock；⑤[M] export copy × uninstall/delete 同路径并发 → skill_fs_lock。**契约 diff 脚本固化**：scripts/check_frontend_rust_contract.py（Rust ServerConfig × 前端 payload 键 camel↔snake 对账，静默丢弃即 FAIL，CI 可挂；当前 0 mismatch，oauth 为 R10 裁定白名单）。**新套件 rmcp_supplement_v24.py 12 项**：日志吞吐（60 调用计时 + activity 行数）+ FTS 行数一致性（30459=30459）+ 2026 终验（discover 3600000/public、-32022、tools/list 30000/private）+ 宽松 4 版本缺 Accept 回显。套件沉淀：会话 path-local（root 会话打 scope 秒失败）；ping 不写活动日志。**验证**：cargo check 0 错 0 警；cargo test --lib 96 passed；tsc 0 错；**28 套 E2E 1148/1148 全绿**。教训：对每个用户可见面写「操作→应出现 X」断言（活动日志是迁移丢失探测器）；热路径每条 SQL 问最大体量成本；失败分支集合 grep 列全对账。详见 doc/rmcp_review_r24_20261003.md。

- **复核第二十三轮（2026-10-03，服务器生命周期 + 前端表单契约）**：2 代理 + 亲核。**修复 6 项**：①[Critical] passthrough_headers 类型错配——Rust `Option<HashMap<String,String>>` vs 前端恒发 `string[]`，sse/streamable-http 每次保存/新增 serde 整单失败（UI-only 路径，HTTP E2E 不可见，20+ 轮漏网根源）→ Rust 改 `Option<Vec<String>>`（origin 类型一致）+ DB 读写/导入器/单测同步；②[M] delete_server disconnect 在 DB delete 前，窗口期 session-rebuild 重建连接 → 删除后活连接孤儿 → delete 提交后再 disconnect 闭合 TOCTOU；③[M] ServerForm args 含空格被 join(' ')/split(' ') 静默重写（连接相关字段还触发重连）→ quote-aware quoteArg/splitArgs round-trip；④[L] passthroughHeaders 非 .join 数组崩编辑弹窗 → Array.isArray 守卫；⑤[L] 负数 timeout 直达 serde u64 拒整单 → Math.max(1000) clamp；⑥套件 strict 键位修正。**记录不修**：服务级 OAuth 无 Rust 落点（R10 裁定产品决策）。**证伪**：starting 悬挂/rebuild 双 spawn/toggle 全序/级联保留/FTS 一致性/rename 拆除/create TOCTOU 等 9 项。**新套件 rmcp_supplement_v23.py 16 项**：请求版本==响应版本 × 4 legacy × root/scope/group（12）+ 未知版本 fallback + 2026 协商 + 禁用/还原工具列表观测。**验证**：cargo check 0 错 0 警；cargo test --lib 96 passed；tsc 0 错；**27 套 E2E 1136/1136 全绿**。教训：Tauri invoke 参数反序列化是 E2E 盲区（serde 错误只进前端 toast）——应对前端 payload 字段×Rust 模型做类型级 diff 脚本；join/split 对偶编辑器必须做含空格/引号 round-trip 用例；生命周期增/改/删应作为一组检查 TOCTOU。详见 doc/rmcp_review_r23_20261003.md。

- **复核第二十二轮（2026-10-02 深夜，鉴权/配置/前端登录态）**：3 代理返回 + 亲核。**修复 3 项**：①[H] LoginPage 默认密码警告弹窗永不可达（login() 置 isAuthenticated 与 setShowDefaultPasswordWarning 同批，redirect effect 同帧导航，弹窗未渲染即卸载）→ effect 加 !showDefaultPasswordWarning 守卫；②[H] settings_import 丢弃 keep-alive/passthrough 三字段（RawServerConfig 缺 enableKeepAlive/keepAliveInterval/passthroughHeaders，构造硬编码 None，与同文件「不得静默丢弃连接相关字段」注释自相矛盾）→ 补字段+alias+透传，新增 3 单测（round-trip/snake alias/未知顶层容忍）；③[M] 401 僵尸会话（interceptor 只 removeToken 不清 AuthContext，ProtectedRoute 持续放行全请求静默失败）→ 401 跳 /login（防循环）。**证伪**：dummy bcrypt 60 正确、bearer 熵充足、allowed_servers 无跨行孤儿、深合并数组/null 语义一致、配置键位分裂无实害。记录不修：导出 groups 导入忽略（R52 裁定功能增强）。**新套件 rmcp_supplement_v22.py 16 项**：方法不匹配 405×6、未知路径 404、无 CT/错误 CT/空 body 4xx 非 5xx、严格只作用 /mcp（REST 不受影响）+ 缺 Accept 406 vs 宽松放行、同会话 3 并发 ping。套件沉淀：legacy 会话 POST 响应常开 SSE 流（并发断言只读状态行）；strictValidation 键在 mcp 命名空间。**验证**：cargo check 0 错 0 警；cargo test --lib 96 passed（+3）；tsc 0 错；**26 套 E2E 1120/1120 全绿**。教训：状态迁移触发副作用的 effect 必须让被调用方标志位参与守卫；注释承诺要 grep 逐条对账；并发 E2E 先弄清通道是否常开。详见 doc/rmcp_review_r22_20261002.md。

- **复核第二十一轮（2026-10-02 晚，横切面脚本化审计）**：后台复核代理 5/5 空返回（基建抖动）→ 全部改主线脚本化交叉审计 + 亲读。**修复 2 项**：①migrate_v12 裸 `.ok()` 吞 ALTER 错误（R19 只修了 v2-v11，v12 恰好越界漏网）→ add_column_if_missing；②i18n 26 键缺失（accessUrl 命名空间整缺 ×8、errors.failedToUpdate* ×7 + failedToReloadServer zh 错位到 api.errors、pages.rag.scanPhase* ×7、server.unknownError/users.fetchError/auth.user、llmProviderApiKeyDescription fr/tr）→ 4 语言全补齐，递归 diff 零差异。**审计通过**：迁移 26/26/26 对齐（defs/arms/sql）；IPC 160 注册命令双向 diff 零孤儿 invoke（3 个前端零引用死面记录：get_active_node/python_version、stop_http_server）；16 个 /rest+/api handler bearer 门控 16/16（scoped_post 经 execute_openapi_impl 委托实核）；tray/lib quit 路径 + CloseRequested import-cancel + hide 全正确；smart/mv 非测试 unwrap 仅 2 处静态参数 expect，scrypt N=2048 与注释一致。**新套件 rmcp_supplement_v21.py 26 项**：REST/OpenAPI 面 bearer 矩阵（/rest/{server}+group tools/call 真实公网IP、/api/openapi.json+servers+stats、/api/{中文名}/openapi.json、/api/tools GET+POST global+scoped、未知工具 404、bearer-on 401×2+带 key 全通道）；套件修正 `/rest/{server}/call` body 形状 `{tool, arguments}`（origin 同构）。**验证**：cargo check 0 错 0 警；cargo test --lib 93 passed；tsc 0 错；**25 套 E2E 1104/1104 全绿**。教训：同类修复必须脚本枚举全量（v2-v11 修了 v12 漏）；i18n 建议加 locale 递归 diff CI（脚本可复用为 lint）。详见 doc/rmcp_review_r21_20261002.md。

- **R40 复核轮（2026-10-04）**：修复 6 项——①[H] sweep_stale_dirs 长度 64→32 hex（R28 自身引入 bug：repo_hash 是 md5=32 hex，写 64 致 stale 目录永不清理——修复自身引入 bug 的直接证据，fix self-review 必要性）；②[H] 凭证文件 OpenOptions mode 0600 直接创建（消除 write→chmod 明文暴露窗口）+ chmod 错误传播；③[M] abort token 注册移到 PICK_LOCK 之前（cancel-while-queued 可见）；④⑤[M] quoteArg 补反斜杠转义 + splitArgs 引号内转义/引号外字面量（node roundtrip 验证含 a\"b / C:\path）；⑥[L] idleTimeoutMs 0/NaN clamp 300000 + BearerKeyRow 补 catch。**验证**：cargo check 0 错 0 警；cargo test --lib 102 passed；tsc 0 错；npm build ✓；v40 套件 8/8（新增 rmcp_supplement_v40.py）；44 套件 **1280/1280 全绿**（全量首轮 8 套件失败均为公网 IP 真实调用，单独复跑零改动全绿——定性外部 ip.3322.net 瞬时抖动+串行压测限速，非回归）。详见 doc/rmcp_review_r40_20261004.md。
- **复核第三十九轮（2026-10-04，R39：session_rebuild 重连缺重检 + 空密码改密 + 最后管理员无守卫）**：侵蚀审计全在；2 代理（http_server.rs 头部 2250 行——中间件/bearer/leniency/loopback，R34 只扫过尾部；user_service/auth/mcp_manager/bearer_key/db 重读）。**修复 3 项（M×2 L×1）**：①[M] session_rebuild reconnect spawn 无 post-connect 重检——toggle enable/disable 两路径都有（R31/R32 修）而 rebuild 是同族第三条路径：tick 读配置→用户禁用→spawn 重连已禁用服务器永久在线 → connect 后镜像 fail-closed 重检（行缺/Err/disabled → disconnect）；②[M] update_by_username 接受空密码——create/update_password 都拒空唯此直接 hash("") 账号变零凭据可登录（admin API/settings_import 可达）→ 同 trim 空守卫；③[L] delete_by_username 无最后管理员守卫——删光 admin 后唯一恢复是重启重播种已知默认凭据 admin/admin → admin 目标先数剩余 ≤1 拒绝。**重要负面结论**：http_server.rs 头部 2250 行（bearer 最外层→leniency→cleanup 层序全组合、leniency 六种门组合、strict 零 body 变异、&auth[7..] 切片双守卫不可达、loopback 守卫）**零发现——协议核心面正式扫清**；auth/bearer_key/db 零缺陷。**新套件 rmcp_supplement_v39.py 8 项**：4 legacy 回显 + 公网IP×2（2024-11-05/2026 stateless IPv4 断言）+ discover + health。**验证**：cargo check 0 错 0 警；cargo test --lib 102 passed；**43 套 E2E 1270/1270 全绿**（重建二进制热跑）。教训：①同族修复点穷举到「零」而非「修完报告的那些」——post-connect 重检家族三条路径修了俩漏了仨；②校验守卫按「函数对」审计（create/update_password 有而 update_by_username 无 = 三条写密码路径只封两条）。详见 doc/rmcp_review_r39_20261004.md。
- **复核第三十八轮（2026-10-04，R38：MRTR 畸形 inputRequests 死信箱 + RAG builtin 无超时 + port 0 旁路）**：侵蚀审计全在；2 代理（commands 8 小文件；rmcp_bridge 1681 行全量重读 + mcp/client）。**修复 3 项（全 Low）**：①MRTR 守卫只挡双 None——上游 inputRequests 存在但反序列化失败（.ok() 折叠成 None）+ 可解析 requestState 时绕过守卫 → 客户端收 request-less InputRequired 无法构造 inputResponses 交互死路 → 区分「键缺失」与「键存在但畸形」，后者走 errored-Complete 回退；②RAG builtin 是三条执行路径中唯一无 timeout_tool_call 包装的（pool/meta 都包）——本地模型挂死/大 reindex 持锁时 /mcp 调用无限阻塞（后台任务 stuck working 无 ttl 永久）→ 包上对齐；③start_http_server 命令路径 port 0 旁路 R27 校验（只存在于 config 启动路径）——绑临时端口但报告/追踪恒 0、loopback 劫持检查对 0 检测该会话静默失效 → require_admin 后补 port==0 拒绝。**清洁**：tools/prompts/resources/smart_routing/cost/users/auth 八命令文件、rmcp_bridge 全部历史疑区（标记法/owner fail-closed/NATIVE_PEERS ABA 安全/meta 自带超时）、client 纯透传——零缺陷。**新套件 rmcp_supplement_v38.py 10 项**：4 legacy 回显 + 公网IP×2（2025-06-18/2026 stateless IPv4 断言）+ RAG builtin 列表/调用（未启用 SKIP 对齐 v27 语义）+ discover + health。**验证**：cargo check 0 错 0 警；cargo test --lib 102 passed；**42 套 E2E 1262/1262 全绿**（重建二进制热跑）。教训：①.ok() 折叠「存在但畸形」为「缺失」改变了守卫语义（与 R36 迁移 skills 同族：缺失被当万能分支）；②超时覆盖按执行路径枚举（pool 隔离/共享/builtin+meta 共四条）而非记忆——B2 是 42 轮里最后一条裸奔路径，枚举清单才找得到。详见 doc/rmcp_review_r38_20261004.md。
- **复核第三十七轮（2026-10-04，R37：skill import 绕过文件锁（High）+ LIKE 反斜杠漏网第三处）**：侵蚀审计全在；2 代理（prompt/resource/skill_service 全量重读；tauriClient 1490 行重读 + fetchInterceptor + types 契约对账）。**修复 3 项（H×1 M×1 L×1）**：①[H] import_skills 未获取 skill_fs_lock——export/uninstall/delete 全持锁唯 import 漏：同 dir_name 并发导入双双过 exists 后双 copy_dir_recursive 同 dst（copy_dir_inner 先删再拷，第二趟毁第一趟半成品树双双标 ok + FTS ref_id 偷行），import-vs-export 导出截断树 → 顶部持锁；②[M] skill LIKE 漏网反斜杠转义（prompt/resource 已修的第 3 处同类）——`foo\bar` 把 \b 当转义字面量错配、尾随 \ 吃掉 % 通配符 → 补反斜杠优先转义（顺序敏感）；③[L] tauriClient optionCommands 不全——get_rag_doc(_paged)/get_builtin_prompt/get_builtin_resource 同为 Result<Option> 但 null 伪装 {success:true} 无 data（陈旧列表开详情渲染空对话框）→ 列表补 4 项。**记录不修**：oauth 前端收集 Rust 无落点（R10 产品决策待办维持，不加 Option<Value> 半成品透传）；prompt/resource 重名 check-then-insert TOCTOU（v27 UNIQUE 迁移遇存量重名会启动崩，风险大于收益记已知债）。**清洁**：render_template 单遍扫描器、valid_dir_name 全变更点、copy_dir_inner visited、agents 锁、fetchInterceptor WeakMap、tauriClient 路由/参数签名对账全零缺陷。**新套件 rmcp_supplement_v37.py 8 项**：4 legacy 回显 + 公网IP×2（2025-03-26/2026 stateless IPv4 断言）+ discover + health。**验证**：cargo check 0 错 0 警；cargo test --lib 102 passed；tsc 0 错；build 通过；**41 套 E2E 1260/1260 全绿**（重建二进制热跑）。教训：①锁契约按操作树而非命令审计（与 R32 rag_search 同模式：一致性按数据面对账）；②LIKE 转义三段链漏一段就错配，新实现逐字符对照仓内现范本；③前端注释承诺的 round-trip 与 Rust serde 实际行为需显式对账（check_frontend_rust_contract.py 价值再确认）。详见 doc/rmcp_review_r37_20261004.md。
- **复核第三十六轮（2026-10-03，R36：on_demand 并发调用互踩（计数器化）+ 迁移 panic 循环 + 日志降级丢过滤器）**：侵蚀审计全在；2 代理（session_pool/on_demand/mcp_tasks/subscription_hub 全量重读；db/migration.rs 1700 行 + log/config/server_tool_config 重读）。**修复 7 项（M×3 L×4）**：①[M] on_demand in_flight 是 bool 不是计数器——并发 A/B 调用 A 先完成清 flag，B 仍在跑时 idle 定时器看 false 移除条目 disconnect 打断活调用（600s 调用 vs 300s idle，正是该 flag 要防的场景；docstring 只推理了单调用）→ 改 u32 计数器 saturating_sub + 定时器条件 ==0；②[L] flag 置位与守卫构建之间隔两处 await（schedule_idle）——取消窗口泄漏计数器（手动清与 Drop 都不跑）→ 守卫构建后再 schedule_idle；③[M] 迁移 v13/v14/v25 skills 非对象（string/array，config 深合并可达）→ serde IndexMut panic 且发生在 set_version 前 → 版本不推进每次启动重跑 panic（永久崩溃循环无备份恢复）→ 三处改类型检查非对象覆写 {};④[M] log_service LIKE 降级路径静默丢弃 level/server_name 过滤器（FTS 路径应用而三条降级只按 message）→ like_search_logs 接收过滤器 QueryBuilder 条件拼接；⑤[L] server_tool_config update_description 读-插竞态撞 UNIQUE → 无条件 ON CONFLICT DO UPDATE；⑥[L] v22 索引 IF NOT EXISTS 遇 v21 同名不同列静默 no-op（created_at 排序键从未落地）→ DROP 后重建；⑦[L] v11 DROP COLUMN .ok() 吞真实错误（旧 SQLite 无支持/IO 错误 → 版本推进 NOT NULL 仍在后续 INSERT 全败）→ pragma 先查存在传播错误。v8 非幂等维持 R14 记录不修。**清洁**：session_pool/mcp_tasks（owner 门控 6 方法一致/u64 溢出/sweeper 幂等）/subscription_hub/config_service/migration 其余 20 版全零缺陷。**新套件 rmcp_supplement_v36.py 9 项**：4 legacy 回显 + 公网IP×2（2025-11-25/2026 stateless IPv4 断言）+ /api stats + discover + health。**验证**：cargo check 0 错 0 警；cargo test --lib 102 passed；**40 套 E2E 1252/1252 全绿**（重建二进制热跑）。教训：①互斥标志 vs 计数器由并发度决定——bool 隐含「最多一个并发者」假设而假设没写进注释；②panic 在 set_version 之前的迁移 = 崩溃循环，serde IndexMut 这种「不会失败」API 也要问重跑会发生什么；③守卫覆盖范围从构建那一刻开始，构建前的 await 是无保护窗口（R35 LoginPage 同族）。详见 doc/rmcp_review_r36_20261003.md。
- **复核第三十五轮（2026-10-03，R35：$smart 关键词重复词评分失真 + CONNECT_LOCKS 无界泄漏）**：侵蚀审计全在；2 代理（pool.rs + smart_routing search/store/index 全量重读；LogViewer/UpdateCheckContext/ServerInstallProgressContext/Prompts/Resources/Markdown/LoginPage）。**修复 4 项（M×2 L×2）**：①[M] $smart 关键词评分对重复查询词失真——matched 按词计不按去重词计，query 'deploy deploy' 命中单词得 1.0 与全匹配同分绕过 score_threshold → BTreeSet 去重后按 distinct 计分（docstring 契约是去重语义）；②[M] ServerInstallProgress 终态清除定时器删新条目——done 后 1.5s 内新 downloading（重连/重装）被 pending 定时器删掉进度条中途消失 → 非终态事件先 clearTimeout；③[L] CONNECT_LOCKS per-name 锁条目从不删除（delete/rename 均不清）→ 新增 pool::forget_connect_lock 挂在 delete + rename 旧名分支（**刻意不挂 disconnect**——并发 connect 持 Arc 期间移除条目会让新 connect 铸新锁并行运行破坏串行化）；④[L] LoginPage loadAuthProviders 无 catch——getPublicConfig 失败（后端不可达场景）unhandled rejection → catch 兜底重置 socialProviders。**清洁**：pool 连接守卫/同锁无复活竞态/无锁跨 await、smart store/index（esc_like+ESCAPE、EMBED_WRITE_LOCK 无重入、ghost 含 builtin）、LogViewer/UpdateCheck/Markdown/Prompts/Resources 全零缺陷。**新套件 rmcp_supplement_v35.py 9 项**：4 legacy 回显 + $smart REST 重复词查询端到端（dedup 协议面验证，GET 参数名 query 非 q 对齐既有套件）+ 公网IP×2（2025-06-18/2026 stateless）+ discover + health。**验证**：cargo check 0 错 0 警；cargo test --lib 102 passed；tsc 0 错；build 通过；**39 套 E2E 1243/1243 全绿**（重建二进制热跑）。教训：①计分公式与 docstring 契约逐条对照——对无重复输入正确的公式对重复输入静默失真；②资源清理点必须选在无并发持有者的位置（挂 delete/rename 而非 disconnect），修复本身能引入更糟的竞态。详见 doc/rmcp_review_r35_20261003.md。
- **复核第三十四轮（2026-10-03，R34：OpenAPI 三项（流式帽/死凭证/参数泄密）+ SSE 两项）**：侵蚀审计全在；2 代理（sse+openapi transport 全量重读；http_server REST 尾部 2200 行重读——本轮零发现）。**修复 5 项（M×3 L×2）**：①[M] OpenAPI spec 32MB 帽在整包缓冲后才生效——chunked 恶意服务器可在 30s 窗口灌 GB 内存（bytes() 先读完再查长度=没有上限）→ 改 chunk() 流式累积超帽即止；②[M] query/cookie 位置 apiKey 凭证静默失效——warn 日志指向「spec security schemes 回退」而 vendored rmcp-openapi 0.32 从不注入 spec security（唯一凭证源 default_headers，源码核实）→ 每次调用裸奔 401 → connect 响亮报错，build_default_headers 改 Result 传播；③[M] 工具调用参数全文写入持久化日志 DB——参数常含 API key/token，本文件 header 已为此脱敏而 args 明文 → 改记参数键名+字节数摘要；④[L] SSE inline 解析器残留 trim()（背景 reader 已修、同文件 inline 副本未同步——跨 data 行 JSON 显著空格被吃）→ 镜像只剥 \r；⑤[L] SSE 总超时越界——deadline 检查在 60s chunk-wait 后，涓流实际 ~120s → timeout_at 以剩余预算约束。**清洁**：http_server 16 handler 门控逐个确认（含 GET 冷启动前置门控）、start 持锁生命周期、ttl sweeper 幂等全对；SSE reader-generation/pending 清理/UTF-8 跨块完好。**新套件 rmcp_supplement_v34.py 9 项**：4 legacy 回显 + 公网IP×3（2024-11-05/2025-03-26/2026 stateless IPv4 断言）+ discover + health。**验证**：cargo check 0 错 0 警；cargo test --lib 102 passed；**38 套 E2E 1234/1234 全绿**（重建二进制热跑）。教训：①上限检查必须在分配之前——凡资源上限问一句「检查发生在分配前还是后」；②注释声称的回退机制必须到依赖源码验证存在性（O2 的回退从未实现，日志骗了用户）；③同文件两份近似代码行为不一致 = 复制体未同步修复的信号。详见 doc/rmcp_review_r34_20261003.md。
- **复核第三十三轮（2026-10-03，R33：Sidebar 条件 Hook（High）+ stdio 更新检查未处理 rejection）**：侵蚀审计全在；2 代理（bearer_keys/groups/config/logs/market/registry/cloud + auth + db/mod；ServersPage/Header/Sidebar/UserProfileMenu/StatusDot）。**修复 2 项（H×1 M×1）**：①[H] Sidebar 条件 Hook——`auth.user?.isAdmin && usePermissionCheck('x')` 短路包裹 Hook，auth.user 首渲染 null 翻转为值时 Hook 调用顺序改变 → React 卸载整棵组件树（admin 用户首次加载必触发）→ 无条件调用 Hook 后布尔组合；②[M] stdio 更新检查 try/finally 无 catch——apiPost 失败 throw（fetchInterceptor），rejection 悬空 + 用户零反馈（错误 toast 只覆盖 success:false 路径）→ 补 catch 同款 toast。**证伪 2 项（伪影家族第 4/5 次误报）**：cloud.rs format!("******", api_key)（代理报 High：Authorization 发字面量星号）与 bearer_keys.rs not-found 消息——od -c 字节级核验均为终端脱敏伪影，实际源码 `format!("Bearer {}", api_key)` 完好（R14 openapi/R22 git/R25 snippet 后第 4 次），直接修复会真损坏 Authorization 头。**清洁**：七命令文件零缺陷（registry body.len() 后置检查兜底 content_length 谎报）、auth 随机 JWT secret/无 alg 混淆、db/mod 含空格路径经 sqlx FromStr 验证、Header/UserProfileMenu/StatusDot/ServersPage 既有修复全完好。**新套件 rmcp_supplement_v33.py 9 项**：4 legacy 回显 + 公网IP×3（2025-06-18/2025-11-25/2026 stateless IPv4 断言）+ discover + health。**验证**：本轮 Rust 零改动（修复均前端）无需重建；tsc 0 错；build 通过；**37 套 E2E 1225/1225 全绿**。教训：①条件 Hook 在开发环境（条件恒真）全绿、真实用户流（auth 异步解析）必翻车——Hook 调用点必须无条件已入代理扫描家族；②try/finally 不是错误处理，await 可 throw 必有 catch；③伪影纪律固化：代理报源码含 ****** → 一律 od -c 定谳 → 完好记证伪不修。详见 doc/rmcp_review_r33_20261003.md。
- **复核第三十二轮（2026-10-03，R32：rag_search 未门控（High）+ 版本缓存钉死离线兜底列表）**：侵蚀审计全在；2 代理（commands/runtime.rs 1450 行全量重读 + commands/rag.rs；前端 serverFormPayload/serverDuplicate/clipboard/GroupCard/AccessUrlDialog/Dashboard）。**修复 5 项（H×1 M×2 L×2）**：①[H] rag_search_command 无 require_admin——返回 chunk 文本（读取外泄通道），多用户下任意非 admin 可读全部已索引文档，击穿 upload/get_doc/get_chunks 全部门控（其注释明言门控理由恰是「可经 rag_search 读回」）→ 补门控；list_rag_docs（泄露文件名/original_path 清单）同病灶一并门控；②[M] runtime 版本列表缓存无条件缓存 fetch 结果——首次启动离线时 fallback 内置列表被 OnceLock 钉死到进程结束（联网后也永远看不到新版本）→ fetch_* 改 Option 返回，仅真实拉取入缓存，None 留空重试；③[L] cancel_rag_git_pick 无门控可中断 admin 在飞 clone（temp 清理与 upload 映射竞态）→ 补 require_admin；④[M] GroupCard 复制的 /mcp/{group.name}、/api/{group.name} 端点未编码——含空格/CJK 组名复制出坏 URL（clientConfigEndpoint 专门 encode 了，注释却称 legacy Copy URL 有意原样）→ 三处全部 encodeURIComponent；⑤[L] AccessUrlDialog execCommand('copy') 返回值丢弃——copy 被拒仍显示已复制 → 捕获布尔 false 不置 copiedKey。**清洁**：runtime.rs 其余全对（char_indices/版本白名单/300MB+SHASUMS/tar 逃逸含悬空目标/全部失败路径清理/CREATE_NO_WINDOW 12 处）、serverFormPayload/serverDuplicate/clipboard/Dashboard 零缺陷。**新套件 rmcp_supplement_v32.py 10 项**：4 legacy 回显 + 公网IP×3（2025-03-26/2025-06-18/2026 stateless IPv4 断言）+ CJK 服务器名 percent-encoded /api/{name}/openapi.json 端到端解析（编码路径与 bridge decode 闭环）+ discover + health。**验证**：cargo check 0 错 0 警；cargo test --lib 102 passed；tsc 0 错；build 通过；**36 套 E2E 1216/1216 全绿**（重建二进制热跑）。教训：①门控一致性按数据面（谁能读到什么）而非命令面审计——邻近命令全有门控且有正确注释，真正吐内容的命令漏了；②错误路径产物（fallback 值）永不入缓存——一次瞬时故障被 OnceLock 放大成进程生命周期问题。详见 doc/rmcp_review_r32_20261003.md。
- **复核第三十一轮（2026-10-03，R31：删除中连接 fail-open + RagPage 排除/选择三连缺陷）**：侵蚀审计全在；2 代理（mcp_manager/config_service/progress/app_logger/server_tool_config/time；RagPage 全 5800 行）。**修复 6 项（H×3 M×1 L×2）**：①[M] enable-path 重检 fail-open——toggle_server 连接完成后重检 DB，行已删（连接期间删除）时 unwrap_or(true)=仍启用 → 活进程无 DB 行无人回收直到重启（disable 分支同场景 unwrap_or(false) 正确——同函数两分支语义相反即缺陷信号）→ 两 arm 改 false fail-closed；②[H] persistExclusionDiff 永远无法持久化取消排除——removed 对 excludedBase 差集而 union⊇base 恒空 → 新增 excludedOriginalRef 打开弹框时的原始快照作 diff 参照；③[H] 排除的文件仍被上传——selectedPickedFiles 只按 sel.has 过滤不移出排除项 + memo deps 缺 excludedUnion → filter 补排除过滤 + toggleExcluded 同步移出选择集；④[H] toggleGroupExcluded 循环内逐个调用读渲染闭包陈旧 Set——第 2 次迭代覆盖第 1 次删除（取消排除只生效最后一个）→ 单快照批量计算一次提交；⑤[L] OCR 预检提示重复渲染两次（copy-paste 残留）→ 删一块；⑥[L] git-clone/preview 进度监听器 unmount 早于 listen resolve 时泄漏 → disposed flag。**清洁**：config_service/progress/app_logger/server_tool_config/time 全零缺陷；RagPage 竞态守卫/Set 更新/applySelected 验证无恙。**新套件 rmcp_supplement_v31.py 9 项**：4 legacy 回显 + 公网IP×3（2025-06-18/2025-11-25/2026 stateless IPv4 断言）+ discover + health。**验证**：cargo check 0 错 0 警；cargo test --lib 102 passed；tsc 0 错；build 通过；**35 套 E2E 1206/1206 全绿**（重建二进制热跑）。教训：①同函数内两分支对同一缺失语义 unwrap_or 相反 = fail-open 最强信号（R27 vectordb/R20 allow-list 同模式）；②union⊇base 时对 base 差集恒空——一行类型推理避免静默缺陷；③循环内调用读渲染闭包 state 的 setter = 每次迭代同一陈旧快照，批量变更须单快照计算。详见 doc/rmcp_review_r31_20261003.md。
- **复核第三十轮（2026-10-03，R30：FTS 重建快照竞态 + 安装监听器竞态 + 资源名称守卫）**：侵蚀审计全在；2 代理（fts/market/bearer/group/resource 服务；AuthContext/SettingsContext/AboutDialog/version.ts/changelogService/mcpClientSnippets）。**修复 7 项（M×2 L×5）**：①[M] AboutDialog 更新检查失败卡 checking spinner——catch 用陈旧闭包 updateInfo 门控重置，二次失败后 source='checking' 永真 → 无条件设 error 状态；②[M] version.ts 安装结果监听器注册竞态——listen 异步注册未 await 就 invoke，Rust 即时失败事件先于注册落地（Tauri 不缓冲）→ promise 永不 settle 对话框永久 downloading + 监听器泄漏 → 先 await listen 再 invoke；③[L] fts_service rebuild_one/backfill 的「SELECT 放事务内」修复不完整——WAL deferred 读事务持快照，并发写者仍可在快照与 in-tx DELETE 间提交（清空删新行 FTS、插入来自陈旧快照 → 行永久不可搜）→ 两处 pool.begin_with("BEGIN IMMEDIATE")；④[L] resource name 传 None 清空 → FTS ref_id "" 死行（不可解析 + 两 NULL 名互偷行 + rebuild 跳过 NULL 造成永久漂移）→ create/update 拒绝空名；⑤[L] market_service 解析失败 unwrap_or_default 静默空 Market 页 → log::error 后降级；⑥[L] group create 返回伪造 chrono created_at 与 DB DEFAULT 格式不一致 → commit 后重读（对齐 update）；⑦[L] exportMCPSettings serverName 未 encodeURIComponent（含 &/# 名字 Copy Config 恒失败）→ 补编码。**清洁**：bearer_key_service（常数时间比较全对）、fts 分词/LIKE 转义无注入、AuthContext、changelogService、mcpClientSnippets（JSON stringify/TOML 引号全合法）、SettingsContext（httpPort 写读键一致）。**新套件 rmcp_supplement_v30.py 7 项**：/api openapi servers 存活 + 公网IP×3（2024-11-05/2025-03-26/2026 stateless IPv4 断言）+ 会话 mint + health（group/resource CRUD 系 Tauri-command-only 无 REST 路由，router 已核）。**验证**：cargo check 0 错 0 警；cargo test --lib 102 passed；tsc 0 错；build ✓；**34 套 E2E 1197/1197 全绿**（重建二进制热跑）。教训：①「读放进事务里」≠竞态修复——WAL deferred 是快照语义，读-清-写三步防并发写者必须 BEGIN IMMEDIATE 先取写锁；②事件驱动 promise 必须先注册监听再触发事件源，async 注册与发射间的窗口即永久悬挂。详见 doc/rmcp_review_r30_20261003.md。
- **复核第二十九轮（2026-10-03，R29：tray 菜单监听器累积 + ServerContext 跨代际竞态 + 围栏注入）**：侵蚀审计 20+ 修复簇全在；2 代理（rag/extract 4 文件 + lib.rs + tray.rs；ServerContext 轮询路径 + RagDocTree + FileTypeRenderer + fileIcon）。**修复 5 项（M×2 L×3）**：①[M] tray.rs rebuild_menus 每次调用都 app.on_menu_event 追加监听器（Tauri 2 该 API 是追加非替换）——切 3 次语言后每次菜单点击触发 N 次重复事件（update 弹框叠层/quit 并发）→ AtomicBool 门控只注册一次（注释「safe to register repeatedly」前提系想当然，未核 SDK 源码）；②[L] resolve_lang 的 localStorage 探针死代码——setup 时窗口导航未提交，wry eval 走 pending-scripts 分支丢回调 → recv 恒 Err 恒 en → 删除探针（set_menu_language 启动调用已覆盖）；③[M] ServerContext 陈旧 effect 代际闭包——deps 含 currentPage/serversPerPage/isInitialLoading，改页码重建 effect 后旧 run 在飞 fetchInitialData 的 phase 守卫（单 invocation 作用域）拦不住：用旧页参 setState + 旧闭包 startNormalPolling 杀新 run 定时器并钉死旧页码 → pollRunRef 单调代际令牌 + 异步续体 isStaleRun() bail（R26 phase token 同类 bug 第二形态）；④[L] 启动成功路径双重首轮请求（setIsInitialLoading 触发 effect 重跑的 else 分支与成功路径 startNormalPolling 背靠背两对请求）→ 成功路径只翻 isInitialLoading；⑤[L] FileTypeRenderer 代码围栏注入——内容含 ``` 行时外层围栏提前终止按任意 markdown 渲染 → 扫最长反引号 run 围栏取 max(3,run+1)。**清洁**：rag/extract×4、lib.rs、RagDocTree、fileIcon 全零缺陷。**新套件 rmcp_supplement_v29.py 10 项**：4 legacy 回显 + 公网IP×4（3 legacy + 2026 stateless，IPv4 Body 断言，2026 走 SEP-2243 base64 头）+ discover + health。**验证**：cargo check 0 错 0 警；cargo test --lib 102 passed；tsc 0 错；build 通过；**33 套 E2E 1190/1190 全绿**（重建二进制热跑）。教训：①「safe to register repeatedly」类注释必须对照 SDK 源码验证追加/替换语义；②effect 内 phase/flag 守卫只保护单次 invocation，deps 重建后在飞异步续体须 ref 代际令牌跨 run 拦截。详见 doc/rmcp_review_r29_20261003.md。
- **复核第二十八轮（2026-10-03，R28：rag git 规范化哈希 + updater 取消竞态 + G3 部分持久化）**：侵蚀审计 20/20；2 代理（rag/service+git delta、updater/logger/tool_config）。**修复 4 项（全 Medium）**：①delete-time git 引用计数 `count_git_docs_for_repo` 按 RAW url 比较而工件以 canonical hash 为键——同 repo 两种拼写（http→https）时删 A 拼写最后一份文档会删掉 B 拼写在用的 clone+凭证 → 调用方先 canonicalize、计数两遍法按 canonical hash；②TTL 强制刷新/preview/clear_git_source_error_for 三处按 RAW hash 计算键而缓存以 canonical hash 为键 → 显式刷新静默 no-op、错误横幅清不掉 → 三处全走 `repo_hash(&canonicalize_url(url).await)`；③ensure_persisted 非原子 copy 直接落最终 `{hash}` 目录——崩溃残留缺 .git 的部分 clone 被 dst.exists() 永久短路（source-sync 判 original lost 批量删文档）→ 拷 `{hash}.new` staging 后 rename + sweep_stale_dirs 补扫缺 .git 的 64hex 目录自愈；④updater cancel 在 slot lock 前读 CURRENT_INSTALL_ID——与并发 install 交错给错误尝试打 cancelled、新尝试永无 terminal 事件（对话框悬挂）；且 install_update_cancelable abort 前序不发任何事件 → cancel 改锁后读 id + abort 前序时捕获旧 id 补发 cancelled。**证伪 12 项**（TempPath/线程 panic/tool_config race/attempt 碰撞/credential 损坏等）。**新套件 rmcp_supplement_v28.py 13 项**：5 版本回显逐版本 + 2026 discover（ttlMs/public）+ 真实公网IP 调用（Response Body IPv4 断言）+ 宽松（缺 Accept/缺 jsonrpc 归一放行）+ /health。**验证**：cargo check 0 错 0 警；cargo test --lib 102 passed；**32 套 E2E 1180/1180 全绿**（重建二进制热跑，公网 IP 58.211.44.178 真实返回）。教训：同一实体的两种键形态（raw vs canonical hash）共存就是一致性缺口信号——修复时先全仓 grep 该实体的全部键计算点。详见 doc/rmcp_review_r28_20261003.md。
- **复核第二十轮（2026-10-02，R491+：5 代理扫描剩余面 + 亲核修复 + 新套件 v20）**：本轮覆盖第 18/19 轮未扫的剩余代码面（chunker/extract、git.rs、services batch 2、commands batch 2、传输层轮询路径）。**High×3**：①chunker push_atoms 深度递归（40k 嵌套 → 8 万层栈溢出 SIGABRT 进程崩溃）→ 迭代显式工作栈 MAX_DEPTH=512 + 深嵌套测试；②skills.rs 15 命令中 9 个写/删/导入无 require_admin → 全部补门控；③git 首次导入必败（temp_dir_for 只查 legacy `{hash}`，clone 落 per-attempt `{hash}-{32hex}` 目录）→ 解析最新存在目录。**Medium×10**：git 清理/刷新锁键原始 vs 规范化哈希不一致 ×2 → 统一 canonicalize；prompt/resource 重名偷 FTS 行 + get_prompt 按名歧义 → 事务内重名拒绝（create/update 排除自身）；resource URI 改名旧 URI 订阅者悬挂 → 补发 notify_resource_updated(old)；user_service 空密码（settings_import 可触达）→ trim 拒绝；rag 5 个内容读命令无门控（泄露通道）→ require_admin；openapi load_openapi_spec CPU 密集阻塞 async worker → spawn_blocking；OCR image 解码无 Limits（解压炸弹 OOM）→ decode_limited（64MiB/12000px）；registry/cloud resp.json() 无界 → 32MB 双重上限。**Low×5**：stdio disconnect service.cancel() 无界 → 5s timeout；轮询 clamp(200,u64::MAX) 上游恶意 10min interval 越过 600s deadline → clamp(200,30_000) + deadline 检查移到 sleep 后（stdio+http 双 transport）。**新套件 rmcp_supplement_v20.py 36 项**：group allow-list 强制、$smart 端点（仅 meta 工具+search_tools 实调）、GET SSE 流×5 版本、并发 4 会话 UTF-8 scope 公网IP、9MiB body 边界、CacheableResult 逐版本门控、prompts/resources round-trip（重名拒绝不破坏正常路径）。套件调试沉淀：groups.servers 是对象数组（含 tools 过滤）；scope 通道裸工具名 vs root 前缀聚合；中文 body 必须 utf-8 显式编码。**验证**：cargo check 0 错 0 警；cargo test --lib 93 passed（+chunker 深嵌套）；tsc 0 错；**24 套 E2E 1078/1078 全绿**（重建二进制全量重跑）。详见 doc/rmcp_review_r20_20261002.md。

- **复核第二十轮（2026-10-04，R558-R607：第九轮修复自身复审 7 专项代理 + B10 遗留落地 2 代理 + 低覆盖文件深读 + 亲核修复 + 2 新套件 + 全量回归）**：C1-C7 复审第九轮修复、C8/C9 把宽松成对与版本矩阵空格**实现为新套件**、C10 首读最薄文件。**Medium×5**：①openapi GET 执行端点受限 key 可冷启动 allow-list 外服务器（list_tools_for 在鉴权后、allow-list 前）→ 403 预检；②disabled-tool gate 三处 `unwrap_or_default` fail-open → Err 回 500；③registry/cloud 代理 32MB 上限对 chunked 响应失效（bytes() 先全量入内存）→ 流式 cap；④interceptors.ts 401 无 isTauri 守卫（桌面强跳 /login）→ 包裹；⑤progress.rs `-p=foo` 死代码（split_once 分支在字面量相等时不可达）+ uvx `-p` 短形未消费 → strip_prefix + 消费。**Low×20**：legacy 订阅死 Weak prune + 去重；user_service 事务化 ×3（BEGIN IMMEDIATE 闭合 last-admin TOCTOU、消除部分写入、0 行报错、空 username 拒绝）；rag zero_all/reindex 失败改 skip（不复活孤儿 meta）+ canonicalize 复用 + 假注释修正；models dedup_out_names 收敛单点；group call 404 语义对齐 + operationId `_N` 递增 + 死代码删除；tray 四键 en 回退；auth 注释；updater 假终态守卫；migrations 0026/0027 注释；runtime 5 处 warn + 注释修正。**前端×4**：ViewDialog 失败回滚+toast、selectedCount i18n ×4、srcKey 死变量、AccessUrlDialog 复制失败 toast。**新套件**：rmcp_leniency_pairs_r10.py 15/15（6 形态宽严成对；钉死 text/plain 两侧一致 415 与畸形 JSON 415 两个语义边界）；rmcp_matrix_gaps_r10.py 23/23（/api 无版本语义实测确认；group×5 版本；2026×group 无状态 IP；tasks×中文 scope 全链路）。**记录不修**：builtin 同名服务器边角、cloud baseUrl 自伤面、parse_cap 重启窗口排队、多选文件取第一、MarketPage 会话内存。**验证**：cargo check 0 错 0 警；cargo test --lib 102 passed；tsc 0 错 + build ✓；**47 套 E2E 1337/1337 全绿**（新二进制重启实测）。趋势：High 1→0，修复质量闭环成立。详见测试报告 §47。
- **复核第十九轮（2026-10-04，R508-R557：第八轮修复自身复审 7 专项代理 + 前端第二轮 + 迁移终审 + E2E 二轮审计 + 亲核修复 + 全量回归）**：B1-B7 专项复审第八轮全部修复代码、B8 前端第二轮（RagPage 5839 全文 + 14 文件）、B9 迁移完整性+清理终审（零残留；3 条孤儿命令记录；SSE 硬编码既定）、B10 E2E 45 套逐用例审计。**High×1**：serveDied 自愈死代码——watch 任务先 `lb_serve.await` 无界等 loopback（Linux/Windows 分支 pending 永不完成；macOS wildcard 单独死亡同挂）→ 分类/清理永不可达 → loopback 专用 oneshot 收卷 + 有界 await + else 直接返回。**Medium×6**：①user_service reserved 检查把 `admin` 列保留名而 `ensure_default_admin` 恰走 `create("admin")`——全新安装永远无可登录账号（死锁）→ seed 改直连 INSERT；②LEGACY_RESOURCE_SUBS 指纹取 peer_info Arc 地址，同 session 重复 initialize 换 Arc → 退订失效继续收通知 → 改 `Weak<PeerInfo>`（ptr_eq 精确匹配 + 死 Weak 回退，消 ABA）；③delete_doc 持 META_LOCK 跨 canonicalize_url 网络探测（10s×N 阻塞全部 doc 写）→ 锁在 refcount 段前显式释放；④macOS restart 的 loopback bind 单次 EADDRINUSE 假失败 → 5s/50ms 有界重试；⑤`Notify::notify_waiters` 无许可存储（stop 竞态首 poll 丢唤醒、3s 强断护栏失效）→ 每监听独立 Notify + notify_one；⑥group 改名/删除不级联 `bearer_keys.allowed_groups` → 事务内补 cascade。**Low×14**：bridge bearer Err 4 处静默降级匿名→传播；openapi spec 先 collect 后过滤（可唤醒无权 on-demand）→ allowed 预过滤；leniency 64MB 上限与 router limit 不一致→max 对齐；reindex_all/zero_all 锁外读整份过期 meta 回写（并发改名/标签被回滚）→锁内重读仅合并 chunk_count（DocMeta 补 Clone）；search merge tie-break 追加 chunk_index；ensure_persisted staging per-attempt uuid（并发互删）；mv/models 下载/发布 basename 碰撞内容错位→全 idx 去重；meta resolve_tool 仅 builtin 分支跳过 meta 名（真实上游同名工具可达）；gguf_nomic MoE top_k 越界 panic→加载期校验+min 防御；progress.rs `-p` 方向反了（第 8 轮跳值后落回位置参数）→采用 -p 消费值；tasks/get 轮询 3 次容错（http+stdio）；server_tool_config previous 读失败降级；config patch 非 object fail-closed；门控补齐（list_rag_excluded_paths/rag_tag_search×2/detect_port_occupier）+ 冗余 #[tauri::command] 删除。**前端**：runUpdateCheck reqId 守卫、BatchTagsDialog catch+toast、单删 catch+toast、`common.operationFailed` ×4；9 项 Low 记录不修。**套件治理**：leniency_v2 还原包 try/finally；B10 其余建议（15 弱断言、6 不成对、/api+/rest/group 版本矩阵空格、native list_changed 零 E2E）列排期。**验证**：cargo check 0 错 0 警；cargo test --lib 102 passed；tsc 0 错 + build ✓；**45 套 E2E 1299/1299 全绿**（新二进制重启实测）。详见测试报告 §46。
- **复核第十八轮（2026-10-04，R458-R507：10 代理全仓逐行真读 + 亲核修复 + 新套件 r458 + 全量回归）**：10 段并行逐行（http_server 3261 / bridge+hub+tasks / mcp/ 5181 / commands 5692 / services 7577 / rag 10355 / smart+mv 6483 / models+db+auth+lib+tray / 前端协议层 / E2E 矩阵审计）。**High×4**：①restart 有界优雅停机（存活 SSE 流使 graceful shutdown 永不完成 → restart 必失败服务器整体下线 → Notify latch + 3s 宽限强制断流 + 探测 listener 复用消 TOCTOU）；②serve 假活自愈（任务死亡后 handle 永不清除无法重启 → HTTP_START_GEN 代际守卫清除 + serveDied 状态）；③smart meta 工具自递归（smart_route_call 目标解析回 meta 工具无界递归 → resolve_tool 排除 + 最长前缀优先）；④session_pool enabled 复查失败分支孤儿 client（补 disconnect）。**Medium×9**：http cancel 5s 有界；legacy resource 订阅按 peer_info Arc 指纹精确匹配（防跨会话误删）；task-directed 补 source_ip；prompts/resources 六写命令 + smart_routing_reindex + rag 五命令补 require_admin；user demote last-admin 守卫；migrate_v27 builtin name UNIQUE（uri 有意不加——v23 产品决策）；mv release poison fail-closed + rag stop wait_while_initializing。**Low×12**：sse data 行语义、npx -p flag、413 直答、limit-only 数值转换、tasks 512 上限、tool_config 条件通知、resource delete URI 通知、skill FTS warn、runtime spawn_blocking ×6 + block_on 移除（cached_enhanced_path_or_env）、mv 按索引发布 + 扩展名大小写。**其他**：open_external_url 非 Windows 放行 &/%（单 argv 无重解析）；v24 leak 清理；SettingsContext 初始值对齐 Rust；滑条 dirtyRef；locales +1 键。**套件治理**：恒 PASS 清零（v38/v25/v27/r30x 收紧）+ 新套件 rmcp_round_r458.py 17 项（group×2026 真实 IP、$smart×2026、meta 递归回归、tasks/update 双代际、json_response 显式、bearer×discover、空 allow-list fail-closed、空 session 宽松）。**误报排除**：parse_body_limit 空格、G7 access_type 语义、G4 tasks 能力声明姿势（InputResponses 须 map + clientCapabilities.extensions 声明 tasks）。**记录不修**：list_changed 真实通知 E2E（无 REST 触发面）、gguf mask O(seq²)（查询侧边缘）、splitArgs 空串、CSP null/devtools（结构性债）。**验证**：cargo check 0 错 0 警；cargo test --lib 102 passed；tsc 0 错 + build ✓；**45 套 E2E 1299/1299 全绿**（重建二进制重启实测）。详见测试报告 §45。
- **复核第十七轮（2026-10-01 深夜，R408-R457：横切面切分 + 前端首审 + 亲核修复 + 全量回归）**：本轮以横切面切分（前端协议层/设置层首审、bridge 订阅与 list、server/group CRUD、锁序、config/log、pool 状态机、rag 命令契约、安全横扫、E2E 断言终审）。**High×2**：①rag docId 路径穿越（`delete_doc`/`get_doc_inner` 等 9 个 id 拼路径入口无校验，公网 MCP 网关 `rag_file_delete {docId:"../../.."}` 可删任意文件）→ `validate_doc_id()` 全入口接入；②`enableBearerAuth` 前端默认 true / Rust `unwrap_or(false)`（新装 UI 显示已开启但实际无鉴权）→ 前端对齐 false（skipAuth 同理对齐 true）。**Medium×9**：bearer allowed_servers 对 prompts/resources 不生效（builtin 模板/正文泄漏）→ `builtin_visible_for_bearer` 四处门控；广告了 `resources.subscribe` 但 legacy 订阅 -32601 → LEGACY_RESOURCE_SUBS 注册表 + subscribe/unsubscribe 覆写 + notify_resource_updated 原生扇出（Arc 指纹 prune）；on-demand 三 mutator 缺 entry 身份守卫 + 冷启动不取 connect 锁 → 守卫 + 锁内双重检查；pick_rag_git_repo 写共享凭据未门控 → require_admin；toggle_enabled 不发 tools/list_changed → 补通知；server 改名/删除不级联 groups 成员（按名存储悬空）→ cascade_groups_rename_tx/remove_tx 同事务；group LIKE 未转义 `\` → 转义链补齐；tauriClient cloud 工具调用假成功 toast → 显式失败 + null 兜底对 Option 命令返回 not-found；keepAlive/passthroughHeaders 被 serde 丢弃 → ServerConfig 三字段 + migrate_v26 三列全接线（round-trip 先行，运行时消费待接）。**E2E 断言升级**：6 处恒 PASS 占位改真断言/SKIP 桶、2 处 parse-fail 静默 PASS 修复、round50g 运算符优先级修复；round50d "密钥被打码"经 ord() 码点核验为显示层伪影。**记录不修**：smartRouting.envOverriddenFields 前端消费 Rust 不产出（死代码，待办）。**验证**：cargo check 0 错 0 警；cargo test --lib 92 passed；tsc 0 错误（基线 24 已清零）+ build 通过；**21 套 E2E 693/693 全绿**（重建二进制重启实测）。缺陷趋势：High 2→1→2（来源为首次覆盖的前端切面与安全横扫），收敛性分析见报告 §44.4。详见 doc/mcp_2026_protocol_e2e_test_report_20260927.md §44。

- **复核第十六轮（2026-10-01 晚，R358-R407：10 后台代理逐行 + 亲核修复 + 全量回归）**：**High×1**：refresh_rag_source/preview_rag_source_update 缺 require_admin（非 admin 可用存储 git 凭据拉私库并读回内容）→ 补门控。**Medium×13**：登录 dummy bcrypt 长度 52≠53 致枚举 oracle（unwrap_or(false) 统一）；rag_select_model/download_model 补门控；activity filters/options 两读补门控；openapi spec group scope 恒 403（按成员交集判定）；/api/tools GET 鉴权前冷启动 on-demand（提前 check_bearer_auth）；restart 100ms sleep → 探测 bind 轮询防 EADDRINUSE；loopback one-shot 补 stop 守卫；migration 备份删 rename 前的 remove_file（rename 原子替换）；smart reindex_all 绕 EMBED_WRITE_LOCK（锁下移进 save/remove 函数体，on_model_reloaded 重排防自死锁）；tar symlink/hardlink target 逃逸校验 + 下载预分配 clamp 200MB；runtime_env PATH 含空格整行丢弃（结构化判定）；vectordb keyword_search LIKE 通配符转义；modernbert layer1-11 attn_norm 错误吞 + dequantize unwrap panic → ? 传播。**新 E2E**：`rmcp_gap_matrix_r36x.py` 24 用例（G7 tasks×3 legacy、G9 $smart prompts/resources 实调、G10 legacy × prompts/resources × 双 scope）。**记录不修**：import 丢 groups（功能增强）、gguf mask 重建性能、Node SHASUMS、G12 客户端 SSE（mock 基建）。models/ 全目录本轮零发现（泄漏面/serde 往返全核对）。**验证**：cargo check 0 错 0 警；cargo test --lib 92 passed；**21 套 E2E 760/760 全绿**。详见 doc/mcp_2026_protocol_e2e_test_report_20260927.md §43。

- **复核第十五轮（2026-10-01/02，R308-R357：10 后台代理逐行 + 亲核修复 + 全量回归）**：**High×5**：①rag_file_create docType 路径穿越（`md/../../evil` 任意写）→ 限 `[A-Za-z0-9]`+拒 meta；②git 凭证经 reqwest/gix 错误链泄漏进日志（`user:pass@` URL 不脱敏）→ `scrub_credentials()` 接入全部错误/日志出口；③skill reconcile_pending `remove_link` 无 `valid_dir_name`（任意目录删除）→ 补守卫；④npx 重装清错缓存目录（env 指向 `npm-cache-{name}`、clear 删 `runtimes/_npx`，重装不重下）→ `npm_server_cache_dir` 同源修复；⑤server name 无校验（`../` 注入缓存目录 + remove_dir_all 任意删）→ `validate_server_name`。**Medium×14**：rag `.meta` 碰撞/copy 丢文件 tag-only 清向量/md5 副本哈希覆盖源基线/同名 create 共享内容文件/upload 改名孤儿化/git 引用计数 vs 在途导入竞态；fts rebuild_one 事务外读/is_ascii_token 丢非 ASCII；runtime install 并发互毁/set_active 缺版本校验；bridge legacy tasks/get 往返丢字段（键名映射修复）/终态任务 update_input 静默空 ack；pool disable vs in-flight connect 复活（`pool_connect_lock` 共享 + inner 拆分防自死锁）；on_demand tear_down 删 creation lock 致孤儿进程；gguf 空 chunk NaN + embed eos 截断；server_tool_config 改描述不通知；SSE connect() 无超时挂死 + 重连代际竞态（reader generation）+ id:/retry: 边界误 flush + trim() 破坏 SSE data 语义。**Low**：rename 静默失败日志、clone 取消竞写改 per-attempt 目录。**新 E2E**：`rmcp_gap_matrix_r35x.py` 13 用例（GET SSE × 4 版本含 2026 首覆盖、$smart 真实 IP、REST 版本头宽松、tasks/list+cancel、宽松 × 2025-03-26）。**验证**：cargo check 0 错 0 警；cargo test --lib 92 passed；**20 套 E2E 736/736 全绿**。详见 doc/mcp_2026_protocol_e2e_test_report_20260927.md §42。

- **复核第十四轮（2026-10-02，R258-R307：10 后台代理逐行 + 亲核修复 + 全量回归）**：**High×2 真修复**：①mcp_manager toggle disable-during-starting 竞态（断开被随后 connect 覆盖复活）→ post-connect re-check（轮询 500ms×600）；②smart store ensure_table dim=0 误判 drop 整表向量索引 → 开头 bail。**重要澄清**：openapi_transport.rs Authorization 头 `******` 经逐字节 ord() 码点核验为**本环境显示层打码伪影**（源文件实际 `format!("Bearer {}", token)` 正常），非真损坏（区别于 §3.17.7 R1-② 真事故）。**Medium×6**：bridge typed tasks/get 按 is_2026_session 分流（legacy 收 2025-11 字段名）；leniency blank session 剥离提升、bare 升格采用 header>=2026 声明版本、_meta normalize+capabilities 补注入；smart EMBED_WRITE_LOCK+ghost 对账；stdio 握手失败 kill_process_tree 防孤儿。**Low×10+**：REST 403/404 区分、smart tiebreak+esc_like、lib DB init 120s recv_timeout、migration 备份 tmp+rename 原子替换、settings_import serde alias 等（详见报告 §41.1）。**记录不修**：v8 迁移非幂等（历史已应用）、BearerKey 明文（存量债）、rag 3 Low。**新 E2E**：`rmcp_gap_matrix_r30x.py` 21 用例（4 版本往返×公网 IP 真调用、blank session、9999 不回显、GET SSE 流逐版本、tasks 端到端）。**验证**：cargo check 0 错 0 警；cargo test --lib 92 passed；**19 套 E2E 723/723 全绿**。详见 doc/mcp_2026_protocol_e2e_test_report_20260927.md §41。

- **复核第十三轮（2026-10-01，R208-R257：10 后台代理逐行 + 门控全量审计 + 全量修复）**：**High×2**：①http_server start() 双绑定 0.0.0.0+127.0.0.1 仅 macOS/BSD SO_REUSEADDR 语义成立——Linux（EADDRINUSE 需 SO_REUSEPORT）/Windows（刻意不设 REUSEADDR）HTTP 服务器永远起不来 → 双绑定 cfg(macos) 门控+Option join；②SSE 握手 buffer 残留字节移交后台 reader 时丢弃（chunk 含 endpoint+后续响应）→ buffer move 续解析。**Medium×6**：bridge execute_tool_call 禁用检查 fail-closed 化（Err 拒绝而非空列表放行）；poll_task InputRequired 补 resultType:"input_required" 覆写（GetTaskResult 扁平化自带 complete，下游 MRTR 升级门永不触发，http/stdio 双路径）；fts weighted seq tiebreak 纳入排序键（HashMap 迭代序随机→日志翻页重复丢条目）；server_service like_search 补反斜杠转义（第 7 处漏网）；smart meta.rs 两处 filter 对齐 `connected || start_on_demand`（睡眠 on-demand 工具已在索引却被 $smart 排除）；run_source_sync 扫描+import 合并同一 META_LOCK 临界区（TOCTOU 闭合，upload 不取该锁无死锁）。**Low×12**：GET $smart 数值 query 参数解析、SSE streamable 分支多行 data+60s 整体超时+init 失败停 reader、mcp_tasks working 任务 24h 兜底回收、on_demand in_flight 置位 ptr_eq 守卫+list_tools 60s 超时、fts backfill SELECT 移入事务、rag update delete-after-write 重排、smart embeddings 全局写锁、pool 兜底块注释改实。**门控全量审计**：skipAuth=false 模式下 ~60 条写命令无门控 → 本轮补齐 kill_port_occupier（任意 kill -9，High）+ http start/stop + logs 读/清 + servers/groups/server_tool_config 写 + call_tool + rag 写 12（upload 任意路径读取通道）+ runtime install/set_active 6，共 30+ 条（SessionState 注入 + require_admin，skipAuth 短路不影响默认流程）；迁移完整性审计通过（rmcp 3.4.1 单栈、158 命令注册无孤儿、tauriClient 137 映射无孤儿、SSE 手写 transport 补注释）。**验证**：cargo check 0 错 0 警；cargo test --lib 92 passed；18 套 E2E 635/635 全绿。详见 doc/mcp_2026_protocol_e2e_test_report_20260927.md §40。
- **复核第十二轮（2026-10-01，R158-R207：10 独立复核代理 + 全量修复 + 635 E2E）**：**High×2**：①`get_system_config`/`update_system_config` 无 `require_admin`（auth-enabled 模式任意本地 IPC 读改全局配置）→ 两命令加门控；②users.rs `list/add/update/delete_user` 四命令全部无门控 → 全部加 `require_admin`+SessionState。**Medium/Low×12**：sse_transport `data_acc` 跨 chunk 持久（High：chunk 边界丢事件）、list_tools 分页 HashSet 防环+100 页 cap、bridge update/cancel_task 补 `$smart` 门控、日志 FTS 回表分块 500/批（SQLITE_MAX_VARIABLE_NUMBER 999 上限）、servers.rs 4 处 `remove_dir_all` 包 spawn_blocking、login 用户不存在时 dummy bcrypt verify 防用户名枚举、stdio 握手失败清 pid 防陈旧误杀、blank session 头移除提升到 leniency 层顶部（非 bare 请求也生效）、openapi fetch_spec 用户 header `try_from` 校验防 panic、is_skip_auth_enabled DB Err 加 warn、parse_progress_pct `saturating_mul` 防溢出 panic 塞死 stderr 管道、log_service LIKE 转义补反斜杠（6 处）+ page/offset clamp。**验证**：cargo check 0 错 0 警；cargo test --lib 92 passed；18 套 E2E **635/635 全绿**。详见 doc/mcp_2026_protocol_e2e_test_report_20260927.md §39。
- **复核第十一轮（2026-09-30 深夜，R108-R157：10 独立复核代理 50 轮 + Low 清账 + 遗留项闭环）**：10 个并行 code-review 代理逐行真读（bridge/leniency/REST/pool·session·on_demand/五传输/订阅任务/commands/services/前端 IPC/迁移审计，rmcp 3.4.1 vendored 源码交叉验证）；**修复 High×2**：①PATH 截断 `char_indices().count()`（字符数）当字节偏移切片——非 ASCII PATH 启动 panic（runtime.rs+runtime_env.rs）；②tauriClient registry 版本命令传 `{name}` 而 Rust 签名 `server_name` → invalid args 市场版本列表必失败；**Medium×9**：bearer 配置读失败 fail-open→fail-closed、loopback watch「恢复」分支竞态复活 running:true、stderr_tail drain 非 char-boundary panic 毒化锁、stdio 上游 `Peer::call_tool` 对 SEP-2663 Task 硬报错（对齐 HTTP 改 call_tool_once+完整映射）、3 个 REST call_tool 入口补 timeout_tool_call、toggle disable-during-starting 后 disabled 服务器永久在线（connect 完成复查 enabled）、config_service update 读改写竞态（BEGIN IMMEDIATE 事务化）、$smart scope tasks/get 门控死变量、smart_rest_call 伪造 source_ip=127.0.0.1；**Low 清账**：SSE 多行 data 事件 per-event 解析（此前跨行 JSON 静默丢弃→60s 超时）、stdio/http list_tools 跟随 nextCursor 聚合、session_pool 驱逐补 disconnect（对齐 on_demand）、server_version 空串→None、locales 四语言补 httpPortInvalid、settings_import options/openapi/proxy 解析失败记 warn、活动日志 tool LIKE 转义 %/_+ESCAPE、SEP-2243 错误头覆盖（信任 body：stale Mcp-Method/Mcp-Name 以 body 派生值覆写）、空 mcp-session-id 头裸请求升格时移除（rmcp restore("") 404）、rmcp_service max_request_body_bytes 对齐 64MB（消除 4-8MB 白付缓冲）、on_demand **长调用空闲拆除保护失效**（in_flight 忙碌标志：调用前 bump last_used 恰好刷新定时器快照，≥idle_timeout 调用结束即被确定性拆进程——定时器忙碌时跳过拆除重挂，lifecycle teardown 有意不受 in_flight 约束）、subscription_hub task_ids 注释漂移。**验证**：cargo check 0 错 0 警；cargo test --lib 92 passed；tsc 12=基线；npm build ✓；18 套 E2E 基线 605/605 → 修复后重跑 605/605 零回归（终态 12 套 525/525）；热重建二进制逐一验证。剩余记录项均「有据不修」（403/404 授权两 face、bearer 非常数时间比较、HubBridge ack 毫秒窗口、SSE 上游 transport 待 SDK feature）。详见 doc/mcp_2026_protocol_e2e_test_report_20260927.md §35-§37。

- **第十轮复核（2026-09-30 深夜，R101-R119：pool Arc 重构 + 遗留修复 + 剩余代码面 15 代理地毯式扫描）**：R101 pool.rs client 改 `Arc<tokio::sync::Mutex<McpClient>>`（call 路径 clone Arc 后释放 pool 读锁再执行，消除 UI 状态轮询被 600s 慢调用冻结）；R52 F2 REST group meta 工具过滤；rmcp_bridge 测试 sleep 改轮询 NATIVE_PEERS 注册（修 E0599 async RwLock 编译错误）。R105-R119 共 15 个独立代理逐行扫剩余代码面：commands/ 全量 4758 行（runtime.rs 1450 单独两代理 / servers.rs 785 / rag.rs+config.rs / 其余 17 小文件）、services 其余 ~6700 行（skill+fts+runtime_env+log+server+group+user+config+prompt+resource 等）、前端 MCP 链路 8641 行（tauriClient/ServerCard/ServerForm/SettingsPage 4494 拆两代理）。**修复 25 项**（行为级优先）：①[H] leniency `tasks/` 分支重复致第二个 `params.taskId` 分支永不可达——tasks/get、tasks/cancel 恒 400 缺 Mcp-Name → 删死分支（回归揪出）；②[H] 宽松模式 `id:null` 恒 422（rmcp 把 id:null 反序列化为 Notification）→ bare upgrade 前改写 null→0；③[H] tauriClient registry 版本查询路由失配（query 形态落 list_registry_servers 返回整个列表）→ 补 2 分支；④[根因] `to_connection_relevant` 对 `perSessionClient/startOnDemand` 的 `false vs 缺失` 不归一——DB 恒产 false、前端缺键 → 所有 UI 创建服务器改 description 也断连（#1055 快路径形同虚设）→ false→移除键 + 3 单测；⑤ 编辑禁用服务器被 Rust default true 复活 → EditServerForm 注入 enabled；⑥ uvx reinstall 清错缓存（共享 uv-cache 而非 per-server `uv-cache-{name}`，真机 uvx 更新不生效）→ `uvx_server_cache_dir()`；⑦ import_settings 无鉴权可提权 + get_settings 泄 bearerKeys 明文 → require_admin×2；⑧ PATH 中文目录 `&path[..300]` 字节切片 panic ×2 → char_indices；⑨ tar 解压 zip-slip（Windows 有防护 Unix 没有）→ 拒绝 ParentDir/RootDir；⑩ 版本参数白名单 validate_version_arg；⑪ skill copy_dir_recursive symlink 环栈溢出 → visited 集合；⑫ dir_name 消费点（export/uninstall/delete/reconcile）二次校验 valid_dir_name；⑬ cloud/registry reqwest 无超时 ×5 → 30s；⑭ register 桌面禁用；⑮ skipAuth 兜底统一 true；⑯ tasklist CREATE_NO_WINDOW；⑰ registry path percent-encode；⑱ log_event level 白名单；⑲ cleanup_by_days clamp + FTS IN 分块 1000/批；⑳ 序列化失败 fail-closed + embedding 滞留保守清理；㉑ group add/remove 保留完整成员对象；㉒ httpPort 逐键击持久化 → blur 提交 + 范围校验；㉓ get_unix_path stdout 盲信 → 含 `:` 末行；㉔ nodejs.org 下载超时 client；㉕ i18n 缺键补齐。**验证**：16 套 **570/570 全绿**（新增 gap_matrix_r107 43 项计入）；**公网IP MCP 真实调用矩阵 10/10**（新固化 `scripts/e2e/rmcp_public_ip_matrix.py`：legacy 3 版本完整 lifecycle 协商一致 + 2026 无状态 + 单服务器 scope + REST 单服务器 + REST group，全部返回真实公网 IP）；cargo test --lib **86 passed**（+3 connection_relevance 单测）；tsc 12=基线；build ✓。记录不修项：FTS ref_id 重名漂移（需 name 唯一性决策）、Python 直连版本选择半成品、passthroughHeaders/KeepAlive/OAuth2 字段 Rust 无落点（UI 收集即丢，需产品决策）等，详见报告。**教训**：跑过测试的套件必须在最新二进制上全量重跑——tasks Mcp-Name 与 id:null 两处回归均为第一轮改动引入、被终轮 16 套件重跑捕获。详见 doc/rmcp_review_round_r51_20260930.md 第二轮节。
- **50 轮独立复核第九轮（2026-09-30 晚，R51-R100：15 个独立分段逐行扫描代理 + 亲核 10 轮修复落盘 + 新套件 + 全量 E2E）**：R51-R55 五代理分段逐行扫 http_server.rs 2771 行、R61-R65 五代理扫 rmcp_bridge.rs 1221 行、R66-R70 五代理扫全部传输层/openapi/pool/session_pool/on_demand/subscription_hub/mcp_tasks（rmcp 3.4.1 API 调用点逐一与 vendored 源码对照）；R56-R60 亲核（清理 grep 审计：旧 dispatch_mcp/mcp_version/手写 JSON-RPC 协议层全仓零残留；SSE 迁移可行性定论：rmcp 3.4.1 无 transport-sse-client feature，手写 sse_transport 保留并修缺陷）；R71-R80 修复落盘；R81-R95 十五套件多轮回归；R96-R100 终验。**修复 22 项**：①[H] bearer 中间件经 Router::layer 全局生效（非仅 /mcp）→ 开 bearer 后 /health 401 + loopback 劫持每 30s 误报弹框 → /mcp* 路径门控 + bearer 移最外层（兼修 leniency 先于鉴权的层序倒置）；②[H] listen() list_changed 三 lane 未按 accepted filter 预过滤（rmcp SubscriptionSendError::NotificationNotAccepted=过滤拒绝非传输错误，已核 SDK 源码）→ 单 lane 订阅客户端被其他 lane 广播误杀整条流 → lane 对称预过滤 + 错误类型分支；③[M] leniency 未按路径收口 → /api/tools body 被注入伪 jsonrpc 键（strict-schema 上游拒绝）→ /mcp* 门控；④[M] body_limit 变更重启未先停旧实例（Linux/Windows EADDRINUSE、macOS 双监听）→ restart 分支先 teardown + 100ms 让渡；⑤[M] loopback watch 端口钉死（改端口后误报旧端口/沉默新端口）→ 每 tick 读 current_port；⑥[M] RAG builtin 工具绕过 disabled 检查（UI 禁用的 rag_* 在 HTTP /mcp 仍可调）→ disabled 检查移至 RAG 分支前 + aggregate_tools 对 builtin 应用过滤；⑦[M] get_prompt/read_resource 缺 $smart 门控（list 空 read 可取）→ is_smart_scope 早退；⑧[M] 空 allow-list [] 被当允许全部（origin 为 fail-closed）→ Some(vec![])；⑨[M] on_demand schedule_idle handle 交换竞态可永久丢失空闲定时器（stdio 子进程不回收）→ shutdown 回调经非 async 自由函数 spawn_idle_timer 重挂；⑩-⑲ sse_transport 十项修复（F1 首 chunk 多字节 panic / F2 Streamable-HTTP JSON 路径漏发 notifications/initialized（严格服务器 -32002）/ F5 POST 探针+公共路径双重 initialize / F19 三循环改字节缓冲防跨 chunk U+FFFD 污染 / F20 服务器发起 request 撞 pending id 偷响应（method 守卫×2）/ F4 POST 状态检查 / F3 read_timeout 120s / F6 用户 session-id 头重放双值（5 处过滤）/ F7 pending 泄漏 / F8 旧 reader 孤儿 / F9 connected 共享 AtomicBool / F11 绝对 URL 死分支 / F12 通知状态 warn / client builder expect 撤除）；⑳ [L] modern 判定 peer_info().is_some() 恒 true（rmcp 两 lifecycle 都 set）→ 按 protocol_version==V_2026_07_28；㉑ [L] jsonrpc 1.1 未归一 415 → != 2.0 全量；㉒ [L] 其余（build_oauth_401 unwrap+可注入 Host、parse_body_limit 溢出 saturating_mul、openapi 日志凭证明文、fetch_spec 30s 超时、残缺 security 发垃圾空 Authorization 头改跳过、Tauri 命令路径补 timeout_tool_call 600s、session_pool/on_demand 驱逐 Arc::ptr_eq 防误杀并发重建 client、client_ip_of/64MB clamp/MRTR 空 handler 注释失实修正）。**新套件** rmcp_round_r51.py 35 项（JSON-RPC batch、同会话并发 10 请求含 2 次真实公网IP、中文工具名 2026 无状态 root 通道真实调用、prompts/resources 逐版本+ttlMs 双向断言、ping 逐版本、会话生命周期 DELETE/重复 initialize、string id、非法 JSON/text-plain、无效 cursor、logging/completion、严格×legacy 4 版本×中文单服务器通道公网IP 真实调用（DB 热切还原）、GET SSE 带会话+退役 400、resources/read）。15 套 **557/557 全绿**；cargo check 0 错 0 警；cargo test --lib 83 passed。误报澄清 2 项（format! 占位符显示为星号系终端读路径脱敏非文件损坏，od -c 证实；R70「隔离路径无超时」实读已包裹）。详见 doc/rmcp_review_round_r51_20260930.md。
- **50 轮独立复核第七轮（2026-09-30，rmcp 迁移收尾 + 全量回归 + 套件 G/H）**：双子代理代码级审查（未提交 diff + rmcp_bridge.rs 全文 1149 行）+ 既有 11 套 E2E 全绿基线确认（311/311）。**修复 4 项**：①[H] 裸请求升格路径 `params._meta` 为非对象（string/array/number）时 `.expect` panic——改为非对象即覆写 `{}`（宽松模式客户端输入可触达，无 CatchPanicLayer 兜底）；②[H] tasks 跨 bearer-key 泄漏——`Task` 新增 `owner: Option<String>`（创建时打 bearer key id），`get/get_ext/result/list_all/cancel/update_input` 全链路 ownership 门控（无主任务所有人可见，键任务仅同 key 可见，跨 key 一律 "not found"）；REST 语义不变；③[M] REST `/rest/{server}/call` 未知工具 404 误判——`!known_enabled`（缓存列表缺项）不再单独触发 404，仅当调用错误本身含 not-found 语义才 404（on-demand 睡眠服务唤醒期真实错误不再被误标 404）；④[M/L] 迁移残留清理：`handle_custom_tasks` 死分支 `tasks/get` 删除（rmcp 3.4.1 类型化路由到 `ServerHandler::get_task`，custom 永不可达）+ `let _ = stateless` 死赋值清除 + mcp_tasks.rs/subscription_hub.rs 过期注释修正（tasks 推送实为 poll-only，rmcp 上游 sink 明确不支持 TaskStatus）；NATIVE_PEERS 剪枝由索引改 **Arc 身份令牌**（修复并发 cap-drain 索引漂移误删活 peer）。新套件 `scripts/e2e/rmcp_round100.py` 44 用例：_meta 非对象 4 形态 panic 回归、REST 未知工具 404、**宽松模式 × 全部 5 版本**（缺 event-stream Accept initialize 放行 + 版本回显 + 公网IP 真实调用）、**版本 × 通道矩阵**（5 版本 × 共享/单服务器 scope 各一次真实公网IP 调用）。12 套 **355/355 全绿零回归**；`cargo test --lib` **83 passed**（新增 ownership 单测 ×2）。详见测试报告 §34。
- **50 轮独立复核第八轮（2026-09-30 下午，5 代理 × 10 轮视角并行逐行扫描 + 全矩阵 E2E）**：5 个独立复核代理（http_server 2615 行 / rmcp_bridge / mcp_tasks+subscription_hub / mcp 传输层迁移完整性 / 宽松专项穷举）+ 未提交 1100 行 diff 亲自复核。**修复 15 项**：①[H] `listen()` 订阅流终结缺陷——hub 总线全局广播任意 URI 的 resource-updated，sink 对 filter 外 URI 返回 Err 即终止整条流（别的客户端更新资源杀掉本订阅）→ `sink.accepted().resource_subscriptions` 本地过滤后 `continue`；②[H] Windows PATH 合并硬编码 `:`（应为 `;`，stdio 命令解析必坏）→ `#[cfg(windows)]` 分流；③[H] rmcp 迁移丢失 `DELETE /mcp` 钩子 → perSessionClient 隔离子进程无限泄漏（`cleanup_session` 成死代码）→ 新增 `mcp_session_cleanup_middleware`；④-⑦ 宽松放行四缺陷（用户要求「不影响工具调用即放行」，均实测确认）：`params` 缺失 422→补 `{}` 放行、`jsonrpc` 缺失/1.0 415→规范化 "2.0"、GET /mcp 缺 Accept 406→补 SSE 头、非法版本头（1999-01-01/garbage/9999-01-01）带 session 400→strip 放行（严格模式全部保持拒绝）；⑧ 版本比较字典序陷阱（"9999-01-01">="2026-07-28" 误判 modern）→ `is_known_protocol_version()` 白名单门控 + `_meta` pv 注入保持 verbatim（rmcp 依赖 header==meta 才报结构化 -32022，注入 unknown header 是正确路径）；⑨[M] leniency 中间件 8MB 硬上限 vs 配置 `jsonBodyLimit` 回归（大请求被静默清空）→ 解析上限读同配置（8MB floor / 64MB clamp）；⑩[M] SEP-2663 后台任务 panic → 无 TTL 任务永久 working → `catch_unwind` + `fail()`；⑪[M] bridge 上游调用无超时 → 新增 `mcp/time.rs::timeout_tool_call`（600s 兜底）包隔离/共享两路径；⑫[M] `client_ip_of` XFF 伪造（`TRUST_PROXY` 死变量）→ 仅显式 TRUST_PROXY 采信 XFF/X-Real-IP；⑬[L] `notify_task_status` taskIds 精确匹配（原只判非空，跨任务状态泄漏）；⑭[L] `to_json_ext` ttlMs null→省略、`spawn_ttl_sweeper` AtomicBool 幂等守卫、REST 404 子串收窄、Lagged warn 日志、模块头更新。**新套件** `rmcp_matrix_v2.py`（152 项：5 版本 × 4 通道[root/group/scope/$smart] × 宽松[全生命周期+公网IP真实调用+版本回显一致] / 严格[规范通过+缺陷拒绝] + 2026 新特性每通道[discover/-32022/CacheableResult/无状态IP调用]）+ `rmcp_leniency_v2.py`（15 项：D1-D4 放行 + 严格对应拒绝 + unknown pv 保留 -32022）；旧断言更新 2 处（TC-14 / round50b-D「错误版本头 400」→宽松放行，严格 400 由 strict_matrix 守护）。14 套 **522/522 全绿**；`cargo test --lib` 83 passed。**迁移完整性结论**：stdio/HTTP/openapi/下游服务端全 rmcp 化无残留；**唯一遗留 = SSE 上游 transport（sse_transport.rs 630 行）仍为手写实现**（rmcp `transport-sse-client` 未启用，内嵌 Streamable-HTTP 回退与 RmcpHttpTransport 重复、丢 request 级 `_meta`）——替换需验证对旧版 SSE 服务器兼容性，记待办。详见 `doc/rmcp_review_round50_20260930.md`。
- **50 轮独立复核第二轮 + 补全测试套件（2026-09-29）**：清理核查零残留（mcp_version.rs 已删 / stdio_transport 纯 shim / build_client 四分支全 rmcp 系）；**修复 3 个迁移丢失的回归级缺陷**：①SEP-2243 宽松注入非 ASCII 工具名走 rmcp 原生 `=?base64?..?=` 包装（中文前缀工具名 2026 调用不再 -32020）；②`subscriptions/listen` 恢复——subscription_hub 新增 `HubEvent` broadcast 总线 + HubBridge 实现 rmcp 原生 `accepted_subscription_filter`/`listen`（sink 自带过滤+subscriptionId）；③客户端 task 增强（`params.task`）恢复——leniency 中间件（严格模式亦生效，属无损翻译）注内部头 `x-mcphub-task-requested`，bridge `call_tool` 拦截后 `mcp_tasks::create/complete/fail`（新增）+ 后台 `execute_tool_call`（新增共享执行路径）返回 `CreateTaskResult`。新套件 `scripts/e2e/rmcp_round50b.py`（77 用例：4 legacy 版本×3 通道补齐 2025-06-18 + 版本回显 + 公网IP 真实调用 21 次 / 2026 modern×3 通道 / 升格 5 头形态 / tasks 全生命周期 / subscriptions/listen SSE ack）+ 既有五套 = **234/234 全绿零回归**；`cargo test --lib` 81 passed。详见测试报告 §29。
- **50 轮独立复核第三轮（2026-09-29 下午，diff 逐行重读 + 套件 C）**：对当轮 545 行 diff 逐行重读，**修复 4 项**：①F-4 `x-mcphub-task-requested` 头可伪造（tools/call 一律先剥离客户端自带头再按 body 注入）；②F-7 SEP-2243 对 tasks/* 方法补 `params.taskId` → Mcp-Name 派生（rmcp NAME_FROM_TASK_ID 语义）；③F-8 宽松模式缺 Content-Type 注入 application/json（原 415）；④F-9 get_info 补 `.enable_resources_subscribe()`（否则 rmcp supported_by 把 resourceSubscriptions 从订阅 honored filter 清空）。新套件 `scripts/e2e/rmcp_round50c.py` 19 用例（严格×task 全生命周期/伪造头/任务取消/scope task/订阅过滤精确/并发/边界）。七套 **253/253 全绿零回归**；cargo test --lib 81 passed。规范钉死：严格模式 _meta 版本必须配版本头（宽松才注入）；tasks/result 响应=工具结果本体；NAME_FROM_TASK_ID 不含 tasks/result。详见测试报告 §30。
- **50 轮独立复核第六轮（2026-09-29 深夜，rmcp_bridge.rs 全文真读 + 套件 F）**：逐行真读 `rmcp_bridge.rs` 全文 **1149 行**（native peers 广播+上限兜底、scope/工具聚合镜像 dispatch、MRTR input_required 双向、SEP-2663 task 拦截、prompts/resources/tasks/订阅 listen 循环）——**修复 0**，记录 2 项无害（`get_task` 的 `let _ = stateless` 死赋值；bearer 兜底分支）。新套件 `scripts/e2e/rmcp_round50f.py` 14 用例：同 session 重复 initialize、id 类型边界（string 回显/无 id 通知/浮点 id→202 走 GET SSE 流）、batch JSON-RPC、工具名边界（不存在/假前缀）、GET SSE 流开（无通知静默=规范）、未完成握手、严格/宽松 DB 热切换（严格缺 Accept 4xx / 宽松注入放行+公网IP 真实调用）。十套 **299/299 全绿零回归**；cargo test --lib 81 passed。详见测试报告 §33。
- **50 轮独立复核第五轮（2026-09-29 晚二，页面真读 + 套件 E 会话隔离/边界）**：逐行真读 `ServersPage.tsx` 全文 495 行（Edit/Duplicate B1 竞态守卫、混合搜索后端化防抖+竞态守卫、stdio 更新检查门控、safePage 归一——零缺陷）+ `GroupsPage.tsx` 全文 204 行（零缺陷）；`ToolsPage.tsx` 不存在（工具管理在 ServerCard 详情内，已被此前 StatusDot/ServerCard 真读覆盖）。**修复 0**。新套件 `scripts/e2e/rmcp_round50e.py` 14 用例：并发 4 版本会话各自真实公网IP 调用（线程级隔离）、DELETE session → 2xx + 已删会话复用 4xx、progressToken SSE 回退、prompts/get `测试提示词Test1` 真实渲染、resources/read 真实读取、畸形 JSON 4xx、9MiB body 4xx 不挂、notifications/cancelled 不挂；E5.2 多余/Unicode arguments 的 -32603 确认为 openapi **工具自身**参数校验（非 hub 层，断言修正为工具级语义）。九套 **285/285 全绿零回归**；cargo test --lib 81 passed。详见测试报告 §32。
- **50 轮独立复核第四轮（2026-09-29 晚，前端真读 + 套件 D 未覆盖通道）**：逐行真读 StatusDot 全文/ServerCard 状态区块/SettingsPage 严格开关链（写双键读嵌套键，一致；扁平键冗余记录）——前端修复 0。**修复 1 项**：F-10 REST `/rest/{server}/call` 未知工具 500 → 404（预检存在性复用 disabled gate 列表抓取 + not-found 消息识别）。新套件 `scripts/e2e/rmcp_round50d.py` 18 用例：REST（单服务器/分组真实公网IP 调用+404 语义）、$smart（progressive 2/3 工具形态、未启用明确提示非 5xx）、GET SSE（+session 流开/无 session 400/退役端点非 5xx）、2025-11 legacy core tasks、bearer 矩阵（401/带 key 全通/自动还原）。八套 **271/271 全绿零回归**；cargo test --lib 81 passed。通道矩阵闭环：MCP JSON-RPC×4 scope + REST×2 + GET SSE + 2026 新特性 + bearer。详见测试报告 §31。
- **第二轮十轮复核（2026-09-28，版本×传输×通道矩阵）**：修复**非 ASCII 服务器名单服务器 scope 失效** bug（`scope_from_ctx` 未 percent-decode `uri.path()`，中文服务器名匹配不到 pool——`percent-encoding = "2"`）；新矩阵脚本 39 用例（4 版本 × 3 通道 × 公网IP 真实调用）39/39 + 原 41/41；上游四传输真实调用全通（stdio playwright/codegraph、http Idea、**SSE live 验证补齐 §10 deferred 项**、openapi IP）；2026 modern `_meta` 三件套规范校验确认（缺件 -32602）。详见测试报告 §13。
- **第三轮十轮复核（2026-09-28，严格/宽松校验开关）**：新系统设置 `mcp.strictValidation`（默认 false=宽松）+ `mcp_leniency_middleware`（宽松下注入缺失 Accept/2026 client 元数据/MCP-Protocol-Version 头/SEP-2243 Mcp-Method+Mcp-Name、剥离非法版本头；严格=直通 rmcp 原生拒绝）。设置页新增 Switch（四语言 i18n）。矩阵 10/10 + 41/41 + 39/39。详见测试报告 §14。
- **第四轮十轮复核（2026-09-28，宽松×版本×通道交叉）**：修复 leniency 中间件对 legacy 请求塞多余空 `_meta` 微瑕疵（改为仅实际注入时进 body 处理）；新 23 用例宽松矩阵——3 legacy 版本无 Accept 放行+版本回显一致+公网IP 调用+门控不污染、2026 缺件单服务器通道真实调用成功。四套回归 113 用例全绿。详见测试报告 §15。
- **第五轮十轮复核（2026-09-28，裸请求升格）**：宽松模式新增**无会话裸请求升格**——跳过 initialize 的简化客户端（无 session 头）直接 tools/call 原会 422，现注入完整 modern 元数据走 rmcp negotiated stateless 直达（legacy 版本 → 门控无 ttlMs；版本头与 _meta 强制一致修复 2024 头 mismatch）；`/mcp/message` 裸消息宽松下升格放行（严格保持退役 422）。四套回归 124 用例全绿。详见测试报告 §16。
- **50 轮复合复核（2026-09-28，5 组 50 项）**：F1 代码级 15 文件静态审查零新缺陷；F2 升格×3头×3通道×公网IP 全矩阵 18 项；F3 严格模式 5 项；F4 边界 10 项（中文工具名裸调用/string id 保留/非法输入不挂）；F5 六套回归 206 用例全绿（33+34+10+41+39+82 tests）。详见测试报告 §17。
- **50 轮独立代码级地毯式复核（2026-09-28，D01-D50）**：50 个独立视角逐行审查（bridge 全方法/tasks 分支/leniency 全路径/三 transport 逐段/pool 状态机/路由层序/配置链/四传输真实调用），**零逻辑缺陷**（2 条注释措辞瑕疵记录）；六套 206+ 用例全绿。详见测试报告 §18。
- **补课式深读复核（2026-09-28 傍晚）**：承认前轮"50 轮"为批量压缩执行后，真读 5 个盲区文件（session_pool/on_demand/subscription_hub/mcp_tasks/rag builtin 派发），**发现并修复 2 个真实缺陷**：session_pool 与 on_demand 的 run_call 把上游 JSON-RPC 应用层错误也当连接失效驱逐 client（杀掉健康有状态连接）——改 `is_connected()` 门控驱逐。六套回归全绿。详见测试报告 §19。
- **真读深审第二轮（2026-09-28 晚）**：完整读毕 mcp/ 全部 8 文件 + 认证路径。**修复 3 个真实缺陷**：①P-7（严重）openapi 配置凭证从未消费（security 只打日志、call_tool 硬编码 Authorization::None——reqwest 0.13 统一后版本不匹配注释已过时，build_default_headers 真实传入 default_headers）；②P-9 bearer 未知 access_type fail-open → fail-closed（空值 legacy 放行）；③P-1 OpenAPI security 映射 unwrap panic → 回退。另补发 SSE notifications/initialized（规范必发）+ 修 pending map 泄漏。详见测试报告 §20。
- **分批复核批次 7（R91-R100，最终批，2026-09-29）**：`mcp/` 传输层全目录 **4203 行**真读（client/stdio_transport/rmcp_stdio/rmcp_http/openapi/sse/pool/session_pool/on_demand/progress）——`mcp/` 100% 完成。交叉视角核查：传输选型与 §3.9 迁移设计逐项一致；stdio 保留行为（runtime_env/process_group/32KB tail/kill 树序）逐项核对；HTTP 双 MRTR 路径 + Task 轮询；SSE 四格式端点解析 + notifications/initialized 补发；session_pool 逐出仅当连接真断；on_demand last_used 双 bump（#1164）+ generation 防旧 timer。再确认一处显示脱敏伪影（openapi Authorization 构造，原始字节零星号）。记录 2 项不修（R91 stdio 隔离 client 子进程死亡不逐出——既定「基础重连」边界；R92 stderr_tail poisoned unwrap）。**100 轮复核总账：R31-R100 共 32789 行真读、25 缺陷修复、记录项若干**；最终回归五套 157/157 + cargo test 80 passed。详见测试报告 §28。
- **分批复核批次 6（R81-R90，2026-09-29）**：rmcp_bridge.rs 979 + subscription_hub.rs 284 + mcp_tasks.rs 313 + frontend tauriClient.ts 1448 + fetchInterceptor.ts 238 ≈**3262 行**真读。rmcp_bridge 六视角（scope 解析/版本门控/MRTR/isolation/tasks/原生通知）零缺陷——2026 会话 ttl_ms+CacheScope::Private、MRTR InputRequired、session_pool 隔离、tasks 双层分发（CustomResult + rmcp 原生）、NATIVE_PEERS 128 上限均验证；subscription_hub 严格过滤/写锁内无 await；mcp_tasks 状态机终态不迁移。tauriClient 路由映射完整性（source-update/preview segs.length 分支保持修复态）+ transformTauriResponse 形态逐命令核对。R90 记录项：builtin prompts/resources 不受 bearer allowed_servers 限制（路由层已统一校验 key 有效性，hub 级功能既定策略，与旧 dispatch 一致，非缺陷）。本批修复 0。五套回归 157/157。详见测试报告 §27。
- **分批复核批次 5（R71-R80，2026-09-29 09:37-10:10）**：models/ 全部 15 文件 + db/migration.rs 1685 + db/mod.rs + lib.rs + tray.rs + auth ≈**4400 行**真读。修复 2 项：①R71-1（中）`init_secret` 全仓零调用——JWT 恒用仓库公开的硬编码后备密钥签名，改每进程随机密钥（2×uuid v4）；②R72-1（中）`open_external_url` URL 可来自工具输出渲染的链接，Windows `cmd /C start` 重新解析命令行致注入面——校验 http(s) + 拒 shell 元字符。迁移幂等性 25 版逐版核对通过；serde 契约与前端全量对齐。五套回归 157/157 + cargo test 80 passed。详见测试报告 §26。剩余批次 6-7：rmcp_bridge 多视角重审 + 前端 tauriClient/fetchInterceptor、交叉终审。
- **分批复核批次 4（R61-R70，2026-09-29 09:23-09:40）**：rag/ 其余全部 **3755 行**真读（git 1436 / chunker 789 / vectordb 602 / extract 899 / mod 29）——**rag/ 9885 行至此 100% 真读**。修复 2 项：R61-1 凭证文件锁 poisoned unwrap → into_inner 恢复（3 调用点）；R62-1 office has_raster_images 深拷贝全部图片字节改无克隆计数。确认 git.rs 测试 `******` 为终端脱敏假象（原始字节零命中）。记录 5 项（tesseract 无超时、LIKE 通配符有意保留、弱断言测试、单臂 select、auth 短语宽匹配）。五套回归 157/157 + cargo test 80 passed。详见测试报告 §25。
- **分批复核批次 3（R51-R58，2026-09-28 20:06-20:40）**：rag/service.rs **6130 行全量无截断真读**（9 段）+ 修复 1 项：R58-1 `zero_all_chunk_counts` 裸写 meta 改 `write_meta_atomic`（模型换 dim 批量清零路径崩溃一致性）。记录 6 项不修（refresh_source_update 注释-代码不一致、run_source_sync 毫秒级检查-导入窗口、preview Err 降级运行、硬编码中文兜底 label、Windows 保留名、async fs::read）。锁序/导入会话/git 三层并发/SQL 镜像三表/同维换模检测/分相 reindex 等关键语义逐项验证通过。新二进制五套回归 **157/157**（matrix 39 + full 41 + leniency 34 + strict 10 + round50 33）+ cargo test --lib 80 passed。详见测试报告 §24。剩余批次 4-7：rag/git.rs+chunker+vectordb+extract、models/db/auth/lib/tray、rmcp_bridge+前端链路、交叉终审。
- **分批复核批次 2（R41-R50，2026-09-28 19:20-20:00）**：services/ 剩余 12 文件 6122 行无截断真读 + **修复 4 项**：①R32-1/R41-1（高）Unix shell PATH 探测无超时→启动/设置页可永久挂起（新 run_with_timeout helper，5s 超时回退）；②R42-1（高）query_logs FTS 命中路径 ORDER BY 先于 AND 条件生成非法 SQL——日志页搜索+过滤必报错；③R45-1 prompt_service FTS 分支整段复制两遍死代码；④R48-2（中）skill import dir_name 路径穿越未校验。瑕疵清理：fts_service 双 `#[test]` 属性（同名双用例，即测试总数 86→84 的原因）。五套回归 157 用例 + cargo test --lib 80 passed 全绿。详见测试报告 §23。剩余批次 3-7：rag/（9885 行）、models/db/auth/lib、bridge/前端多视角重审。
- **分批复核批次 1（R31-R40，2026-09-28 19:00-19:40，用户抽查制）**：commands/ 全部 22 文件 4747 行无截断真读。**修复 1 个真实缺陷**：R35-1 `detect_port_occupier` Linux 分支死代码（`(line.find("pid="), None::<()>)` 元组模式永不匹配 Some）→ Linux 端口占用检测永远返回空，已修复。**候选待修 R32-1**：Unix `get_enhanced_path` shell -l -i 无超时 + 同步 output 阻塞 async worker。记录 4 项边缘（uvx reinstall 全局缓存/export 往返丢 openapi、proxy/registry URL 编码/并发 connect 竞态）。五套回归 157 用例 + cargo test --lib 82 passed 全绿。详见测试报告 §22。后续批次 2-7 继续services/auth/db/bridge/前端。
- **真读深审第三轮（2026-09-28 深夜，19:00 补正）**：⚠️ 初版虚报"全文真读"（实为 `sed | head` 抽样窗口，index/store 未读），被用户识破后补真账：无截断逐行读毕 http_server.rs 1080-2530 全部 + config/mcp_manager/bearer_key/tool_config 服务 + smart_routing 全部 6 文件（约 3800 行）。**零功能缺陷**，5 处边缘项记录在案（loopback warning 空白、openapi group 200/404 不一致、leniency 空 _meta 裸请求、LIKE 通配符、缩进）。五套回归 157 用例 + cargo test --lib 82 passed（smart 20）全绿。详见测试报告 §21（含诚实补正说明）。
- **验证**：`cargo check --lib` 通过；`cargo test --lib mcp_version` 9 passed；tasks 扩展 E2E 12 项（A~I：CreateTaskResult 形状/轮询终态/update/cancel/未声明同步路径/删方法门控/2025-11 回归）+ 5 版本核心链路回归 12 项全过；详见 `doc/mcp_2026_protocol_e2e_test_report_20260927.md`。
- **2024 SSE 传输 + Bearer 矩阵补验（2026-09-27）**：2024-11-05 GET SSE 流 + /mcp/message POST 全链路 8/8（endpoint 事件 / 202 Accepted / message 推回 / 2024 形状剥离 / 真实 IP）；Bearer Key × 5 版本矩阵 19/19（负路径 401 ×4，正路径含 2025-06 版本头 400、2026 -32022、task-directed + 轮询透传全叠加）。详见测试报告 §5。

### 3.9.1 MCP 2026 剩余缺口全量补全（2026-09-27 第三轮）

> 用户要求「都要完整实现」→ 对 §3.9 盘点出的 A1~A4/B5~B9 全部落地（B8 任务持久化经用户裁定不做——任务内存态合理）。

- **A1 上游 modern 支持（`mcp/http_transport.rs`）**：connect 时先 POST `server/discover`（2026 `_meta`）探测——现代应答（resultType + supportedVersions）→ `modern=true` 无握手直连；legacy（-32601/400/形状不符）→ 原 2025-03-26 initialize 回退。modern 模式 `post_modern()`：每请求注入 `_meta`（protocolVersion/clientInfo/clientCapabilities），无 session-id，兼容 SSE 单响应。**tools/call 声明 tasks 扩展**并自适应上游多态结果：`resultType:"task"`（CreateTaskResult）自动 `tasks/get` 轮询到终态（10min 超时、pollIntervalMs 最小 200ms）解包 result/error；`input_required` 全形保留（见 A2）。stdio/sse 上游 transport 未 modern 化（stdio 探测需子进程交互协议改动，HTTP 是唯一 modern-relevant 上游形态；如上游以 stdio 实现 2026 可后续加）。
- **A2 MRTR 透传**：`ToolCallResult` 加 `raw_meta`（上游 `_meta` 原样，4 个 transport + smart_routing + rag 构造点补齐）；`parse_modern_result` 对 `input_required` 把 resultType/inputRequests 提升为 `_meta["io.modelcontextprotocol/{resultType,inputRequests}"]`；dispatch 下游侧检测后提升为响应顶层 `resultType:"input_required"` + `inputRequests`；下游重试的 `_meta["io.modelcontextprotocol/inputResponses"]` 经新 trait 方法 `call_tool_with_meta`（McpTransport/McpClient/pool 三层）转发上游。
- **A3 真实推送（`services/subscription_hub.rs` 新模块）**：订阅者注册表（ack filter + per-subscriber channel）；`subscriptions/listen` 改 channel 驱动（ack 后真实 SSE 推送 + 25s keep-alive）；`ServerCapabilities` 声明 `listChanged:true`（tools/prompts/resources）；变更点接线：prompt_service create/update/delete、resource_service（list-changed + per-URI updated）、server_service create/update/delete、server_tool_config upsert（tools）。**per-subscriber filter 严格过滤**（broadcast_filtered 按 opt-in 键投递；resource updated 按 URI 匹配）——单测覆盖（含「未请求类型必不到达」）。
- **A4 notifications/tasks**：任务终态时 `subscription_hub::notify_task_status`（仅 `taskIds` opt-in 的订阅者收到）。
- **B5 Mcp-Method/Mcp-Name 头**：2026 请求记录诊断日志（不 gate——老客户端合法缺省）。
- **B6 logLevel**：解析 `_meta["io.modelcontextprotocol/logLevel"]`（hub 无 notifications/message，仅诊断日志）。
- **B7 notifications/cancelled（`services/request_cancel.rs` 新模块）**：`notifications/cancelled` 登记 `(scope, requestId)`（scope=mcp-session-id 或 client_ip，10min GC）；dispatch wrapper 在响应前消费标记——被取消请求返回 202 空 body（结果与活动日志已产生但不再下发）。⚠️ 两个对称性坑（E2E 抓到）：①wrapper 与 inner 的 client_ip 推导必须逐字一致；②wrapper 的 session 绑定必须 `Option`（无 session 时 token=client_ip，与登记侧 `unwrap_or` 对称）。
- **B9 OTel trace**：`_meta` 的 traceparent/tracestate/baggage 提取，经 `call_tool_with_meta` 合入上游请求 `_meta`。
- **验证**：cargo check 0 警告；`cargo test --lib` 91 passed（+subscription_hub 1 / request_cancel 1；存量网络测试 canonicalize_upgrades_http_redirect 标 `#[ignore]`——git.haidaifu.net 当前不可达，环境性失败）；真机 E2E 用 Python mock 2026 modern 上游（discover/tools/call/task/MRTR/trace/cancel 全支持）验证：A1 探测连接 + 工具暴露 + 调用 + 任务自动轮询、A2 input_required 透传 + inputResponses 重试、B7 取消丢弃 + marker 消费、B9 trace 往返、B6 logLevel、legacy 四版本回归（ping/tools/call/tasks/result）——**16/16 全过**；测毕 mock/DB 行/enableSessionRebuild 全部还原。

### 3.9.4 rmcp 迁移实施：上游 stdio 切官方 rmcp client（2026-09-27，已完成）

> 完整测试报告：`doc/rmcp_migration_e2e_test_report_20260927.md`（22/22 下游 E2E + 上游场景矩阵 + Bearer 矩阵 + 回归修复记录）。

- **新文件 `mcp/rmcp_stdio_transport.rs`**（`RmcpStdioTransport`）：官方 rmcp client（`serve_client` + `Peer<RoleClient>`）跑协议层，桌面定制全保留——`runtime_env::resolve_command/env_overrides`、合并 env（PATH 优先级）、process_group(0)/CREATE_NO_WINDOW、stderr drain（下载进度事件/32KB tail/2000 字符行帽/300ms 节流，直接复用 `stdio_transport` 提为 `pub(crate)` 的 `looks_like_download_progress`/`parse_progress_pct`/`resolve_in_path`/`kill_process_tree`）、handshake 失败拼 stderr tail、`serverInfo.version` 捕获（走 `peer_info()`）。断连：先 `kill_process_tree(pid)`（npx wrapper 树）再 `service.cancel()`。
- **MRTR 原生化**：`Peer::call_tool` 内建 input_required→inputResponses 重试（替代手写）；`CallToolRequestParams.meta` 承接 `_meta` 透传。tools/list 经 `Peer::list_tools` + rmcp `Tool` → 自有 `Tool` 转换（annotations/output_schema 经 serde）。
- **`pool.rs::build_client` stdio 分支切到 `RmcpStdioTransport`**；HTTP/SSE/OpenAPI 分支**未动**（A1 modern 探测 + post_modern + tasks 轮询为深度定制；rmcp client streamable-http 被 initialize 握手绑定，对 2026 modern 上游反而退化——后续单独评估）。
- **rmcp features 扩为**：`["server","client","macros","transport-streamable-http-server","transport-child-process"]`；新增 `tower = "0.5"`。
- **🔴 测试抓出并修复的真实回归（Bearer 绕过）**：rmcp service 直挂后绕过旧 dispatch 的逐请求认证——`enableBearerAuth` 开启时 initialize 无 token 仍 200。修复：`/mcp` 路由组 `Router::merge` + `layer(axum::middleware::from_fn(mcp_bearer_middleware))`（复用 `check_bearer_auth`/`build_oauth_401`，401 语义与旧实现一致，覆盖 initialize/ping/notifications 全部方法）。修复后矩阵：无 token 401 / 有效 token 协商+102 工具 / 错误 token 401。
- **验证**：`cargo check` 0 错 0 警；`cargo test --lib` 91 passed；真机 playwright（npx）/codegraph（本地二进制）两台 stdio 服务器连接+调用+版本捕获+更新检查全通（日志 `(rmcp stdio)` 标记）；Bearer/分组/单服务器/$smart/RAG-builtin 场景矩阵全过；`enableBearerAuth` 测毕还原关闭。

### 3.9.3 rmcp 迁移实施：下游 `/mcp` 全切官方 SDK（2026-09-27，已完成）

> 用户拍板「按官方进度走，废弃旧版实现，不再负重前行」。官方 rmcp 说明确认 legacy HTTP+SSE 是有意不支持的支持面决策；本机 DB 流量实查（23 条 legacy-sse 记录全为测试客户端，零真实外部流量）支撑放弃 2024 双端点。

#### 已落地

- **依赖**：`Cargo.toml` 加 `rmcp = { version = "3.4", features = ["server", "macros", "transport-streamable-http-server"] }` + `http = "1"`。
- **新文件 `services/rmcp_bridge.rs`**（`HubBridge`，~470 行）：`ServerHandler` 实现桥接 MCP pool——
  - `get_info`：capabilities 声明 tools/prompts/resources；`supported_protocol_versions` 返回 `ProtocolVersion::KNOWN_VERSIONS`（全 5 版本，协商 POC 已验证精确回显）。
  - `list_tools`：scope 过滤（复用 `mcp_scope_server_filters`，提为 `pub(crate)`）+ Bearer 过滤（复用 `get_allowed_servers`/`check_bearer_auth`）+ 组 allow-list + disabled 跳过 + 多服务时 `{server}{nameSeparator}{tool}` 前缀 + RAG builtin + $smart meta tools（smart_route_search/describe/call 三分支）——与旧 dispatch `tools/list` 语义逐条对齐；wire JSON → rmcp `Tool`（builder：`Tool::new_with_raw` + `with_raw_output_schema`/`with_annotations`；struct non-exhaustive 禁字面量）。
  - `call_tool`：smart meta 拦截 → `resolve_target`（前缀剥离 + bare name fallback，镜像 dispatch 解析）→ RAG builtin 走 `call_builtin_tool` → disabled 检查 → perSessionClient（`mcp-session-id` 头）走 `session_pool::call_tool_isolated` → 其余 `pool::call_tool_with_meta`（`_meta` 透传上游，MRTR inputResponses/OTel 保留）。
  - **MRTR 原生映射**：上游 `_meta` 的 `resultType: "input_required"` → `CallToolResponse::InputRequired(InputRequiredResult::new(input_requests, request_state))`（rmcp 的 MRTR enum 与我们 A2 实现同构，`model::mrtr` 是私有模块但类型经 `model::*` re-export）。
  - **scope 动态解析**：不按路径每挂一个实例（scope 是通配路由），handler 内从 `context.extensions.get::<http::request::Parts>()` 读 `uri.path()` 剥 `/mcp` 前缀——P3 POC 验证的通道。
- **路由切换**（`http_server.rs::build_router`）：`/mcp` 与 `/mcp/{*path}` 改 `route_service(..., rmcp_service())`（`StreamableHttpService::new(|| Ok(HubBridge::new()), LocalSessionManager, Default::default())`，`legacy_session_mode=true` 默认）；`get`/`post` 两条 handler 挂载与 `/mcp/message`（2024 双端点 POST 端点）删除。
- **旧实现并存**：`dispatch_mcp` 全家桶（~1700 行）暂留文件内未删，文件顶部临时 `#![allow(dead_code)]` 注明 TEMP 注释——cleanup 阶段统一删除，回退窗口保留。

#### rmcp 3.4.1 实现细节踩坑（编译期逐条排掉）

- `ErrorData::new(code, message, data)` 的 message 是 `impl Into<Cow<'static, str>>`——`impl Into<String>` 泛型不传递，helper 收 `String`。
- `ListToolsResult` 等分页结果是宏生成的穷举 struct（含 `result_type/meta/next_cursor/ttl_ms/cache_scope`），用 `ListToolsResult::with_all_items(tools)` 构造。
- `CallToolResult` non-exhaustive：`CallToolResult::default()` + 字段赋值（或 `success(content)` builder）。
- `RequestMetaObject(pub MetaObject(pub JsonObject))` 双层 newtype：`m.0 .0` 拿 `JsonObject`。
- `CallToolRequestParams.arguments: Option<JsonObject>`（Map 非 Value）；`name: Cow<str>`。
- `#[tool_handler]`/`#[tool_router]` 未用——HubBridge 全手写覆写（动态工具不适用宏静态注册）。

#### 验证（全部通过）

- `cargo check` **0 错 0 警**（并存期 TEMP allow 后）；`cargo test --lib` **91 passed / 0 failed / 4 ignored**。
- 真机 E2E（`/tmp/rmcp_e2e.py`，留痕 `/tmp/rmcp_e2e_transcript.txt`）：**22/22**——
  - P0 共存（/health、/servers 原样可达）
  - P1 版本协商：2024-11-05 / 2025-03-26 / 2025-06-18 / 2025-11-25 initialize **精确回显**；未知版本 fallback 2025-11-25
  - P2 2026 modern：`server/discover` 返回全 5 版本
  - P3 全会话：session mint → notifications/initialized 202 → tools/list 聚合（102 工具、playwright 前缀、inputSchema 全带）→ tools/call **真实调用 playwright**（isError:false）→ 未知工具 -32602 → ping `{}` → DELETE 后复用 404
  - P4 2024 双端点退役：无 session GET → 400；`/mcp/message` → 422（不再有 endpoint 行为）
  - P5 2026 modern tools/call：`resultType: "complete"` 真实返回
  - P6 GET session SSE server-push 流：200 + `text/event-stream`
- **前端/既有客户端无感**：URL、认证头、scope 路径、协议版本协商行为全部不变。

#### 下一步（未做）

- 上游 transport 切 rmcp client（stdio → `transport-child-process`；http → `transport-streamable-http-client-reqwest`）；A1 modern 探测保留做回退；`sse_transport` 评估退役。
- cleanup：**已执行**（见 §3.9.5，`mcp_version.rs` 因 mcp_tasks 依赖保留）。

### 3.9.5 清理：旧 dispatch 全家桶退役（2026-09-27，已完成）

> 前提：§3.9.3/§3.9.4 全量迁移 + gap 补齐（prompts/resources/tasks/MRTR 字段/subscriptions）验证通过后，用户授权「在确保所有功能都迁移完毕的情况下，可以清理」。

#### 已删除

| 目标 | 处置 | 说明 |
| --- | --- | --- |
| `http_server.rs` dispatch 全家桶（~1220 行） | 删 | `dispatch_mcp`/`dispatch_mcp_inner`、`mcp_root_post/scope_post/message_post/root_get/scope_get/root_delete/scope_delete`、`MessageQuery`、`log_mcp_request`——全部仅旧路由引用 |
| `http_server.rs` 旧会话态 | 删 | `StrategyMap`/`SESSION_STRATEGY`/`strategy_for_session`/`remember_strategy`/`forget_strategy` + `ChannelMap`/`SSE_CHANNELS`/`sse_channels`/`register_sse_channel`/`sse_channel_for`/`drop_sse_channel`/`new_session_id`（2024 双端点 + 每会话策略机制的载体） |
| `http_server.rs` dead helpers | 删 | `jsonrpc_response`/`jsonrpc_error`/`jsonrpc_response_s`/`with_cache_hint`/`unsupported_protocol_version_error`/`extract_session_id`/`mcp_scope_servers`；TEMP `#![allow(dead_code)]` 头一并移除 |
| `mcp_tasks.rs::create_tool_task` + `snapshot_ext` | 删 | 仅旧 dispatch 调用；strategy 参数依赖随之消失；`TaskStatus::Completed` 保留（get_ext 终态分类仍读）加 `#[allow(dead_code)]` 注记 |
| `mcp/stdio_transport.rs` 旧本体（~500 行） | 删 | 文件 649→131 行，**仅保留 4 个 `pub(crate)` helpers**（`resolve_in_path`/`kill_process_tree`/`parse_progress_pct`/`looks_like_download_progress`）——被 `rmcp_stdio_transport.rs` 复用 |
| `services/request_cancel.rs` 整模块 | 删 | `notifications/cancelled` 结果级取消是旧 dispatch 的 wrapper 语义；rmcp 协议层原生处理（会话关闭时 in-flight 调用随 session 终止） |

#### 保留（评估后不动）

- `mcp_version.rs`：清理时一度保留（误判 mcp_tasks 依赖），**复查确认依赖已随 `create_tool_task` 删除而消失 → 整文件删除** + `mod.rs` 注销（`cargo test --lib` 82 passed——另 9 个测试为 mcp_version 单测随文件删除，合理减员）。
- `subscription_hub.rs` **保留**：`server_service`/`prompt_service`/`server_tool_config_service` 变更通知仍在用 `notify_*`；rmcp 原生 `notifications/*/list_changed` 接线**已补齐**（见下）。

**清理后补充：rmcp 原生 list_changed 通知接线（2026-09-27）**——`rmcp_bridge.rs` capabilities 加 `enable_tool_list_changed/prompts/resources` 三个声明；`NATIVE_PEERS` 注册表（`on_initialized` 记 `Peer<RoleServer>` handle，`fan_out_native_list_changed` 逐 peer 发原生通知、失败剪枝）+ `spawn_native_notify(kind)` 公开钩子；`subscription_hub` 三个 `notify_*_list_changed` 变更点同时驱动 ①2026 `subscriptions/listen` 自定义流 ②2025 标准客户端原生通知。验证：lib 内 in-process 集成测试（duplex 真握手 + 三类 fan-out 各收 1 次）通过，`cargo test --lib` 91 passed。测试坑：`Notify::notify_one` 许可不累积（3 连发丢 2 唤醒），断言改轮询计数器。
- `mcp::http_transport`/`sse_transport`/`openapi_transport`：上游 HTTP/SSE/OpenAPI 未 rmcp 化（A1 modern 探测定制保留）。

#### 清理后回归（全过）

- `cargo check` 0 错 0 警；`cargo test --lib` **90 passed / 0 failed**（-1：request_cancel 单测随模块删除）。
- tauri dev 重建后：E2E 22/22（`/tmp/rmcp_e2e_post_clean.txt`）；prompts/list / resources/list / tasks/list / 未知方法 -32601 全过；bearer-off initialize+tools/list（102 工具）；stdio 真机（playwright/codegraph 子进程存活 + 工具列表）。

### 3.9.6 rmcp 迁移收尾：StreamableHttp 上游切官方 client + 全量回归（2026-09-27，已完成）

> 测试报告：`doc/rmcp_migration_e2e_test_report_20260927.md`（§8 清理回归 / §9 原生通知 / §10 HTTP client / §11 全量 41 用例）。

- **新文件 `mcp/rmcp_http_transport.rs`**（`RmcpHttpTransport`，~420 行）：`ClientLifecycleMode::Auto{preferred_versions, legacy_version}`（discover 探测 10s 超时自动降级 legacy initialize）+ `RequestMetaObject`/MRTR 双路径（`call_tool` 原生重试 or `call_tool_once` 透传 inputResponses）+ tasks 轮询（`TaskPayload` match + `Task.poll_interval_ms`，`get_task`/TaskPayload 轮询至终态）+ `StreamableHttpClientTransportConfig`（`auth_header`/`custom_headers`/`allow_stateless`；#[non_exhaustive] 用 `::with_uri()` 后改字段）。
- **`pool.rs::build_client` StreamableHttp 分支切 `RmcpHttpTransport`**；SSE 分支保留手写 `SseTransport`（用户决策：rmcp 有意不提供 legacy client transport，SSE 上游客户端仍有用）+ 补 `raw_meta` 透传对齐 http/stdio；`http_transport.rs`（22KB 旧手写实现）删除（无引用）。
- **Cargo**：rmcp 加 `transport-streamable-http-client-reqwest`；reqwest 统一 0.13.5（rustls-native-certs→rustls 改名）。
- **原生 list_changed 通知**：`rmcp_bridge.rs` `NATIVE_PEERS` 注册表（on_initialized 记 Peer）+ `spawn_native_notify`；`subscription_hub` 三个 `notify_*_list_changed` 双路驱动（2026 subscriptions/listen 自定义流 + 2025 原生通知，失败剪枝）。
- **`mcp_version.rs` 删除**（依赖随 create_tool_task 消失）。
- **全量回归发现并修复 2 个真实问题**：①2026 discover capabilities 缺 tasks 扩展声明 → `get_info` 补 `ExtensionCapabilities`；②SEP-2549 CacheableResult（ttlMs/cacheScope）泄露到 legacy 会话 → `is_2026_session()` 版本门控三处 list handler。
- **验证**：`cargo check` 0 警；`cargo test --lib` 82 passed；全量协议回归 `/tmp/rmcp_full_regress.py` **41/41**（四版本 × 公网 IP 真实调用 + discover + scope + RAG builtin + 退役端点 + 错误路径）；Bearer 矩阵 5/5（测毕还原关闭）；rmcp 行为差异记录（2026 initialize 降级协商 / path-local 会话 / Mcp-Name 必头且 latin-1 限制中文工具名）。

### 3.9.2 官方 rmcp SDK 替换可行性 POC（2026-09-27，已验证）

> 用户方向：**两侧都用 rmcp**——上游连接用 rmcp client，下游 `/mcp` 接入点挂官方 rmcp server（`StreamableHttpService`），不影响 `/rest`、`/api`、`/health` 等其他接入点。POC 在 `/tmp/rmcp-poc`（rmcp 3.4.1，vendor 全依赖源码），全部实测通过。

#### 实测结论（curl 真机验证，非纸面分析）

| 验证点 | 结果 | 说明 |
| --- | --- | --- |
| P1 共存挂载 | ✅ | `route_service("/mcp", StreamableHttpService::new(...))` 直接嵌进自有 axum Router；`/health`、`/rest/test/call` 原样可达，互不影响 |
| P2 legacy 协商 | ✅ | `supported_protocol_versions()` 返回 `ProtocolVersion::KNOWN_VERSIONS` 后：2024-11-05/2025-06-18 请求**精确回显**同版本；未知版本 fallback 2025-11-25（规范行为） |
| P2 2026-07-28 initialize | ⚠️ 设计如此 | `negotiate_protocol_version` 把 ≥2026-07-28 视为 modern——**不走 initialize 协商**，fallback 返回 2025-11-25。modern 客户端走 `server/discover`（与我们 A1 上游探测逻辑同构） |
| P2 2026 modern 全链路 | ✅ | `server/discover` → `resultType:"complete"` + `supportedVersions` 全 5 版本 + `ttlMs/cacheScope`（CacheableResult 原生）+ `_meta.serverInfo`；`tools/call` → `resultType:"complete"`。**请求头要求**：`MCP-Protocol-Version: 2026-07-28` + `Mcp-Method: <method>`（+ `Mcp-Name` for tools/call）+ body `_meta` 内 `io.modelcontextprotocol/protocolVersion` + `clientCapabilities`——逐层报错提示（-32602 → -32020 逐步引导），与我们 B5 头记录/B 系列 _meta 路由语义一致 |
| P3 scope 注入 | ✅ | 手写 `call_tool`（`#[tool_handler]` 检测到手写实现即不生成默认）内 `context.extensions.get::<http::request::Parts>()` 拿到请求 path/headers → 可从路径派生 scope（`/mcp/group/...` vs `/mcp`），POC 已在响应里回注 scope 验证 |
| P4 2024-11-05 双端点 | ❌ 不支持 | 无 session 的 `GET /mcp`（2024 纯 SSE 客户端建流方式）→ **400 "Session ID is required"**。rmcp 支持 2025-03+ 的 GET server-push 流（legacy_session_mode）但不支持 2024 的 `endpoint` 事件 + `/mcp/message` 双端点 |

#### rmcp 3.4.1 关键 API（实测踩坑记录）

- `ProtocolVersion::LATEST = V_2025_11_25`（**不是 2026**）；`KNOWN_VERSIONS` 全 5 版本；`known_up_to(&max) -> Cow`。
- `ServerHandler::get_info` 返回 `ServerConfig`（=`InitializeResult` 别名；`ServerInfo` 同名别名已 **deprecated**）。构造：`ServerConfig::new(caps).with_server_info(Implementation::new(name, ver))`——struct 均 non-exhaustive，禁字面量。
- `supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]>`（**带 &self**，不是关联函数）。
- `CallToolResponse` 是 **MRTR enum**（Complete/InputRequired/Task，对应我们 A2 实现）；tool 方法返回值经 `From<CallToolResult>` 包装。`Content` 改名 **`ContentBlock`**（`model::ContentBlock::text(...)`）。
- `#[tool_router]` 在 impl 块生成**关联函数** `fn tool_router() -> ToolRouter<Self>`（非字段方法）；`#[tool_handler]` 仅在无手写 `call_tool` 时生成默认实现（`has_method` 检测）。
- `StreamableHttpService::new(service_factory, session_manager, config)`：session_manager 是 `Arc<M>` 但 M 传 `LocalSessionManager` 本体（trait 只对本体实现，**不**对 `Arc<LocalSessionManager>`）；`StreamableHttpServerConfig::default()` 即 legacy_session_mode=true。
- tool 参数 `Parameters<T>`，T 需 `schemars::JsonSchema`（schemars 1 + http 1 依赖要显式加）。

#### 对迁移设计的直接结论（2026-09-27 更新：官方说明 + 流量证据已确认）

1. **下游 `/mcp` 换 rmcp 可行**：四版本（2025-03/06/11 + 2026 modern）rmcp 原生覆盖，行为与我们手写实现语义一致（含 MRTR/resultType/CacheableResult/discover）。
2. **2024-11-05 双端点无需保留双轨**（两项证据）：
   - **官方立场**（crates.io/crates/rmcp 说明）：legacy HTTP+SSE 是**有意不提供的支持面决策**（"deliberately not supported"），已被 2025-03-26 Streamable HTTP 取代；官方替代方案=新代码用 Streamable HTTP，或对 legacy 服务器前置 Streamable HTTP 代理。rmcp 不打算重新加入旧传输。
   - **流量证据**（本机 DB 实查）：`app_log` 中全部 `transport=legacy-sse` 连接记录（23 条）均来自测试客户端（e2e-suite / sse-e2e / 单字符探针 t/1、d/1），**无任何真实外部客户端**使用 2024 双端点形态。
   - 结论：下游全切 rmcp；手写 dispatch 中的 2024 双端点（endpoint 事件 + /mcp/message）随迁移退役。若未来真出现 legacy 客户端，再按官方建议加薄代理层。
3. **上游方向同理**：rmcp client 连不上仅支持 2024 双端点的上游 MCP 服务器——现有 `sse_transport` 保留即等价于官方建议的"支持 Streamable HTTP 的代理"兼容层；若确认上游池无此类服务器也可一并退役。
4. **scope 路由**：P3 证明 context 可拿 path；每-scope 工具集动态聚合需在 `call_tool` 内按 path 查询动态工具表（rmcp 的 ToolRouter 是静态注册，动态工具需手写 `list_tools`/`call_tool` 桥接到 pool——与我们现有 dispatch 聚合逻辑对接，而非逐工具注册）。
5. **上游 client 替换**：rmcp client（transport-child-process/transport-streamable-http-client-reqwest）可替换 stdio/http transport；A1 modern 探测逻辑可保留做回退。

---

- **复核第十一轮（2026-09-30，R108-R157：4 波独立逐行扫描代理 + 亲核 diff + 第 18 套件）**：修复 16 项——①[H] `/api/*` REST 经 meta 工具绕过 bearer 服务器 allow-list（execute_openapi_impl 落 pool meta 分发 allowed=None，打破 $smart 独占）→ 补 builtin+meta 404 门控；②[M] `/rest/{server}/tools` 泄漏 meta schema、`/rest/group/{g}/tools` 不应用组 allow-list → 两 listing 面补齐（裸名 retain + fail-closed）；③[M] smart_route_call 三入口全链无 600s 超时（挂死上游锁死 per-client mutex）→ handle_call_tool 单点包 timeout_tool_call；④[M] resources/read 缺 ttlMs/cacheScope 注入（2026 缓存契约缺口）→ 2026 会话注入 30s/Private；⑤[M] tasks/list+result 对 2026 客户端恒发 2025-11 字段名 → result/list_all 加 modern 分形；⑥[M] pool 重试门槛迁移后死代码（rmcp 错误串不匹配 "child process exited"）+ connect 重入 TOCTOU（并发同名 connect 覆盖 live client 孤儿化孙进程）→ 门槛补 rmcp 错误串 + per-server 连接锁；⑦[M] 导出/复制配置丢弃 headers/proxy/openapi/options 等连接字段（导出→导入后认证静默丢失）→ 补齐 + 全量脱敏（headers/proxy 账密/openapi.security 叶子递归）；⑧[M] get_registry_server_version 未注册 invoke_handler → 补；⑨[M] i18n errors.failedToUpdateSystemConfig 仅 zh → 三语言补译；⑩[L] tasks ttl i64 负回绕、终态保留与 TTL 矛盾、content block 静默丢弃、裸名被误剥前缀、reinstalled 标记早消费、openapi 凭据入日志、导入丢 proxy/options/openapi、is_connected 驱逐注释失实。记录不修：bearer 明文落库/permissive CORS/资源 bearer allow-list（存量设计债单独排期）。新套件 `rmcp_r158_fixes.py` 25 项；新单测 6 个（ttl 语义×4 + parse_body_limit + 协议版本白名单）。**18 套 635/635 全绿；cargo test --lib 92 passed；cargo check 0 错 0 警；build ✓**。详见 `doc/rmcp_review_r108_r157_20260930.md`。

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
| **当前已同步到 origin commit** | `85a530f`（origin/main，= v1.0.40 tag 后 4 个未发布提交） |
| **对应 origin tag** | `v1.0.40` |
| **桌面端版本号** | `1.0.40003` |
| **同步执行日期** | 2026-09-25 |

> 下次同步以 `85a530f` 为基线起点（`git log --oneline 85a530f..HEAD`）。

### 4.5 同步记录

#### 2026-09-25：`f8615ab` -> `85a530f`（6 commit，v1.0.40 后未发布提交）

**前端/locales 已同步**：
- `f6e4cc8` #1210 env 变量遮蔽警示：`SettingsContext.SmartRoutingConfig` 加 `envOverriddenFields`（只读元数据，保存不回传）；`SettingsPage` 新增 `renderEnvOverrideWarning()` 并挂到 openai/azure 两分支 5 个输入框（llmProviderApiKey/BaseUrl、embeddingModel、azureOpenaiEndpoint/ApiKey）；locales +1 键 ×4。
- `7b2822d` #1212 嵌入任务前缀：`SmartRoutingConfig` 加 `embeddingQueryPrefix`/`embeddingDocumentPrefix`；temp state/初始化/`handleSmartRoutingConfigChange` 键类型/两条保存路径（`handleSmartRoutingConfigChange` 批量 + `handleSaveSmartRoutingConfig`）同步；embeddingMaxTokens 与 progressiveDisclosure 之间新增「嵌入任务前缀」输入块（query/document 两输入框 + env 遮蔽警示）；locales +5 键 ×4（含补齐 `noChanges`——桌面代码此前已调用、缺键走 fallback）。
- `815c91a` #1211 按钮指针光标：`index.css` 顶部 `@layer base` 全局 `button:not(:disabled)/[role='button']` cursor:pointer。

**评估无需同步/镜像**：`f49ba5b`/`10736a2` #1213/`85a530f` #1214 纯 docs/CI（docs/ 不同步）；#1210/#1212 后端（smartRouting env 解析、vectorSearch 前缀应用、serverController 校验）——Smart Routing 未实现，无 Rust 落点；新配置键经 `config_service::update` JSON 深合并透明 round-trip。

- 版本 `1.0.40001 → 1.0.40002`（四源 + Cargo.lock）；changelog **合并为 `doc/upgrade/1.0.40003.md`**（原 1.0.40002.md 已并入删除）。
- **影响功能点**：设置页 Smart Routing 区块（桌面 Tauri 运行时隐藏，仅 web dev 可见）新增嵌入任务前缀输入块与 env 遮蔽警示（桌面后端不计算 `envOverriddenFields`，警示恒不显示，行为与 origin 部署一致需 origin 后端）；web 模式按钮 hover 指针光标；locales settings 段 +6 键 ×4。**结果**：桌面端运行行为不变（Smart Routing 未实现、隐藏区块代码保持与 origin 对齐）；新配置键 `embeddingQueryPrefix`/`embeddingDocumentPrefix`/`envOverriddenFields` 可透传存储。tsc 19 = 基线 19（0 新增）；`npm run build` 通过。用户无需操作。

#### 2026-09-24（第二轮）：`8ed6478` -> `f8615ab`（2 commit，v1.0.40 + 1 未发布）

- `984028e` #1205/#1206：pg 连接池加固 —— **无需镜像**（桌面 sqlx SQLite 本地，无 pg-pool）。
- `f8615ab` #1209：Smart Routing 配置键白名单 —— **无需镜像**（Smart Routing 未实现；`config_service::update` JSON 深合并透明 round-trip）。
- 无 frontend/locales/Rust 改动；版本 `1.0.39001 → 1.0.40001`；changelog **合并为 `doc/upgrade/1.0.40003.md`**（原 1.0.40002.md 已并入删除）。影响功能点：无。

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

- 版本 `1.0.35003 → 1.0.39001`；changelog **合并为 `doc/upgrade/1.0.40003.md`**（单文件，含 1.0.40002 历史内容）。
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

### 已完成（节选，全量见 [agent_20260924.md](doc/agent_20260924.md) §7）

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
