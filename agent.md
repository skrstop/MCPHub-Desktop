# MCPHub Desktop (Tauri) — Agent 开发文档

> 本文档是 Tauri 桌面客户端迁移的**完整参考**，供 AI Agent 和开发者续接工作使用。
> 包含：原项目架构、桌面端架构、已完成内容、待办事项及所有关键技术细节。

> ⚠️ **核心约束（MUST FOLLOW）**：**禁止修改 `mcphub/frontend/`、`mcphub/src/` 等原始源文件**。
> 所有修改必须在 `tauri/` 目录内进行。前端代码需先拷贝到 `tauri/frontend/`，再在副本上修改。目的：保留原始代码可供问题溯源，桌面端改动与 Web 端完全隔离。
> 做任何较大修改后，必须更新agent.md文档，用来记录。目的：为了方便后续维护和理解项目结构。
---

## 1. 项目概览

### 1.1 原项目（mcphub — Node.js/Express + React/Vite）

| 属性 | 值 |
|------|-----|
| 包名 | `@samanhappy/mcphub` |
| 技术栈 | Express.js + TypeScript ESM + React/Vite + Tailwind CSS |
| 入口 | `src/index.ts` → `src/server.ts` |
| 前端 | `frontend/` (React + Vite，端口 5173，代理到后端 3000) |
| 认证 | JWT + bcrypt，admin 用户由 `ADMIN_PASSWORD` 环境变量控制 |
| 数据存储 | JSON 文件 (`mcp_settings.json`) 或 PostgreSQL (USE_DB=true) |
| MCP 连接 | `src/services/mcpService.ts` 管理所有 MCP 服务端连接 |
| 路由 | `/mcp/{group\|server}`、`/mcp/$smart`、REST API `/api/*` |
| i18n | react-i18next，翻译文件在 `locales/` |
| 部署 | Docker 多阶段构建 + NPM 包 CLI (`bin/cli.js`) |

#### 原项目目录结构

```
mcphub/
├── src/
│   ├── index.ts              # 应用入口
│   ├── server.ts             # Express 服务器设置
│   ├── betterAuth.ts         # OAuth/Better-Auth 集成
│   ├── controllers/          # HTTP 请求处理器
│   ├── routes/               # 路由定义
│   ├── services/
│   │   └── mcpService.ts     # 核心 MCP 服务器管理逻辑
│   ├── dao/                  # 数据访问层（JSON 文件 + PostgreSQL 双实现）
│   ├── db/                   # TypeORM 实体 & Repository（PostgreSQL 模式）
│   ├── middlewares/          # Express 中间件
│   ├── config/               # 配置管理
│   ├── types/                # TypeScript 类型定义 & DTO
│   └── utils/                # 工具函数
├── frontend/
│   ├── src/
│   │   ├── pages/            # 页面组件（11个页面）
│   │   │   ├── Dashboard.tsx
│   │   │   ├── ServersPage.tsx
│   │   │   ├── GroupsPage.tsx
│   │   │   ├── UsersPage.tsx
│   │   │   ├── SettingsPage.tsx
│   │   │   ├── LogsPage.tsx
│   │   │   ├── ActivityPage.tsx
│   │   │   ├── LoginPage.tsx
│   │   │   ├── MarketPage.tsx
│   │   │   ├── PromptsPage.tsx
│   │   │   └── ResourcesPage.tsx
│   │   ├── components/       # 可复用 UI 组件
│   │   ├── utils/
│   │   │   ├── fetchInterceptor.ts   # HTTP 请求拦截器（需替换为 Tauri invoke）
│   │   │   ├── runtime.ts            # 运行时配置（basePath、apiUrl）
│   │   │   └── api.ts                # API 工具函数
│   │   ├── services/         # 前端服务层
│   │   ├── hooks/            # React hooks
│   │   └── contexts/         # React contexts
│   └── dist/                 # Vite 构建输出（占位文件已创建）
├── locales/                  # i18n 翻译（en/zh/fr/tr）
├── mcp_settings.json         # MCP 服务器定义 + 用户账户（原项目配置）
├── package.json              # 根包（pnpm workspace）
└── tauri/                    # ← 桌面端代码（本文档范围）
```

#### 原项目数据模型（DAO 层）

| 模型 | DAO 接口 | DB 实体 | JSON 路径 |
|------|----------|---------|-----------|
| IUser | UserDao | User | settings.users[] |
| ServerConfig | ServerDao | Server | settings.mcpServers{} |
| IGroup | GroupDao | Group | settings.groups[] |
| SystemConfig | SystemConfigDao | SystemConfig | settings.systemConfig |
| BearerKey | BearerKeyDao | BearerKey | settings.bearerKeys[] |
| IOAuthClient | OAuthClientDao | OAuthClient | settings.oauthClients[] |
| BuiltinPrompt | BuiltinPromptDao | BuiltinPrompt | settings.prompts[] |
| BuiltinResource | BuiltinResourceDao | BuiltinResource | settings.resources[] |

---

### 1.2 桌面端项目（tauri/ — Rust/Tauri 2 + 复用原 React 前端）

| 属性 | 值 |
|------|-----|
| 位置 | `tauri/` 子目录（不在项目根目录） |
| Tauri 版本 | v2 |
| Rust crate | `tauri/src-tauri/` |
| 包名(Rust) | `mcphub`，lib crate: `mcphub_lib` |
| 前端复用 | `frontend/dist/`（原项目 React/Vite 构建产物） |
| 数据存储 | SQLite（`$APPDATA/mcphub.db`，通过 sqlx 0.8） |
| 认证 | jsonwebtoken 9 + bcrypt 0.15，密钥存 OS 钥匙串(keyring 3) |
| 异步运行时 | tokio 1 full |
| HTTP 客户端 | reqwest 0.12 (rustls-tls + stream + json) |
| 应用标识 | `app.mcphub.desktop` |

---

## 2. 桌面端架构

### 2.1 目录结构

```
tauri/
├── package.json              # @tauri-apps/cli devDependency，npm install 已完成
├── frontend/                 # ⚠️ 原 frontend/ 的副本（Phase 6 创建），所有前端修改在此进行
│   ├── package.json          # 继承自原 frontend/package.json，新增 @tauri-apps/api
│   ├── vite.config.ts        # 继承自原 frontend/vite.config.ts
│   ├── src/
│   │   └── utils/
│   │       ├── tauriClient.ts        # 新建：isTauri() 检测 + invoke() 封装
│   │       └── fetchInterceptor.ts   # 修改：拦截 /api/ 请求转为 invoke()
│   └── dist/                 # Vite 构建输出（tauri.conf.json 指向此处）
├── src-tauri/
│   ├── Cargo.toml            # Rust 包清单（所有依赖）
│   ├── Cargo.lock            # 锁定依赖版本
│   ├── build.rs              # tauri-build 构建脚本
│   ├── tauri.conf.json       # Tauri 应用配置
│   ├── .cargo/
│   │   └── config.toml       # sparse 注册表配置（解决 GitHub 访问问题）
│   ├── icons/                # 应用图标（占位 PNG/ICO/ICNS，需替换为真实图标）
│   │   ├── 32x32.png
│   │   ├── 128x128.png
│   │   ├── 128x128@2x.png
│   │   ├── icon.png
│   │   ├── icon.icns
│   │   └── icon.ico
│   ├── migrations/
│   │   └── 0001_initial.sql  # SQLite 完整 schema
│   └── src/
│       ├── main.rs           # Rust 二进制入口（调用 lib::run()）
│       ├── lib.rs            # 应用核心：插件注册、setup hook、invoke_handler
│       ├── auth/
│       │   └── mod.rs        # JWT 签发/验证 + bcrypt 密码哈希
│       ├── db/
│       │   └── mod.rs        # SQLite 连接池初始化（OnceLock<SqlitePool>）
│       ├── models/
│       │   ├── mod.rs
│       │   ├── server.rs     # ServerType, ServerConfig, ServerStatus, Tool, ServerInfo
│       │   ├── user.rs       # User, UserRole, UserInfo, UserPayload
│       │   ├── group.rs      # Group, GroupPayload
│       │   ├── config.rs     # SystemConfig
│       │   ├── auth.rs       # LoginRequest, AuthToken, Claims
│       │   └── log.rs        # LogEntry, ActivityEntry, LogQuery
│       ├── mcp/
│       │   ├── mod.rs
│       │   ├── client.rs     # McpTransport trait + McpClient 封装
│       │   ├── stdio_transport.rs  # 子进程 JSON-RPC over stdin/stdout
│       │   ├── sse_transport.rs    # HTTP SSE 传输（含自动重连）
│       │   ├── http_transport.rs   # Streamable HTTP POST 传输
│       │   └── pool.rs       # 全局连接池（OnceLock<Arc<RwLock<HashMap>>>）
│       ├── services/
│       │   ├── mod.rs
│       │   ├── mcp_manager.rs      # 启动时连接所有 enabled server
│       │   ├── server_service.rs   # CRUD for servers (SQLite)
│       │   ├── user_service.rs     # CRUD for users + ensure_default_admin
│       │   ├── group_service.rs    # CRUD for groups
│       │   ├── config_service.rs   # 读写 system_config
│       │   ├── log_service.rs      # 写入/查询 app_log & activity_log
│       │   └── settings_import.rs  # 从 mcp_settings.json 导入服务器+用户
│       └── commands/
│           ├── mod.rs
│           ├── auth.rs       # login, logout, get_current_user, change_password
│           ├── servers.rs    # list_servers, get_server, add_server, update_server, delete_server, toggle_server, reload_server
│           ├── groups.rs     # list_groups, add_group, update_group, delete_group
│           ├── tools.rs      # list_tools, call_tool
│           ├── users.rs      # list_users, add_user, update_user, delete_user
│           ├── config.rs     # get_system_config, update_system_config, import_settings, export_settings
│           └── logs.rs       # get_logs, get_activity_logs
```

### 2.2 数据流架构

```
React Frontend (frontend/dist/)
        │
        │  window.__TAURI__.invoke("command_name", args)
        │  (替代原来的 fetch("/api/..."))
        ▼
Tauri IPC Bridge
        │
        ▼
commands/ (Tauri commands = 原 controllers/)
        │
        ▼
services/ (业务逻辑 = 原 services/)
        │
        ├─▶ db/ (SQLite via sqlx = 原 dao/ + TypeORM)
        └─▶ mcp/ (MCP 连接池 = 原 mcpService.ts)
                │
                ├─▶ stdio_transport (子进程)
                ├─▶ sse_transport (HTTP SSE)
                └─▶ http_transport (Streamable HTTP)
```

### 2.3 SQLite Schema（`migrations/0001_initial.sql`）

| 表名 | 说明 |
|------|------|
| `users` | 用户账户（id TEXT PK, username UNIQUE, password_hash, role） |
| `servers` | MCP 服务器配置（id TEXT PK, name UNIQUE, server_type, command/args/env/url/headers/options JSON） |
| `groups` | 服务器分组（id TEXT PK, name UNIQUE, servers TEXT = JSON数组） |
| `system_config` | 单行系统配置（id=1, proxy, registry, log_level, expose_http, http_port） |
| `bearer_keys` | Bearer 认证密钥 |
| `activity_log` | 用户操作日志（user_id, action, resource, detail JSON） |
| `app_log` | 应用日志（level, message, server_name） |
| `builtin_prompts` | 内置 Prompt |
| `builtin_resources` | 内置 Resource |

### 2.4 Tauri Commands 映射表（原 REST API → Tauri invoke）

| 原 REST API | Tauri Command | 文件 |
|-------------|---------------|------|
| POST /api/auth/login | `login` | commands/auth.rs |
| POST /api/auth/logout | `logout` | commands/auth.rs |
| GET /api/auth/me | `get_current_user` | commands/auth.rs |
| PUT /api/auth/password | `change_password` | commands/auth.rs |
| GET /api/servers | `list_servers` | commands/servers.rs |
| GET /api/servers/:name | `get_server` | commands/servers.rs |
| POST /api/servers | `add_server` | commands/servers.rs |
| PUT /api/servers/:name | `update_server` | commands/servers.rs |
| DELETE /api/servers/:name | `delete_server` | commands/servers.rs |
| PUT /api/servers/:name/toggle | `toggle_server` | commands/servers.rs |
| POST /api/servers/:name/reload | `reload_server` | commands/servers.rs |
| GET /api/groups | `list_groups` | commands/groups.rs |
| POST /api/groups | `add_group` | commands/groups.rs |
| PUT /api/groups/:id | `update_group` | commands/groups.rs |
| DELETE /api/groups/:id | `delete_group` | commands/groups.rs |
| GET /api/tools | `list_tools` | commands/tools.rs |
| POST /api/tools/call | `call_tool` | commands/tools.rs |
| GET /api/users | `list_users` | commands/users.rs |
| POST /api/users | `add_user` | commands/users.rs |
| PUT /api/users/:id | `update_user` | commands/users.rs |
| DELETE /api/users/:id | `delete_user` | commands/users.rs |
| GET /api/config | `get_system_config` | commands/config.rs |
| PUT /api/config | `update_system_config` | commands/config.rs |
| POST /api/config/import | `import_settings` | commands/config.rs |
| GET /api/config/export | `export_settings` | commands/config.rs |
| GET /api/logs | `get_logs` | commands/logs.rs |
| GET /api/logs/activity | `get_activity_logs` | commands/logs.rs |

---

## 3. 关键依赖与版本

### Rust 依赖（Cargo.toml）

```toml
tauri = { version = "2", features = ["tray-icon", "image-ico", "image-png"] }
tauri-plugin-shell = "2"
tauri-plugin-fs = "2"
tauri-plugin-dialog = "2"
tauri-plugin-notification = "2"
tauri-plugin-autostart = "2"
tauri-plugin-updater = "2"
tauri-plugin-process = "2"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
reqwest = { version = "0.12", features = ["json","stream","rustls-tls"] }
sqlx = { version = "0.8", features = ["runtime-tokio","sqlite","macros","migrate","uuid","chrono"] }
jsonwebtoken = "9"
bcrypt = "0.15"
uuid = { version = "1", features = ["v4"] }
chrono = { version = "0.4", features = ["serde"] }
keyring = { version = "3", features = ["apple-native","windows-native","sync-secret-service"] }
anyhow = "1"
thiserror = "1"
async-trait = "0.1"
log = "0.4"
env_logger = "0.11"
```

### 工具链

| 工具 | 版本 |
|------|------|
| Rust | 1.83.0（`asdf set rust 1.83.0`） |
| Node.js | v22.12.0 |
| pnpm | 8.15.4 |
| Tauri CLI | ^2（安装在 `tauri/node_modules`） |

---

## 4. 开发环境配置

### 4.1 关键注意事项

```bash
# ⚠️ 必须使用 sparse 协议运行 cargo（绕过 GitHub git 访问限制）
cd tauri/src-tauri
CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse cargo check

# .cargo/config.toml 已配置（无需手动设置环境变量时也生效）
# tauri/src-tauri/.cargo/config.toml:
# [registries.crates-io]
# protocol = "sparse"
```

### 4.2 sqlx 使用规则（重要）

```rust
// ✅ 正确：使用 sqlx::query() 非宏 API
use sqlx::Row;
let rows = sqlx::query("SELECT id, name FROM servers")
    .fetch_all(db::pool())
    .await?;
let id: String = rows[0].try_get("id")?;

// ❌ 禁止：sqlx::query!() 宏（需要 DATABASE_URL 编译时检查，桌面应用无法提供）
let rows = sqlx::query!("SELECT id FROM servers").fetch_all(pool).await?;

// ✅ 例外：sqlx::migrate!() 是嵌入文件宏，不需要 DATABASE_URL，可以使用
sqlx::migrate!("./migrations").run(&pool).await?;
```

### 4.3 开发命令

```bash
# 安装 Tauri CLI
cd tauri && npm install

# 开发模式（启动 frontend dev server + Tauri 窗口）
cd tauri && npm run dev
# 等价于：tauri dev（会先执行 beforeDevCommand: cd .. && pnpm frontend:dev）

# 生产构建
cd tauri && npm run build
# 等价于：tauri build（会先执行 beforeBuildCommand: cd .. && pnpm frontend:build）

# 仅检查 Rust 编译
cd tauri/src-tauri && CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse cargo check

# 仅检查 Rust 编译并查看所有警告
cd tauri/src-tauri && CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse cargo check 2>&1
```

---

## 5. 已完成工作（✅）

### Phase 1 — 项目骨架
- [x] `tauri/src-tauri/Cargo.toml` — 完整依赖清单
- [x] `tauri/src-tauri/build.rs` — tauri-build 构建脚本
- [x] `tauri/src-tauri/tauri.conf.json` — Tauri 应用配置
- [x] `tauri/src-tauri/src/main.rs` / `lib.rs` — 应用核心
- [x] `tauri/src-tauri/.cargo/config.toml` — sparse 注册表配置

### Phase 2 — 数据模型
- [x] `models/server.rs` / `user.rs` / `group.rs` / `config.rs` / `auth.rs` / `log.rs`

### Phase 3 — 数据库层
- [x] `migrations/0001_initial.sql` — SQLite schema（users, servers, groups, system_config, bearer_keys, activity_log, app_log, builtin_prompts, builtin_resources）
- [x] `db/mod.rs` — 连接池 + 自动迁移

### Phase 4 — MCP 传输层 & 核心服务
- [x] `mcp/` — McpTransport trait, StdioTransport, SseTransport, HttpTransport, Pool
- [x] `services/mcp_manager.rs` / `server_service.rs` / `user_service.rs` / `group_service.rs` / `config_service.rs` / `log_service.rs` / `settings_import.rs`
- [x] `auth/mod.rs` — JWT + bcrypt

### Phase 5 — Tauri Commands（核心）
- [x] `commands/auth.rs` — login/register/logout/get_current_user/change_password
- [x] `commands/servers.rs` — list/get/add/update/delete/toggle/reload
- [x] `commands/groups.rs` — list/add/update/delete
- [x] `commands/tools.rs` — list_tools/call_tool
- [x] `commands/users.rs` — list/add/update/delete
- [x] `commands/config.rs` — get/update system_config, import/export settings
- [x] `commands/logs.rs` — get_logs/get_activity_logs/clear_logs

### Phase 6 — 前端适配器
- [x] `tauri/frontend/` — 原 frontend/ 的副本（完整 React 前端）
- [x] `tauri/frontend/src/utils/tauriClient.ts` — 全量 REST→invoke 路由映射（mapRestToCommand + invokeMapped + transformTauriResponse）
- [x] `tauri/frontend/src/utils/fetchInterceptor.ts` — apiRequest() 已集成 isTauri() 检测，自动路由到 invoke()

### Phase 7 — 系统托盘
- [x] `lib.rs` — 托盘图标 + Show/Quit 菜单
- [x] 关闭窗口最小化到托盘（CloseRequested → window.hide()）

---

## 6. 待完成功能实现计划

> **原则**：除 PostgreSQL 外，所有原项目功能均需在桌面端实现。SQLite 是唯一数据存储。
> **约束**：所有修改仅在 `tauri/` 目录内。非宏 sqlx API（见第 4.2 节）。

### 当前存根（`__stub__`）清单

以下路由在 `tauriClient.ts` 中返回静态存根，需替换为真实 Rust 命令：

| 前端路由 | 当前存根状态 | 目标 Tauri 命令 |
|---------|------------|----------------|
| GET /auth/keys | 返回空数组 | `list_bearer_keys` |
| POST/PUT/DELETE /auth/keys | 返回不支持错误 | `create/update/delete_bearer_key` |
| GET /prompts | 返回空数组 | `list_builtin_prompts` |
| POST/PUT/DELETE /prompts | 返回不支持错误 | `create/update/delete_builtin_prompt` |
| POST /prompts/call | 返回不支持错误 | `call_prompt` |
| GET /resources | 返回空数组 | `list_builtin_resources` |
| POST/PUT/DELETE /resources | 返回不支持错误 | `create/update/delete_builtin_resource` |
| GET /activities | 返回空+available:false | `get_tool_activities` |
| GET /activities/available | 返回 available:false | `get_activity_available` |
| GET /market/servers | 返回空数组 | `list_market_servers` |
| GET /registry/servers | 返回空数组 | `list_registry_servers` |
| GET /cloud/servers | 返回空数组 | `list_cloud_servers` |
| GET/POST /templates | 返回不支持错误 | `list/export/import_template` |
| POST /mcpb | 返回不支持错误 | `install_mcpb` |
| servers/:n/tools/:t/toggle | 返回 success:true 存根 | `toggle_server_tool` |

---

### Phase A — Schema 修正迁移（所有后续 Phase 的前提）

**文件**：`src-tauri/migrations/0002_schema_fix.sql`

**需要修正的问题**：

| 表 | 问题 | 修正方案 |
|----|------|---------|
| `bearer_keys` | 当前是 user_id/key_hash 结构，与原项目不符 | DROP + CREATE 新结构 |
| `builtin_prompts` | 缺少 `title TEXT` 和 `template TEXT` | ALTER TABLE ADD COLUMN |
| `builtin_resources` | 缺少 `content TEXT` | ALTER TABLE ADD COLUMN |
| `activity_log` | 通用操作日志结构，不适合 tool call 监控 | DROP + CREATE 新结构 |
| `system_config` | 缺少 mcpRouter 配置字段 | ALTER TABLE ADD COLUMN |

**新结构定义**：

```sql
-- 0002_schema_fix.sql

-- 1. 重建 bearer_keys 表（与原项目一致）
DROP TABLE IF EXISTS bearer_keys;
CREATE TABLE IF NOT EXISTS bearer_keys (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    token           TEXT NOT NULL UNIQUE,
    enabled         INTEGER NOT NULL DEFAULT 1,
    access_type     TEXT NOT NULL DEFAULT 'all', -- all | groups | servers | custom
    allowed_groups  TEXT NOT NULL DEFAULT '[]',  -- JSON 数组
    allowed_servers TEXT NOT NULL DEFAULT '[]',  -- JSON 数组
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

-- 2. 修复 builtin_prompts（补充缺失字段）
ALTER TABLE builtin_prompts ADD COLUMN title TEXT;
ALTER TABLE builtin_prompts ADD COLUMN template TEXT NOT NULL DEFAULT '';

-- 3. 修复 builtin_resources（补充缺失字段）
ALTER TABLE builtin_resources ADD COLUMN content TEXT NOT NULL DEFAULT '';

-- 4. 重建 activity_log（tool call 监控，对应原 ActivityLoggingService）
DROP TABLE IF EXISTS activity_log;
CREATE TABLE IF NOT EXISTS activity_log (
    id            TEXT PRIMARY KEY,
    timestamp     TEXT NOT NULL DEFAULT (datetime('now')),
    server        TEXT NOT NULL,
    tool          TEXT NOT NULL,
    duration_ms   INTEGER,
    status        TEXT NOT NULL DEFAULT 'success', -- success | error
    input         TEXT,   -- JSON string
    output        TEXT,   -- JSON string
    group_name    TEXT,
    key_id        TEXT,
    key_name      TEXT,
    error_message TEXT
);

-- 5. 扩展 system_config（MCPRouter 集成）
ALTER TABLE system_config ADD COLUMN mcprouter_api_key TEXT;
ALTER TABLE system_config ADD COLUMN mcprouter_base_url TEXT;

-- 6. 新增 templates 表
CREATE TABLE IF NOT EXISTS templates (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    description TEXT,
    content     TEXT NOT NULL,  -- JSON 序列化的 ConfigTemplate
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- 7. 新增 server_tool_config 表（per-server 工具开关/描述覆盖）
CREATE TABLE IF NOT EXISTS server_tool_config (
    id          TEXT PRIMARY KEY,
    server_name TEXT NOT NULL,
    item_type   TEXT NOT NULL DEFAULT 'tool', -- tool | prompt | resource
    item_name   TEXT NOT NULL,
    enabled     INTEGER NOT NULL DEFAULT 1,
    description TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(server_name, item_type, item_name)
);
```

---

### Phase B — Bearer Keys（API 访问控制）

**目标**：允许外部工具/脚本通过 Bearer token 访问 MCP 服务（当 HTTP 暴露启用时）。

**涉及文件**：

| 文件 | 操作 | 说明 |
|------|------|------|
| `models/bearer_key.rs` | 新建 | `BearerKey`, `BearerKeyPayload` 结构体 |
| `services/bearer_key_service.rs` | 新建 | CRUD：list_all / create / update / delete / find_by_token |
| `commands/bearer_keys.rs` | 新建 | `list_bearer_keys`, `create_bearer_key`, `update_bearer_key`, `delete_bearer_key` |
| `models/mod.rs` | 修改 | 导出 bearer_key 模块 |
| `services/mod.rs` | 修改 | 导出 bearer_key_service |
| `commands/mod.rs` | 修改 | 导出 bearer_keys |
| `lib.rs` | 修改 | 注册 4 个命令 |
| `tauriClient.ts` | 修改 | 替换 `/auth/keys` 存根为真实命令 |

**BearerKey 数据模型**：
```rust
pub struct BearerKey {
    pub id: String,
    pub name: String,
    pub token: String,
    pub enabled: bool,
    pub access_type: String,       // "all" | "groups" | "servers" | "custom"
    pub allowed_groups: Vec<String>,
    pub allowed_servers: Vec<String>,
    pub created_at: String,
}
```

**命令签名**：
```rust
list_bearer_keys() -> Result<Vec<BearerKey>, String>
create_bearer_key(payload: BearerKeyPayload) -> Result<BearerKey, String>
update_bearer_key(id: String, payload: BearerKeyPayload) -> Result<BearerKey, String>
delete_bearer_key(id: String) -> Result<(), String>
```

**tauriClient.ts 映射**：
```typescript
if (segs[0] === 'auth' && segs[1] === 'keys') {
  if (m === 'GET') return { command: 'list_bearer_keys', args: {} };
  if (m === 'POST') return { command: 'create_bearer_key', args: { payload: body } };
  if (m === 'PUT') return { command: 'update_bearer_key', args: { id: segs[2], payload: body } };
  if (m === 'DELETE') return { command: 'delete_bearer_key', args: { id: segs[2] } };
}
```

---

### Phase C — Builtin Prompts（内置提示词管理）

**目标**：用户可以创建/管理内置提示词模板，供 MCP 客户端调用。

**涉及文件**：

| 文件 | 操作 | 说明 |
|------|------|------|
| `models/prompt.rs` | 新建 | `BuiltinPrompt`, `BuiltinPromptPayload`, `PromptArgument` |
| `services/prompt_service.rs` | 新建 | CRUD + call（渲染模板） |
| `commands/prompts.rs` | 新建 | 6 个命令 |
| `lib.rs` | 修改 | 注册命令 |
| `tauriClient.ts` | 修改 | 替换 `/prompts` 存根 |

**BuiltinPrompt 数据模型**：
```rust
pub struct BuiltinPrompt {
    pub id: String,
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub template: String,
    pub arguments: Vec<PromptArgument>,  // JSON 序列化存储
    pub enabled: bool,
    pub created_at: String,
}
pub struct PromptArgument {
    pub name: String,
    pub description: Option<String>,
    pub required: bool,
}
```

**命令签名**：
```rust
list_builtin_prompts() -> Result<Vec<BuiltinPrompt>, String>
get_builtin_prompt(id: String) -> Result<Option<BuiltinPrompt>, String>
create_builtin_prompt(payload: BuiltinPromptPayload) -> Result<BuiltinPrompt, String>
update_builtin_prompt(id: String, payload: BuiltinPromptPayload) -> Result<BuiltinPrompt, String>
delete_builtin_prompt(id: String) -> Result<(), String>
call_builtin_prompt(id: String, args: serde_json::Value) -> Result<String, String>
```

---

### Phase D — Builtin Resources（内置资源管理）

**目标**：用户可以定义静态资源，供 MCP 客户端通过 URI 读取。

**涉及文件**：

| 文件 | 操作 | 说明 |
|------|------|------|
| `models/resource.rs` | 新建 | `BuiltinResource`, `BuiltinResourcePayload` |
| `services/resource_service.rs` | 新建 | CRUD |
| `commands/resources.rs` | 新建 | 5 个命令 |
| `lib.rs` | 修改 | 注册命令 |
| `tauriClient.ts` | 修改 | 替换 `/resources` 存根 |

**BuiltinResource 数据模型**：
```rust
pub struct BuiltinResource {
    pub id: String,
    pub uri: String,
    pub name: String,
    pub description: Option<String>,
    pub mime_type: String,
    pub content: String,
    pub enabled: bool,
    pub created_at: String,
}
```

---

### Phase E — Activity Log（工具调用监控）

**目标**：记录每次 `call_tool` 的调用信息（耗时、状态、输入/输出），支持 ActivityPage 展示。

**涉及文件**：

| 文件 | 操作 | 说明 |
|------|------|------|
| `models/log.rs` | 修改 | 更新 `ActivityEntry` 字段（server, tool, duration_ms, status, input, output）|
| `services/log_service.rs` | 修改 | 新增 `write_activity()` 函数 |
| `commands/tools.rs` | 修改 | call_tool 执行后异步写入 activity_log |
| `commands/logs.rs` | 修改 | get_activity_logs 支持完整过滤（server, tool, status, 时间范围，分页）|
| `tauriClient.ts` | 修改 | 替换 `/activities` 存根：available:true，真实数据 |

**新增 tauriClient 命令映射**：
```typescript
if (p === 'activities/available') return { command: 'get_activity_available', args: {} };
if (p === 'activities' && m === 'GET') return { command: 'get_tool_activities', args: { ...queryParams } };
if (p === 'activities' && m === 'DELETE') return { command: 'clear_activities', args: {} };
```

**ActivityEntry 数据结构**（对应原 `IActivity`）：
```rust
pub struct ActivityEntry {
    pub id: String,
    pub timestamp: String,
    pub server: String,
    pub tool: String,
    pub duration_ms: Option<i64>,
    pub status: String,  // "success" | "error"
    pub input: Option<String>,
    pub output: Option<String>,
    pub group_name: Option<String>,
    pub key_id: Option<String>,
    pub key_name: Option<String>,
    pub error_message: Option<String>,
}
```

---

### Phase F — Market（本地 MCP 市场）

**目标**：读取打包的 `servers.json` 展示 MCP 服务市场，支持一键安装到服务器列表。

**涉及文件**：

| 文件 | 操作 | 说明 |
|------|------|------|
| `tauri.conf.json` | 修改 | 将 `servers.json` 加入 `resources` 数组 |
| `models/market.rs` | 新建 | `MarketServer` 结构体 |
| `services/market_service.rs` | 新建 | 读取 servers.json（从 app resources 或文件系统）|
| `commands/market.rs` | 新建 | 6 个命令 |
| `lib.rs` | 修改 | 注册命令 |
| `tauriClient.ts` | 修改 | 替换 `/market` 存根 |

**servers.json 读取路径**（优先级）：
1. `app.path().resource_dir() / "servers.json"` — 打包时内嵌
2. 应用工作目录下的 `servers.json` — 开发模式 fallback

**命令签名**：
```rust
list_market_servers(app: AppHandle) -> Result<Vec<MarketServer>, String>
get_market_server(app: AppHandle, name: String) -> Result<Option<MarketServer>, String>
search_market_servers(app: AppHandle, query: String) -> Result<Vec<MarketServer>, String>
get_market_categories(app: AppHandle) -> Result<Vec<String>, String>
get_market_tags(app: AppHandle) -> Result<Vec<String>, String>
// 一键安装：创建 ServerConfig 并加入数据库
install_market_server(app: AppHandle, name: String) -> Result<ServerConfig, String>
```

---

### Phase G — Registry Proxy（官方 MCP 注册表代理）

**目标**：代理对 `https://registry.modelcontextprotocol.io/v0.1` 的请求，避免前端 CORS 问题。

**涉及文件**：

| 文件 | 操作 | 说明 |
|------|------|------|
| `commands/registry.rs` | 新建 | HTTP 请求转发 |
| `lib.rs` | 修改 | 注册命令 |
| `tauriClient.ts` | 修改 | 替换 `/registry` 存根 |

**命令签名**：
```rust
// cursor: 分页游标，limit: 每页数量，search: 关键词
list_registry_servers(cursor: Option<String>, limit: Option<u32>, search: Option<String>)
  -> Result<serde_json::Value, String>
get_registry_server_versions(server_name: String) -> Result<serde_json::Value, String>
```

**实现说明**：使用 `reqwest` 直接 GET `https://registry.modelcontextprotocol.io/v0.1/servers`，将响应透传给前端。Tauri 应用有完整网络访问权，无需额外配置。

---

### Phase H — Cloud Proxy（MCPRouter 云服务代理）

**目标**：对接 MCPRouter API，支持云端 MCP 服务器的浏览和工具调用。

**涉及文件**：

| 文件 | 操作 | 说明 |
|------|------|------|
| `models/cloud.rs` | 新建 | `CloudServer`, `CloudTool` |
| `services/cloud_service.rs` | 新建 | MCPRouter API 调用逻辑 |
| `commands/cloud.rs` | 新建 | 云服务命令 |
| `lib.rs` | 修改 | 注册命令 |
| `tauriClient.ts` | 修改 | 替换 `/cloud` 存根 |

**MCPRouter API base URL**：`https://api.mcprouter.to/v1`（从 system_config.mcprouter_base_url 覆盖）

**命令签名**：
```rust
list_cloud_servers() -> Result<Vec<CloudServer>, String>
get_cloud_server(name: String) -> Result<Option<CloudServer>, String>
get_cloud_server_tools(server_key: String) -> Result<Vec<CloudTool>, String>
call_cloud_tool(server_key: String, tool_name: String, arguments: serde_json::Value) -> Result<serde_json::Value, String>
get_cloud_categories() -> Result<Vec<String>, String>
get_cloud_tags() -> Result<Vec<String>, String>
```

---

### Phase I — Templates（配置模板导入导出）

**目标**：将当前服务器+分组配置导出为可分享的 JSON 模板，或从模板导入。

**涉及文件**：

| 文件 | 操作 | 说明 |
|------|------|------|
| `models/template.rs` | 新建 | `ConfigTemplate`, `TemplateServer`, `TemplateGroup` |
| `services/template_service.rs` | 新建 | 导出（从 DB 读取）/ 导入（写入 DB）|
| `commands/templates.rs` | 新建 | 5 个命令 |
| `lib.rs` | 修改 | 注册命令 |
| `tauriClient.ts` | 修改 | 替换 `/templates` 存根 |

**命令签名**：
```rust
list_templates() -> Result<Vec<TemplateInfo>, String>
export_template(options: TemplateExportOptions) -> Result<ConfigTemplate, String>
import_template(template: ConfigTemplate) -> Result<TemplateImportResult, String>
save_template(name: String, description: Option<String>, template: ConfigTemplate) -> Result<(), String>
delete_template(id: String) -> Result<(), String>
```

**ConfigTemplate 结构**（与原项目兼容）：
```rust
pub struct ConfigTemplate {
    pub version: String,      // "1.0"
    pub name: String,
    pub description: Option<String>,
    pub servers: Vec<TemplateServer>,
    pub groups: Vec<TemplateGroup>,
    pub exported_at: String,
}
```

---

### Phase J — MCPB/DXT 文件安装

**目标**：通过文件选择对话框安装 `.mcpb` 格式的 MCP server 包（ZIP 格式，含 manifest.json）。

**涉及文件**：

| 文件 | 操作 | 说明 |
|------|------|------|
| `commands/mcpb.rs` | 新建 | 文件选择 + 解压 + 注册服务器 |
| `lib.rs` | 修改 | 注册命令 |
| `tauriClient.ts` | 修改 | 替换 `/mcpb` 存根 |
| `Cargo.toml` | 修改 | 添加 `zip = "2"` 依赖 |

**实现流程**：
1. 使用 `tauri-plugin-dialog` 打开文件选择器（筛选 `.mcpb`）
2. 使用 `zip` crate 解压到 `$APPDATA/mcphub/servers/{name}/`
3. 读取 `manifest.json` 获取服务器名称、命令、args
4. 调用 `server_service::create()` 注册服务器
5. 调用 `mcp_manager` 连接新服务器

**命令签名**：
```rust
install_mcpb_from_dialog(app: AppHandle) -> Result<ServerConfig, String>
install_mcpb_from_path(app: AppHandle, file_path: String) -> Result<ServerConfig, String>
```

---

### Phase K — Per-Server 工具/提示/资源开关

**目标**：允许用户在服务器详情页对单个工具/提示/资源进行启用/禁用和描述覆盖。

**涉及文件**：

| 文件 | 操作 | 说明 |
|------|------|------|
| `services/server_config_service.rs` | 新建 | CRUD for server_tool_config 表 |
| `commands/server_config.rs` | 新建 | 3 个命令 |
| `lib.rs` | 修改 | 注册命令 |
| `tauriClient.ts` | 修改 | 替换 servers/:n/tools/:t/toggle 存根 |

**命令签名**：
```rust
toggle_server_item(server_name: String, item_type: String, item_name: String, enabled: bool) -> Result<(), String>
update_server_item_description(server_name: String, item_type: String, item_name: String, description: Option<String>) -> Result<(), String>
get_server_item_configs(server_name: String) -> Result<Vec<ServerItemConfig>, String>
```

---

### Phase L — Smart Routing（智能路由）

**目标**：提供 `$smart` 路由，根据查询内容通过工具描述自动选择合适的 MCP 服务器和工具。

**涉及文件**：

| 文件 | 操作 | 说明 |
|------|------|------|
| `services/smart_routing.rs` | 新建 | 基于关键词匹配/向量搜索的工具选择 |
| `commands/smart_routing.rs` | 新建 | 2 个命令 |
| `lib.rs` | 修改 | 注册命令 |
| `tauriClient.ts` | 修改 | 添加 smart routing 相关映射 |

**实现策略**（桌面端简化版）：
- 第一期：关键词匹配（工具名/描述包含查询词）
- 第二期（可选）：嵌入 embedding 模型做向量搜索

**命令签名**：
```rust
get_smart_routing_tools(group: Option<String>, query: Option<String>) -> Result<Vec<Tool>, String>
smart_call_tool(query: String, tool_name: String, arguments: serde_json::Value) -> Result<serde_json::Value, String>
```

---

### Phase M — OpenAPI 生成

**目标**：将当前所有连接的 MCP 工具生成 OpenAPI 3.0 规范，供 OpenWebUI 等工具集成。

**涉及文件**：

| 文件 | 操作 | 说明 |
|------|------|------|
| `services/openapi_service.rs` | 新建 | 从 mcp pool 读取工具列表，生成 OpenAPI JSON |
| `commands/openapi.rs` | 新建 | 2 个命令 |
| `lib.rs` | 修改 | 注册命令 |
| `tauriClient.ts` | 修改 | 添加 openapi 映射 |

**命令签名**：
```rust
get_openapi_spec(group_filter: Option<String>, server_filter: Option<Vec<String>>) -> Result<serde_json::Value, String>
get_openapi_servers() -> Result<Vec<String>, String>
```

---

### Phase N — HTTP 服务端暴露（可选高级功能）

**目标**：当 `system_config.expose_http = true` 时，在桌面应用内启动 Axum HTTP 服务器，将本地 MCP 服务器通过 SSE/HTTP 端点暴露给外部 AI 客户端。

**依赖**：
```toml
axum = { version = "0.7", features = ["json"] }
```

**涉及文件**：

| 文件 | 操作 | 说明 |
|------|------|------|
| `services/http_server.rs` | 新建 | Axum 路由：`/mcp/{server}`, `/mcp/{group}`, `/health` |
| `lib.rs` | 修改 | 根据 system_config 启动/停止 HTTP 服务器 |
| `commands/config.rs` | 修改 | update_system_config 时动态启停 HTTP 服务器 |

**HTTP 端点**：
- `GET /health` — 健康检查
- `GET/POST /mcp/{server}` — 单服务器 SSE/HTTP MCP 端点
- `GET/POST /mcp/{group}` — 分组聚合 MCP 端点
- Bearer Key 认证中间件

---

### Phase O — CI/CD & 打包配置

**需要创建**：`.github/workflows/tauri-build.yml`

```yaml
name: Tauri Desktop Build
on:
  push:
    tags: ['desktop-v*']
  workflow_dispatch:
jobs:
  build:
    strategy:
      matrix:
        include:
          - os: ubuntu-22.04
            target: x86_64-unknown-linux-gnu
          - os: macos-latest
            target: aarch64-apple-darwin
          - os: macos-13
            target: x86_64-apple-darwin
          - os: windows-latest
            target: x86_64-pc-windows-msvc
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with: { node-version: '22' }
      - uses: pnpm/action-setup@v3
        with: { version: 8 }
      - uses: dtolnay/rust-toolchain@stable
        with: { targets: "${{ matrix.target }}" }
      - name: Linux deps
        if: matrix.os == 'ubuntu-22.04'
        run: sudo apt-get install -y libgtk-3-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev
      - run: pnpm install
      - run: cd tauri && npm install
      - run: cd tauri && npm run build -- --target ${{ matrix.target }}
        env: { CARGO_REGISTRIES_CRATES_IO_PROTOCOL: sparse }
      - uses: actions/upload-artifact@v4
        with:
          name: mcphub-desktop-${{ matrix.target }}
          path: tauri/src-tauri/target/${{ matrix.target }}/release/bundle/
```

---

### 实现优先级总览

| 优先级 | Phase | 功能 | 预计工作量 | 依赖 |
|--------|-------|------|-----------|------|
| P0 | A | Schema 修正迁移 | 0.5h | — |
| P1 | B | Bearer Keys | 2h | A |
| P1 | C | Builtin Prompts | 2h | A |
| P1 | D | Builtin Resources | 1.5h | A |
| P1 | E | Activity Log | 2h | A |
| P2 | F | Market（本地）| 2h | — |
| P2 | G | Registry Proxy | 1h | — |
| P2 | H | Cloud Proxy | 2h | A |
| P2 | I | Templates | 2h | A |
| P3 | J | MCPB 安装 | 2h | — |
| P3 | K | Per-server 工具开关 | 1.5h | A |
| P3 | L | Smart Routing | 3h | — |
| P3 | M | OpenAPI 生成 | 2h | — |
| P4 | N | HTTP 服务端暴露 | 4h | B |
| P4 | O | CI/CD 打包 | 1h | — |

---

## 7. 已知问题 & 解决方案

### 问题 1 — Cargo 无法访问 crates.io（GitHub git 协议被阻塞）
- **根因**：网络环境阻断 `https://github.com/rust-lang/crates.io-index`
- **解决**：`tauri/src-tauri/.cargo/config.toml` 配置 sparse 协议
- **运行命令时**：`CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse cargo check`

### 问题 2 — sqlx::query!() 宏需要 DATABASE_URL
- **根因**：编译时 SQL 验证宏需要连接真实数据库
- **解决**：全部使用 `sqlx::query()` 非宏 API（见第 4.2 节）
- **状态**：所有 service 文件已完成转换，`cargo check` 通过

### 问题 3 — stdio_transport.rs UB（无效引用转型）
- **根因**：`&ChildStdin as *const _ as *mut ChildStdin` 是未定义行为
- **解决**：将 `stdin` 字段改为 `Arc<Mutex<Option<ChildStdin>>>`，通过锁获取可变引用

### 问题 4 — tauri::generate_context!() 找不到 frontendDist
- **根因**：`frontend/dist/` 目录不存在
- **解决**：创建占位 `frontend/dist/index.html`（实际构建时由 `pnpm frontend:build` 生成）

### 问题 5 — 图标文件不存在 / 非 RGBA 格式
- **解决**：用 Python 生成最小有效 RGBA PNG 占位图标（实际发布需替换）

---

## 8. 前端 API 调用架构（已完成）

> 此节说明 Phase 6 完成后的 tauriClient.ts 架构，用于新增命令时参考。

### 调用链（简化版）

```
React 组件 → apiRequest(url, options)      # fetchInterceptor.ts
                  │
                  ├─ isTauri() === true
                  │   └─ mapRestToCommand(url, options)  # tauriClient.ts
                  │       └─ invokeMapped(cmd, args)
                  │           └─ invoke(command, args)   # @tauri-apps/api/core
                  │               └─ Rust command handler
                  │
                  └─ isTauri() === false
                      └─ fetch(url, options)  # 标准 HTTP 请求（Web 版）
```

### 添加新命令的步骤模板

```typescript
// tauriClient.ts 中，在 mapRestToCommand() 函数内添加：
if (segs[0] === 'new_feature') {
  if (m === 'GET') return { command: 'list_new_feature', args: {} };
  if (m === 'POST') return { command: 'create_new_feature', args: { payload: body } };
  if (m === 'PUT') return { command: 'update_new_feature', args: { id: segs[1], payload: body } };
  if (m === 'DELETE') return { command: 'delete_new_feature', args: { id: segs[1] } };
}
```

### transformTauriResponse() 返回格式规范

| 返回类型 | transformTauriResponse() 期望格式 |
|---------|----------------------------------|
| 列表 | `{ data: T[], total: number }` 或直接 `T[]` |
| 单项 | `T` 对象直接返回 |
| 操作成功 | `{ success: true }` 或 `{ id: "..." }` |
| 操作失败 | Rust 返回 `Err(String)` → 自动变成 `{ error: "..." }` |

---

## 9. 启动顺序（运行时）

```
1. main.rs → lib::run()
2. tauri::Builder::default()
   ├── 注册所有 tauri-plugin-*
   ├── setup hook:
   │   ├── app.manage(SessionState) — 注册会话状态
   │   └── tokio::spawn:
   │       ├── db::initialize(app) — 创建 SQLite 连接池，运行 migrations
   │       └── services::mcp_manager::start_all(app) — 连接所有 enabled MCP server
   └── invoke_handler — 注册 28 个 Tauri commands
3. tauri::generate_context!() — 加载 tauri.conf.json
4. 打开 Webview 窗口，加载 frontend/dist/index.html
5. 前端初始化，通过 invoke() 与 Rust 通信
```

---

## 10. 下一步行动计划（当前状态）

> Phase 1-7 已全部完成。现在开始实现缺失功能。

### 立即执行（P0）

```bash
# 第一步：创建 schema 修正迁移（Phase A）
# 文件：tauri/src-tauri/migrations/0002_schema_fix.sql
# 内容见第 6 节 Phase A 详细定义

# 第二步：验证迁移能被加载
cd tauri/src-tauri
CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse cargo check
```

### 实现路线图

| 阶段 | Phase | 功能 | 依赖 | 预计工时 |
|------|-------|------|------|---------|
| **P0** | A | Schema 修正迁移 | — | 0.5h |
| **P1** | B | Bearer Keys（API 密钥管理）| A | 2h |
| **P1** | C | Builtin Prompts（内置提示词）| A | 2h |
| **P1** | D | Builtin Resources（内置资源）| A | 1.5h |
| **P1** | E | Activity Log（工具调用监控）| A | 2h |
| **P2** | F | Market（本地 MCP 市场）| — | 2h |
| **P2** | G | Registry Proxy（注册表代理）| — | 1h |
| **P2** | H | Cloud Proxy（MCPRouter 集成）| A | 2h |
| **P2** | I | Templates（配置模板）| A | 2h |
| **P3** | J | MCPB 文件安装 | — | 2h |
| **P3** | K | Per-server 工具开关 | A | 1.5h |
| **P3** | L | Smart Routing | — | 3h |
| **P3** | M | OpenAPI 生成 | — | 2h |
| **P4** | N | HTTP 服务端暴露（可选）| B | 4h |
| **P4** | O | CI/CD 打包 | — | 1h |

### 每个 Phase 完成的验收标准

- [ ] `cargo check` 无新错误
- [ ] 前端对应页面不再显示存根数据或"不支持"错误
- [ ] `tauriClient.ts` 中对应存根已被真实命令替换
- [ ] SQLite 数据持久化正常（重启后数据不丢失）
