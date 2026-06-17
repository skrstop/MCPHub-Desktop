# MCPHub Desktop (Tauri) — Agent 开发文档

> 本文档是 Tauri 桌面客户端迁移的**完整参考**，供 AI Agent 和开发者续接工作使用。
> 包含：原项目架构、桌面端架构、已完成内容、待办事项及所有关键技术细节。

> ⚠️ **核心约束（MUST FOLLOW）**：**禁止修改 `mcphub-origin/frontend/`、`mcphub-origin/src/` 等原始源文件**。
> 所有修改必须在 `frontend/`、`src-tauri/`、`locales/` 目录内进行。
> 做任何较大修改后，必须更新 agent.md 文档，用来记录。目的：为了方便后续维护和理解项目结构。

---

## 1. 项目概览

### 1.1 原项目（mcphub-origin — Node.js/Express + React/Vite）

| 属性 | 值 |
|------|-----|
| 包名 | `@samanhappy/mcphub` |
| 技术栈 | Express.js + TypeScript ESM + React/Vite + Tailwind CSS |
| 前端 | `mcphub-origin/frontend/` (React + Vite) |
| 认证 | JWT + bcrypt + Better-Auth（OAuth/OIDC） |
| 数据存储 | JSON 文件 (`mcp_settings.json`) 或 PostgreSQL |
| MCP 连接 | `src/services/mcpService.ts` 管理所有 MCP 服务端连接 |
| 路由 | `/mcp/{group\|server}`、`/mcp/$smart`、REST API `/api/*` |
| i18n | react-i18next，翻译文件在 `locales/` |

### 1.2 桌面端项目（mcphub-desktop — Rust/Tauri 2 + 复用原 React 前端）

| 属性 | 值 |
|------|-----|
| 位置 | 项目根目录 |
| Tauri 版本 | v2 |
| Rust crate | `src-tauri/` |
| 前端 | `frontend/`（原 mcphub-origin/frontend 的副本，有改造） |
| 数据存储 | SQLite（`$APPDATA/mcphub.db`，通过 sqlx 0.8） |
| 认证 | jsonwebtoken 9 + bcrypt 0.15，密钥存 OS 钥匙串(keyring 3) |
| 异步运行时 | tokio 1 full |
| HTTP 客户端 | reqwest 0.12 (rustls-tls + stream + json) |
| 应用标识 | `app.mcphub.desktop` |

---

## 2. 桌面端架构

### 2.1 目录结构

```
mcphub-desktop/
├── frontend/                   # 原 mcphub-origin/frontend/ 的副本（有改造）
│   ├── src/
│   │   ├── pages/              # 页面组件（11个页面）
│   │   ├── components/         # 可复用 UI 组件
│   │   │   ├── layout/         # Header, Sidebar, Content
│   │   │   ├── ui/             # 通用 UI 组件
│   │   │   ├── icons/          # SVG 图标组件
│   │   │   ├── ServerCard.tsx   # ⚠️ 本地修改：移除 sponsor/wechat/discord
│   │   │   ├── ServerForm.tsx   # ⚠️ 本地修改：使用 hub-* 样式 + 保留 visibility/OAuth2
│   │   │   └── RuntimeVersionManager.tsx  # 🆕 桌面端新增：运行时版本管理
│   │   ├── utils/
│   │   │   ├── tauriClient.ts  # 🆕 isTauri() + invoke() 封装 + REST→invoke 路由映射
│   │   │   ├── fetchInterceptor.ts  # ⚠️ 修改：拦截请求转为 invoke()
│   │   │   └── runtime.ts      # 运行时配置
│   │   ├── contexts/
│   │   │   ├── AuthContext.tsx  # ⚠️ 修改：支持 skipAuth/guest 模式
│   │   │   └── ...
│   │   └── services/
│   │       └── configService.ts # ⚠️ 修改：getPublicConfig 使用 apiGet
│   ├── dist/                   # Vite 构建输出
│   └── package.json
├── locales/                    # i18n 翻译（en/zh/fr/tr）
│   ├── en.json                 # ⚠️ 本地修改：添加 runtime* 翻译
│   └── zh.json                 # ⚠️ 本地修改：添加 runtime* 翻译
├── mcphub-origin/              # git 子模块，仅作代码参考
├── src-tauri/                  # Rust 后端
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── migrations/
│   │   ├── 0001_initial.sql
│   │   ├── 0002_schema_fix.sql
│   │   ├── 0003_config_json.sql
│   │   ├── 0004_default_admin.sql
│   │   └── 0005_default_skip_auth.sql  # 🆕 桌面端：默认开启免登录
│   └── src/
│       ├── main.rs
│       ├── lib.rs              # 应用核心：插件注册、setup hook、invoke_handler
│       ├── auth/
│       │   └── mod.rs          # JWT + bcrypt + guest token 签发
│       ├── db/
│       │   └── mod.rs          # SQLite 连接池
│       ├── models/
│       │   ├── server.rs       # ServerType, ServerConfig, ServerStatus, Tool
│       │   ├── user.rs         # User, UserRole(Admin|User|Guest), UserInfo
│       │   ├── group.rs
│       │   ├── config.rs
│       │   ├── auth.rs
│       │   ├── bearer_key.rs
│       │   └── log.rs
│       ├── mcp/
│       │   ├── client.rs       # McpTransport trait + McpClient
│       │   ├── stdio_transport.rs
│       │   ├── sse_transport.rs    # ⚠️ 本地修改：改进 SSE 事件解析
│       │   ├── http_transport.rs   # Streamable HTTP POST 传输
│       │   └── pool.rs         # 全局连接池
│       ├── services/
│       │   ├── mod.rs
│       │   ├── mcp_manager.rs
│       │   ├── server_service.rs
│       │   ├── user_service.rs
│       │   ├── group_service.rs
│       │   ├── config_service.rs
│       │   ├── log_service.rs
│       │   ├── settings_import.rs
│       │   ├── bearer_key_service.rs
│       │   ├── http_server.rs      # 内置 HTTP 服务器（expose_http 模式）
│       │   ├── runtime_env.rs      # 🆕 运行时环境管理（Node.js/Python 版本隔离）
│       │   ├── server_tool_config_service.rs
│       │   └── market_service.rs
│       └── commands/
│           ├── mod.rs
│           ├── auth.rs         # login/logout/get_current_user/change_password
│           ├── servers.rs      # list/get/add/update/delete/toggle/reload
│           ├── groups.rs
│           ├── tools.rs
│           ├── users.rs
│           ├── config.rs       # 🆕 新增 get_public_config 命令
│           ├── logs.rs
│           ├── bearer_keys.rs
│           ├── prompts.rs
│           ├── resources.rs
│           ├── market.rs
│           ├── registry.rs
│           ├── cloud.rs
│           ├── server_tool_config.rs
│           ├── http_server.rs
│           └── runtime.rs      # 🆕 运行时版本管理命令
├── servers.json                # 本地 MCP 市场数据
├── package.json
└── agent.md                    # 本文档
```

### 2.2 数据流架构

```
React Frontend (frontend/dist/)
        │
        │  isTauri() ? invoke() : fetch()
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
        ├─▶ mcp/ (MCP 连接池 = 原 mcpService.ts)
        │       ├─▶ stdio_transport (子进程，使用 runtime_env 解析命令)
        │       ├─▶ sse_transport (HTTP SSE)
        │       └─▶ http_transport (Streamable HTTP)
        └─▶ runtime_env/ (管理下载的 Node.js/Python 版本)
```

---

## 3. 桌面端本地自定义功能（与 origin 的差异）

> 以下是桌面端相对于 mcphub-origin 的所有自定义修改，同步时需保留这些差异。

### 3.1 核心架构差异

#### 3.1.1 Tauri IPC 通信层
**文件**：`frontend/src/utils/tauriClient.ts`

- 新增 `isTauri()` 函数：检测是否在 Tauri 环境运行
- 新增 `mapRestToCommand()` 函数：将 REST API 路径映射到 Tauri 命令
- 新增 `invokeMapped()` 函数：调用 Tauri 命令并处理响应
- 新增 `transformTauriResponse()` 函数：将 Tauri 响应转换为前端期望格式
- 新增 `public-config` 路由映射（`get_public_config` 命令）
- 新增 `get_public_config` 响应转换

#### 3.1.2 请求拦截器
**文件**：`frontend/src/utils/fetchInterceptor.ts`

- `apiRequest()` 函数集成 `isTauri()` 检测
- 在 Tauri 环境下自动路由到 `invoke()` 而非 HTTP fetch
- 保留 Web 环境的正常 HTTP 请求能力

#### 3.1.3 认证上下文
**文件**：`frontend/src/contexts/AuthContext.tsx`

- 支持 `skipAuth` 模式（免登录模式）
- 当 `skipAuth=true` 时，自动创建 guest 用户（`username: '免登陆模式'`, `isAdmin: true`）
- 默认启用免登录模式（桌面端不需要登录）

#### 3.1.4 配置服务
**文件**：`frontend/src/services/configService.ts`

- `getPublicConfig()` 使用 `apiGet` 而非 `fetchWithInterceptors`（适配 Tauri IPC）
- 默认返回 `skipAuth: true`（桌面端默认免登录）

### 3.2 UI/样式差异

#### 3.2.1 ServerForm（服务器表单）
**文件**：`frontend/src/components/ServerForm.tsx`

- 使用 mcphub-origin 的 `hub-*` 设计系统样式（`hub-card`, `hub-btn`, `hub-icon-btn` 等）
- **隐藏了可见性选择器**（Private/Group/Public）——桌面端默认所有服务器为公开
- 可见性默认值从 `private` 改为 `public`
- 保留桌面端新增的 OAuth2 完整配置（`oauth2TokenUrl`, `oauth2ClientId`, `oauth2ClientSecret`）
- 使用 lucide-react 的 `X` 图标作为关闭按钮

#### 3.2.1.1 ServerCard（服务器卡片）
**文件**：`frontend/src/components/ServerCard.tsx`

- **隐藏了可见性列**——桌面端不需要私有/公开区分，所有服务器默认公开
- 可见性相关的 UI 元素（下拉选择器/徽章）已移除，用空 `div` 占位保持网格布局

#### 3.2.2 Header（顶部导航）
**文件**：`frontend/src/components/layout/Header.tsx`

- GitHub 链接改为 `https://github.com/skrstop/mcphub-desktop`
- 移除了文档按钮（BookOpen 图标）

#### 3.2.3 UserProfileMenu（用户菜单）
**文件**：`frontend/src/components/ui/UserProfileMenu.tsx`

- 移除了赞助按钮（SponsorIcon）
- 移除了微信按钮（WeChatIcon）
- 移除了 Discord 按钮（DiscordIcon）
- 保留了：设置、关于、退出登录

#### 3.2.4 AboutDialog（关于对话框）
**文件**：`frontend/src/components/ui/AboutDialog.tsx`

- 添加了 "MCPHub Desktop" 标识文字

#### 3.2.5 Dashboard（仪表盘）
**文件**：`frontend/src/pages/Dashboard.tsx`

- 隐藏了 SMART 接入点（智能路由未实现）
- 隐藏了 Docs 文档链接

#### 3.2.6 LoginPage（登录页）
**文件**：`frontend/src/pages/LoginPage.tsx`

- GitHub 链接改为 `https://github.com/skrstop/mcphub-desktop`
- 移除了文档按钮
- 用户名默认填充 `admin`，且设为只读（`readOnly`），用户不能修改
  - 桌面端默认使用 admin 账户登录，简化登录流程
  - 样式使用 `opacity: 0.7` 和 `cursor: not-allowed` 提示不可编辑
- 登录表单下方显示默认密码提示：`默认密码: admin`（英文：`Default password: admin`）
  - 使用 `t('auth.defaultPasswordHint')` 国际化
- **Logo 使用应用图标**：用 `/assets/logo.png`（来自 `src-tauri/icons/icon.png`）替代原来的 CSS 样式 "M" 字母

#### 3.2.6.1 Sidebar（侧边栏）
**文件**：`frontend/src/components/layout/Sidebar.tsx`

- **Logo 使用应用图标**：用 `/assets/logo.png` 替代原来的 CSS 样式 "M" 字母
- 统一登录页和首页左上角的 logo 显示

#### 3.2.7 SettingsPage（设置页）
**文件**：`frontend/src/pages/SettingsPage.tsx`

- 导入了 `isTauri` 函数
- 导入了 `RuntimeVersionManager` 组件
- 隐藏了以下未实现的功能模块：
  - Smart Routing（智能路由）
  - Tool Result Compression（工具结果压缩）
  - OAuth Server（OAuth 服务器）
  - MCP Router（MCPRouter 配置）
  - Better Auth（社交登录配置）
- 在安装配置部分添加了 Node.js 版本管理（RuntimeVersionManager）
- 在安装配置部分添加了 Python 版本管理（RuntimeVersionManager）
- **隐藏了安装配置中的"基础地址"字段**（baseUrl）——端口在路由配置中设置
- **在路由配置中新增了 HTTP 服务端口设置**：
  - `exposeHttp`：启用/禁用 HTTP 服务开关
  - `httpPort`：HTTP 服务监听端口（默认 23333）
  - 修改端口后提示用户需要重启应用
- **默认 baseUrl 从 `http://localhost:3000` 改为 `http://localhost:23333`**（与 HTTP 服务器默认端口一致）
- 更新了所有语言的 `baseUrlPlaceholder` 翻译
- 添加了 `exposeHttp`、`httpPort` 相关的国际化翻译

#### 3.2.7.1 SettingsContext（设置上下文）
**文件**：`frontend/src/contexts/SettingsContext.tsx`

- `RoutingConfig` 接口新增 `httpPort: number` 和 `exposeHttp: boolean` 字段
- 默认值：`httpPort: 23333`，`exposeHttp: true`

### 3.3 国际化差异

#### 3.3.1 中文翻译
**文件**：`locales/zh.json`

新增的翻译键：
```json
{
  "settings": {
    "nodeVersion": "Node.js 版本",
    "nodeVersionDescription": "选择或安装特定的 Node.js 版本用于运行 MCP 服务器",
    "pythonVersion": "Python 版本",
    "pythonVersionDescription": "选择或安装特定的 Python 版本用于运行 MCP 服务器",
    "runtimeSystemDefault": "系统默认",
    "runtimeInstalled": "已安装",
    "runtimeBroken": "异常",
    "runtimeBrokenWarning": "版本 {{version}} 安装不完整，建议重新安装",
    "runtimeReinstall": "重新安装",
    "runtimeReinstallTip": "强制重新安装当前选中的版本",
    "runtimeUninstall": "卸载",
    "runtimePhase.started": "开始",
    "runtimePhase.downloading": "下载中",
    "runtimePhase.extracting": "解压中",
    "runtimePhase.verifying": "验证中",
    "runtimePhase.running": "执行中",
    "runtimePhase.done": "完成",
    "runtimePhase.error": "错误"
  }
}
```

#### 3.3.2 英文翻译
**文件**：`locales/en.json`

新增的翻译键（同上，英文版本）

### 3.4 自动更新配置

#### 3.4.1 更新机制概述

桌面端使用 Tauri 原生 updater 插件实现自动更新，**不依赖** mcphub-origin 的 changelog API。

**更新流程**：
1. GitHub Actions 构建所有平台的安装包
2. 使用私钥签名更新包（生成 `.sig` 文件）
3. 生成 `latest.json`（包含版本信息、下载链接和签名）
4. 创建 draft Release 并上传所有文件
5. 用户端定期检查 `latest.json` 端点，验证签名后提示更新

**相关文件**：
- `src-tauri/tauri.conf.json` — updater 插件配置（endpoints + pubkey）
- `frontend/src/utils/version.ts` — Tauri updater 集成（`check()`, `downloadAndInstall()`）
- `frontend/src/services/changelogService.ts` — Web 端 changelog 服务（Tauri 中禁用）
- `.github/workflows/release.yml` — CI/CD 构建和发布流程

#### 3.4.2 签名密钥配置

**生成签名密钥**：
```bash
bash scripts/generate-signing-key.sh
```

**配置步骤**：
1. 运行脚本生成密钥对（`~/.tauri/mcphub.key` 和 `~/.tauri/mcphub.key.pub`）
2. 将公钥内容复制到 `src-tauri/tauri.conf.json` 的 `plugins.updater.pubkey` 字段
3. 添加 GitHub Secrets：
   - `TAURI_SIGNING_PRIVATE_KEY` — 私钥文件内容
   - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` — 密钥密码（如果有）

**验证配置**：
```bash
bash scripts/verify-signing.sh
```

#### 3.4.3 GitHub Actions 配置

**文件**：`.github/workflows/release.yml`

**触发条件**：
- 推送 `v*` 格式的 tag（如 `v1.0.17`）
- 手动触发（workflow_dispatch）

**构建矩阵**：
| 平台 | Runner | Target | 架构 |
|------|--------|--------|------|
| macOS ARM64 | macos-latest | aarch64-apple-darwin | arm64 |
| macOS x64 | macos-13 | x86_64-apple-darwin | x64 |
| Linux x64 | ubuntu-22.04 | x86_64-unknown-linux-gnu | x64 |
| Linux ARM64 | ubuntu-22.04-arm | aarch64-unknown-linux-gnu | arm64 |
| Windows x64 | windows-latest | x86_64-pc-windows-msvc | x64 |
| Windows ARM64 | windows-latest | aarch64-pc-windows-msvc | arm64 |

**关键步骤**：
1. 安装 Node.js 20 + Rust stable + 目标 triple
2. 安装系统依赖（Linux: webkit2gtk, appindicator3, rsvg2, patchelf, ssl）
3. 下载 bundled runtimes（Node.js + uv + Python）
4. 构建 Tauri 应用（使用私钥签名）
5. 收集平台产物（.dmg, .app.tar.gz, .deb, .AppImage, .exe, .nsis.zip）
6. 生成 `latest.json`（Python 脚本解析 .sig 文件）
7. 创建 draft Release 并上传所有文件

**产物说明**：
| 平台 | 安装包 | 更新包 | 签名文件 |
|------|--------|--------|----------|
| macOS | .dmg | .app.tar.gz | .app.tar.gz.sig |
| Linux | .deb, .AppImage | .AppImage.tar.gz | .AppImage.tar.gz.sig |
| Windows | .exe, .msi | .nsis.zip | .nsis.zip.sig |

#### 3.4.4 latest.json 格式

```json
{
  "version": "1.0.16",
  "notes": "MCPHub Desktop 1.0.16\n\nSee release page for full changelog.",
  "pub_date": "2026-06-17T12:00:00Z",
  "platforms": {
    "darwin-aarch64": {
      "signature": "dW50cnVzdGVkIGNvbW1lbnQ6...",
      "url": "https://github.com/skrstop/MCPHub-Desktop/releases/download/v1.0.16/MCPHub.Desktop_1.0.16_aarch64.app.tar.gz"
    },
    "darwin-x86_64": {
      "signature": "...",
      "url": "..."
    },
    "linux-x86_64": {
      "signature": "...",
      "url": "..."
    },
    "windows-x86_64": {
      "signature": "...",
      "url": "..."
    }
  }
}
```

**平台标识**：
- `darwin-aarch64` — macOS ARM64 (Apple Silicon)
- `darwin-x86_64` — macOS x64 (Intel)
- `linux-aarch64` — Linux ARM64
- `linux-x86_64` — Linux x64
- `windows-aarch64` — Windows ARM64
- `windows-x86_64` — Windows x64

#### 3.4.5 Changelog API 禁用说明

**文件**：`frontend/src/utils/tauriClient.ts`

```typescript
// Changelog endpoints — not implemented in desktop client
if (segs[0] === 'changelog') {
  return { command: '__stub__', args: { __response: { success: true, data: { hasUpdate: false, entries: [] } } } };
}
```

**原因**：
- Tauri 桌面应用使用原生 updater 插件进行自动更新
- Changelog API 是为 Web 版本设计的，在桌面版本中不需要
- `frontend/src/utils/version.ts` 正确实现了 Tauri updater 集成

**前端调用**：
```typescript
// 检查更新（使用 Tauri updater 插件）
import { checkForAppUpdate, installAppUpdate } from '@/utils/version';

const updateInfo = await checkForAppUpdate();
if (updateInfo) {
  console.log('New version:', updateInfo.version);
  await installAppUpdate((event) => {
    console.log('Download progress:', event);
  });
}
```

#### 3.4.6 故障排除

**问题：updater 无法验证签名**
- 原因：公钥配置错误或私钥不匹配
- 解决：确认 `tauri.conf.json` 中的 `pubkey` 与生成的公钥一致，确认 GitHub Secrets 中的私钥与公钥配对

**问题：CI 构建失败**
- 原因：GitHub Secrets 配置错误
- 解决：检查 `TAURI_SIGNING_PRIVATE_KEY` 和 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` 是否正确设置

**问题：用户无法收到更新**
- 原因：`latest.json` 文件不存在或格式错误
- 解决：检查 GitHub Release 是否包含 `latest.json` 文件，确认格式正确

**详细文档**：参见 `SIGNING_SETUP.md`

### 3.5 Rust 后端差异

#### 3.5.1 免登录模式
**文件**：`src-tauri/src/commands/config.rs`

- 新增 `get_public_config` 命令：返回 `skipAuth` 和 `permissions` 配置
- 默认 `skipAuth: true`（桌面端默认免登录）

**文件**：`src-tauri/src/lib.rs`

- 注册了 `get_public_config` 命令

**文件**：`src-tauri/migrations/0005_default_skip_auth.sql`

- 数据库迁移：默认设置 `routing.skipAuth = true`

#### 3.5.2 Guest 用户支持
**文件**：`src-tauri/src/models/user.rs`

- `UserRole` 枚举新增 `Guest` 变体

**文件**：`src-tauri/src/commands/auth.rs`

- `login` 命令处理 `UserRole::Guest` 匹配
- `get_current_user` 命令在无 token 且 skipAuth 启用时返回 guest 用户

**文件**：`src-tauri/src/auth/mod.rs`

- 新增 `issue_guest_token()` 函数：签发 guest JWT token

#### 3.5.3 SSE 传输改进
**文件**：`src-tauri/src/mcp/sse_transport.rs`

改进内容：
- 正确跟踪 SSE 事件类型（`event:` 行）
- 支持多种 endpoint 格式：
  - `event: endpoint\ndata: /messages`（标准 MCP SSE）
  - `data: {"endpoint": "/messages"}`（JSON 格式）
  - `data: /messages`（无 event 类型）
- 不自动添加 `/sse` 后缀（使用用户提供的 URL 原样连接）
- 改进后台 SSE 响应读取（使用缓冲区处理不完整行）
- 添加详细日志输出

#### 3.5.4 运行时环境管理
**文件**：`src-tauri/src/services/runtime_env.rs`

- 管理下载的 Node.js 和 Python 版本
- 解析命令到下载的版本（而非系统环境）
- 支持设置活跃版本（`set_active_node`, `set_active_python`）
- 提供环境变量覆盖（`UV_DEFAULT_INDEX`, `npm_config_registry`）

**文件**：`src-tauri/src/mcp/stdio_transport.rs`

- 使用 `runtime_env::resolve_command()` 解析命令
- 使用 `runtime_env::env_overrides()` 获取环境变量

**文件**：`src-tauri/src/commands/runtime.rs`

- 新增运行时版本管理命令：
  - `list_node_versions` / `list_python_versions`
  - `install_node_version` / `install_python_version`
  - `uninstall_node_version` / `uninstall_python_version`
  - `set_active_node_version` / `set_active_python_version`
  - `get_active_node_version` / `get_active_python_version`

#### 3.5.5 内置 HTTP 服务器
**文件**：`src-tauri/src/services/http_server.rs`

- 使用 Axum 框架实现内置 HTTP 服务器
- 支持 MCP Streamable HTTP 协议（JSON-RPC 2.0）
- 支持 Bearer Key 认证
- 支持 Smart 路由（`/mcp`, `/mcp/$smart`, `/mcp/$smart/{group}`）
- 支持分组路由（`/mcp/{group}`）
- 支持单服务器路由（`/mcp/{server}`）

---

## 4. 上游 mcphub-origin 同步记录

### 4.1 同步策略

1. `mcphub-origin/` 是 git 子模块，仅作为代码参考与 diff 来源，**桌面端永远不直接修改子模块内容**。
2. 桌面端 `frontend/`、`locales/` 是 origin 对应目录的**有改造副本**：
   - 大部分文件保持与 origin 一致；
   - desktop 主动改造的文件（见第 3 节）保留差异，**同步时需手动合并**。
3. 后端由 Rust 重写在 `src-tauri/`，**Node 后端代码不直接同步**，但需评估安全相关 fix 是否要在 Rust 端镜像实现。
4. `package.json`、`pnpm-lock.yaml`、`docs/`、`Dockerfile`、`docker-compose*.yml` 等部署/文档文件**不同步**。

### 4.2 同步操作流程（标准 SOP）

```bash
# 1. 更新 origin 子模块到 latest main
cd mcphub-origin && git fetch origin && git checkout origin/main && cd ..

# 2. 列出待同步提交（基线 = 上次记录的 commit）
cd mcphub-origin && git --no-pager log --oneline <last-sync-sha>..HEAD

# 3. 生成 frontend + locales 综合 patch
git --no-pager diff <last-sync-sha>..HEAD -- frontend/ locales/ > /tmp/origin_frontend.patch

# 4. dry-run 检查冲突
cd .. && patch -p1 --dry-run --batch --forward --no-backup-if-mismatch -F 5 < /tmp/origin_frontend.patch

# 5. 对未冲突文件直接 cp 覆盖；对冲突文件手动合并（保留桌面端差异）
# 6. 升级版本号
# 7. cd frontend && npm run build 验证
# 8. 更新本章节「最近同步基线」与「同步条目」
```

### 4.3 最近同步基线

| 项 | 值 |
|------|-----|
| **当前已同步到 origin commit** | `a34dbac` (origin/main) |
| **对应 origin tag** | `v0.12.15` |
| **桌面端版本号** | `1.0.16` |
| **同步执行日期** | 2026-06-17 |

> 下次同步时，使用 `a34dbac` 作为新的基线 SHA 起点（命令：`cd mcphub-origin && git --no-pager log --oneline a34dbac..HEAD`）。

### 4.4 同步条目历史

#### 2026-06-17：同步 `3ea0bbe` → `a34dbac`（77 个 commit）

**已同步到 desktop（前端 / locales）**

| 来源 commit | 说明 | desktop 应用方式 |
|------|------|------|
| `bbc8f00` | 更新 skipAuth 描述 | `locales/{en,fr,tr,zh}.json` 直接覆盖 |
| `c44a32b` | 代码分割与懒加载 | `frontend/src/App.tsx` 手动合并 |
| `bcf993e` | UI 重新设计（32 个组件文件） | 大部分文件直接覆盖 |
| `3ea2019` | 按钮样式更新 | 直接覆盖 |
| `c2b16da` | 宽屏布局优化 | 直接覆盖 |
| `f04eb69` | 服务器可见性列 | 直接覆盖 |
| `8977514` | 轮询优化 | 直接覆盖 |
| `3142206` | OIDC 支持 | 直接覆盖 |
| `62d706d` | 活动日志 IP 追踪 | 直接覆盖 |
| `ee1aa9d` | 基础 URL 解析增强 | 直接覆盖 |
| `c951f63` | Discord 链接更新 | 直接覆盖 |
| `bfbf6b6` | 可见性权限修复 | 直接覆盖 |
| `ff05c9b` | OAuth2 客户端凭证 | 直接覆盖 |
| `f2baf0a` | stdio 请求选项保留 | 直接覆盖 |
| `ed622fc` | ssoUserId 匹配 | 直接覆盖 |
| `79730a8` | 服务器可见性编辑 | 直接覆盖 |
| `a37bad5` | 用户级 bearer keys | 直接覆盖 |
| `b344aba` | 隐藏系统日志导航 | 直接覆盖 |
| `f783ef2` | 访问范围过滤 | 直接覆盖 |
| `92625b2` | 用户名列过滤 | 直接覆盖 |
| `b721275` | OIDC 账户链接 | 直接覆盖 |
| `58e11ab` | Bearer Keys 样式修复 | 直接覆盖 |
| `bb2652e` | MCP Apps 支持 | 直接覆盖 |
| `3ce8bc2` | Context Footprint | 直接覆盖 |
| `c0050d8` | 工具结果压缩 | 直接覆盖 |
| `deff236` | Changelog 功能 | 直接覆盖（新增 `changelogService.ts`） |
| `33b6613` | 禁用过滤标签 | 直接覆盖 |
| `2982a08` | 自定义 Switch 组件 | 直接覆盖 |
| `fd43a8b` | ServerCard 样式修复 | 直接覆盖 |
| `e84ff7e` | 工具描述管理增强 | 直接覆盖 |
| `a34dbac` | UUID 正则检查 | 直接覆盖 |
| `6a0256a` | 用户级密钥查看公共服务器 | 直接覆盖 |

**新增文件**

| 文件 | 来源 commit | 说明 |
|------|------|------|
| `frontend/src/services/changelogService.ts` | `deff236` | Changelog 服务 |
| `frontend/src/utils/bearerKeyScopeFilter.ts` | `a37bad5` | Bearer key 范围过滤 |
| `frontend/src/utils/contextCost.ts` | `3ce8bc2` | Context 成本计算 |
| `frontend/src/utils/jsonImport.ts` | `f2baf0a` | JSON 导入工具 |
| `frontend/src/utils/navigationPermissions.ts` | `b344aba` | 导航权限 |
| `frontend/src/utils/serverFilters.ts` | `33b6613` | 服务器过滤 |
| `frontend/src/utils/serverListState.ts` | `33b6613` | 服务器列表状态 |
| `frontend/src/utils/serverPermissions.ts` | `bfbf6b6` | 服务器权限 |
| `frontend/src/utils/serverVisibility.ts` | `f04eb69` | 服务器可见性 |
| `frontend/src/utils/toolDescription.ts` | `e84ff7e` | 工具描述 |
| `frontend/src/components/ui/EndpointCopy.tsx` | `bcf993e` | 端点复制组件 |
| `frontend/src/components/ui/StatusDot.tsx` | `bcf993e` | 状态点组件 |

**未同步（后端 / 不适用）**

| 来源 commit | 类型 | 处理决策 |
|------|------|------|
| `077eed9` | Add headless mode | **Rust 端 TODO** |
| `7300b74` | skipAuth guest access | ✅ 已实现 |
| `927e98d` | reject scoped bearer keys (CWE-863) | **Rust 端 TODO** |
| `60a4da4` | require admin for MCP settings export (CWE-862) | **Rust 端 TODO** |
| `2de5057` | TRUST_PROXY 环境变量 | **Rust 端 TODO** |
| `45b1f05` | scoped bearer auth on smart routes | **Rust 端 TODO** |
| `8fe47e2` | load server config from DB after OAuth | **Rust 端 TODO** |
| `87f241a` | exclude tool description from hash | **Rust 端 TODO** |
| `6377812` | skip reconnect for disabled servers | **Rust 端 TODO** |
| `3fb39f0` | harden JWT binding (GHSA-wf8q-wvv8-p8jf) | **Rust 端 TODO** |
| `06b18cb` | server env interpolation in headers | **Rust 端 TODO** |
| `da23f69` | return MCP initialize metadata | **Rust 端 TODO** |
| `5ca154a` | prevent embedding regeneration | **Rust 端 TODO** |
| `c8779df` | SSO/OIDC user matching | **Rust 端 TODO** |
| `b1e7a52` | smart routing arbitrary args | **Rust 端 TODO** |
| `030d12e` | DEFAULT_REQUEST_TIMEOUT | **Rust 端 TODO** |
| `f42c828` | Redis error handling | **Rust 端 TODO** |
| `330e0f7` | env-first better auth config | **Rust 端 TODO** |
| `20adf81` | hosted mode | **Rust 端 TODO** |
| `61a59b4` | runtime config resolution | **Rust 端 TODO** |
| `e894500` | IPv4-mapped IPv6 normalization | **Rust 端 TODO** |
| `0c30aad` | Better Auth trusted origins | **Rust 端 TODO** |
| `bbec0c6` | bridge Better Auth session | **Rust 端 TODO** |
| 依赖更新 commits | chore(deps) | **不同步** |

---

## 5. 开发环境配置

### 5.1 关键注意事项

```bash
# ⚠️ 必须使用 sparse 协议运行 cargo（绕过 GitHub git 访问限制）
cd src-tauri
CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse cargo check

# .cargo/config.toml 已配置（无需手动设置环境变量时也生效）
```

### 5.2 sqlx 使用规则（重要）

```rust
// ✅ 正确：使用 sqlx::query() 非宏 API
use sqlx::Row;
let rows = sqlx::query("SELECT id, name FROM servers")
    .fetch_all(db::pool())
    .await?;
let id: String = rows[0].try_get("id")?;

// ❌ 禁止：sqlx::query!() 宏（需要 DATABASE_URL 编译时检查，桌面应用无法提供）

// ✅ 例外：sqlx::migrate!() 是嵌入文件宏，不需要 DATABASE_URL，可以使用
sqlx::migrate!("./migrations").run(&pool).await?;
```

### 5.3 开发命令

```bash
# 前端开发
cd frontend && npm run dev

# 前端构建
cd frontend && npm run build

# Rust 编译检查
cd src-tauri && CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse cargo check

# Tauri 开发模式（启动 frontend dev server + Tauri 窗口）
npm run dev

# Tauri 生产构建
npm run build
```

---

## 6. 已知问题 & 解决方案

### 问题 1 — Cargo 无法访问 crates.io
- **根因**：网络环境阻断 `https://github.com/rust-lang/crates.io-index`
- **解决**：`src-tauri/.cargo/config.toml` 配置 sparse 协议

### 问题 2 — sqlx::query!() 宏需要 DATABASE_URL
- **解决**：全部使用 `sqlx::query()` 非宏 API

### 问题 3 — SSE 连接失败
- **根因**：SSE 事件解析不正确，未跟踪 event 类型
- **解决**：改进 SSE 传输，正确跟踪 `event:` 类型，支持多种 endpoint 格式

### 问题 4 — 运行时版本隔离
- **根因**：MCP 服务器使用系统环境的 Node.js/Python
- **解决**：实现 `runtime_env` 服务，管理下载的版本，`stdio_transport` 使用 `resolve_command()` 解析命令

---

## 7. 当前状态与待办

### 已完成
- [x] 基础架构（Tauri + SQLite + MCP 传输层）
- [x] 所有 Tauri 命令（auth, servers, groups, tools, users, config, logs）
- [x] 前端适配器（tauriClient.ts + fetchInterceptor.ts）
- [x] 系统托盘
- [x] 免登录模式（guest 模式）
- [x] 运行时版本管理（Node.js/Python）
- [x] 内置 HTTP 服务器（expose_http 模式）
- [x] Bearer Keys 管理
- [x] Builtin Prompts/Resources
- [x] Activity Log
- [x] Market（本地 MCP 市场）
- [x] Registry Proxy
- [x] Cloud Proxy（MCPRouter）
- [x] SSE 传输改进

### 待办
- [ ] Smart Routing（智能路由）
- [ ] OAuth Server
- [ ] Better Auth 集成
- [ ] Tool Result Compression
- [ ] OpenAPI 生成
- [ ] MCPB/DXT 文件安装
- [ ] Templates（配置模板）
- [ ] CI/CD 打包配置
