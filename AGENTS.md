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
