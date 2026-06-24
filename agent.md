# [](https://)[](https://)MCPHub Desktop (Tauri) — Agent 开发文档

> 本文档是 Tauri 桌面客户端迁移的**完整参考**，供 AI Agent 和开发者续接工作使用。
> 包含：原项目架构、桌面端架构、已完成内容、待办事项及所有关键技术细节。

> ⚠️ **核心约束（MUST FOLLOW）**：**禁止修改 `mcphub-origin/frontend/`、`mcphub-origin/src/` 等原始源文件**。
> 所有修改必须在 `frontend/`、`src-tauri/`、`locales/` 目录内进行。
> 做任何较大修改后，必须更新 agent.md 文档，用来记录。目的：为了方便后续维护和理解项目结构。

---

## 1. 项目概览[](https://)[](https://)

### 1.1 原项目（mcphub-origin — Node.js/Express + React/Vite）


| 属性     | 值                                                      |
| -------- | ------------------------------------------------------- |
| 包名     | `@samanhappy/mcphub`                                    |
| 技术栈   | Express.js + TypeScript ESM + React/Vite + Tailwind CSS |
| 前端     | `mcphub-origin/frontend/` (React + Vite)                |
| 认证     | JWT + bcrypt + Better-Auth（OAuth/OIDC）                |
| 数据存储 | JSON 文件 (`mcp_settings.json`) 或 PostgreSQL           |
| MCP 连接 | `src/services/mcpService.ts` 管理所有 MCP 服务端连接    |
| 路由     | `/mcp/{group                                            |
| i18n     | react-i18next，翻译文件在`locales/`                     |

### 1.2 桌面端项目（mcphub-desktop — Rust/Tauri 2 + 复用原 React 前端）


| 属性        | 值                                                        |
| ----------- | --------------------------------------------------------- |
| 位置        | 项目根目录                                                |
| Tauri 版本  | v2                                                        |
| Rust crate  | `src-tauri/`                                              |
| 前端        | `frontend/`（原 mcphub-origin/frontend 的副本，有改造）   |
| 数据存储    | SQLite（`$APPDATA/mcphub.db`，通过 sqlx 0.8）             |
| 认证        | jsonwebtoken 9 + bcrypt 0.15，密钥存 OS 钥匙串(keyring 3) |
| 异步运行时  | tokio 1 full                                              |
| HTTP 客户端 | reqwest 0.12 (rustls-tls + stream + json)                 |
| 应用标识    | `app.mcphub.desktop`                                      |

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
│   │   ├── 0005_default_skip_auth.sql  # 🆕 桌面端：默认开启免登录
│   │   └── 0006_openapi_column.sql    # 🆕 servers 表添加 openapi 列
│   └── src/
│       ├── main.rs
│       ├── lib.rs              # 应用核心：插件注册、setup hook、invoke_handler
│       ├── auth/
│       │   └── mod.rs          # JWT + bcrypt + guest token 签发
│       ├── db/
│       │   ├── mod.rs          # SQLite 连接池 + 初始化入口
│       │   └── migration.rs    # 🆕 版本化 DB 迁移管理模块
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
│       │   ├── openapi_transport.rs # 🆕 OpenAPI → MCP 传输（spawn rmcp-openapi）
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

> ⚠️ **核心原则（MUST FOLLOW）**：本项目是**开源项目**，签名密钥**直接明文存储在仓库中**（`src-tauri/updater/mcphub.key`），
> **不使用 GitHub Secrets**。所有密钥相关配置均通过仓库文件完成，无需配置任何 GitHub Secret。

**生成签名密钥**：

```bash
bash scripts/generate-signing-key.sh
```

**配置步骤**：

1. 运行脚本生成密钥对（`~/.tauri/mcphub.key` 和 `~/.tauri/mcphub.key.pub`）
2. 将私钥以 base64 编码存入 `src-tauri/updater/mcphub.key`（脚本自动完成）
3. 将公钥以 base64 编码存入 `src-tauri/updater/mcphub.key.pub`（脚本自动完成）
4. 将公钥内容复制到 `src-tauri/tauri.conf.json` 的 `plugins.updater.pubkey` 字段
5. 将私钥和公钥文件提交到仓库（**不需要配置 GitHub Secrets**）

> ⚠️ **密钥存储格式**：`src-tauri/updater/mcphub.key` 文件以 **base64 编码**存储私钥内容（以 `dW50cnVzdGVk...` 开头），
> 而 Tauri signer 期望 `TAURI_SIGNING_PRIVATE_KEY` 环境变量是**原始格式**（以 `untrusted comment:` 开头的两行文本）。
> release.yml 中已使用 Python 脚本在 CI 中自动解码 base64 后设置环境变量，无需手动处理。

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


| 平台          | Runner           | Target                    | 架构  |
| ------------- | ---------------- | ------------------------- | ----- |
| macOS ARM64   | macos-14         | aarch64-apple-darwin      | arm64 |
| macOS x64     | macos-14         | x86_64-apple-darwin       | x64   |
| Linux x64     | ubuntu-22.04     | x86_64-unknown-linux-gnu  | x64   |
| Linux ARM64   | ubuntu-22.04-arm | aarch64-unknown-linux-gnu | arm64 |
| Windows x64   | windows-latest   | x86_64-pc-windows-msvc    | x64   |
| Windows ARM64 | windows-latest   | aarch64-pc-windows-msvc   | arm64 |

**关键步骤**：

1. 安装 Node.js 22 + Rust stable + 目标 triple
2. 安装系统依赖（Linux: webkit2gtk, appindicator3, rsvg2, patchelf, ssl）
3. 下载 bundled runtimes（Node.js + uv + Python）
4. 解码签名私钥（base64 → 原始格式，**必须 strip 尾部空白**）
5. 验证签名密钥格式（必须以 `untrusted comment:` 开头）
6. 构建 Tauri 应用（使用私钥签名，`createUpdaterArtifacts: true`）
7. 调试：列出构建产物，检查 `.sig` 文件是否生成
8. 收集平台产物并重命名为统一格式 `mcphub-desktop-{platform-tag}.{ext}`
9. 生成 `latest.json`（Python 脚本解析 .sig 文件，验证非空）
10. 创建 draft Release 并上传所有文件

> ⚠️ **bundles 配置（MUST GET RIGHT）**：
>
> - **macOS**: bundles 必须为 `app,dmg`（不能只写 `dmg`）。`dmg` 只生成安装包，**不会**生成 updater 产物（`.app.tar.gz` + `.app.tar.gz.sig`）。必须加 `app` 目标。
> - **Windows**: bundles 必须包含 `nsis`，才会生成 `.nsis.zip` + `.nsis.zip.sig`。
> - **Linux**: `deb,rpm` 即可，Linux 不支持自动更新（无 AppImage）。
> - 如果 bundles 配置错误，Tauri 会输出警告：`The bundler was configured to create updater artifacts but no updater-enabled targets were built`，且 `.sig` 文件不会生成。

**产物说明**：


| 平台    | bundles 配置 | 安装包     | 更新包      | 签名文件        | 备注                          |
| ------- | ------------ | ---------- | ----------- | --------------- | ----------------------------- |
| macOS   | `app,dmg`    | .dmg       | .app.tar.gz | .app.tar.gz.sig | 支持自动更新                  |
| Linux   | `deb,rpm`    | .deb, .rpm | 无          | 无              | 不支持自动更新（无 AppImage） |
| Windows | `nsis,msi`   | .exe, .msi | .nsis.zip   | .nsis.zip.sig   | 支持自动更新                  |

#### 3.4.4 latest.json 格式

> ⚠️ `latest.json` 由 CI 在 release job 中自动生成，**不需要手动维护**。
> 仓库中的 `src-tauri/updater/latest.json` 仅作占位参考，实际更新检查使用 GitHub Release 上的版本。

```json
{
  "version": "1.0.17",
  "notes": "MCPHub Desktop 1.0.17\n\nSee release page for full changelog.",
  "pub_date": "2026-06-18T12:00:00Z",
  "platforms": {
    "darwin-aarch64": {
      "signature": "dW50cnVzdGVkIGNvbW1lbnQ6...",
      "url": "https://github.com/skrstop/MCPHub-Desktop/releases/download/v1.0.17/mcphub-desktop-macos-arm64.app.tar.gz"
    },
    "darwin-x86_64": {
      "signature": "...",
      "url": "https://github.com/skrstop/MCPHub-Desktop/releases/download/v1.0.17/mcphub-desktop-macos-x64.app.tar.gz"
    },
    "windows-x86_64": {
      "signature": "...",
      "url": "https://github.com/skrstop/MCPHub-Desktop/releases/download/v1.0.17/mcphub-desktop-windows-x64.nsis.zip"
    },
    "windows-aarch64": {
      "signature": "...",
      "url": "https://github.com/skrstop/MCPHub-Desktop/releases/download/v1.0.17/mcphub-desktop-windows-arm64.nsis.zip"
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

#### 3.4.5 更新检查与 Linux 回退机制

**文件**：`frontend/src/utils/version.ts`（⚠️ 本地修改）

桌面端更新检查逻辑：

1. **macOS / Windows**：使用 Tauri updater 插件（`check()`），支持自动下载安装
2. **Linux（deb/rpm）**：Tauri updater 不支持自动更新，回退到检查 GitHub `latest.json` 版本号，提示用户手动下载

**`UpdateInfo` 接口新增字段**：

- `canAutoUpdate: boolean` — 当前平台是否支持自动更新（macOS/Windows=true, Linux=false）
- `downloadUrl: string` — 手动下载链接（Linux 使用 GitHub Releases 页面）

**文件**：`frontend/src/components/ui/AboutDialog.tsx`（⚠️ 本地修改）

- 当 `canAutoUpdate=true` 时显示"安装更新"按钮（macOS/Windows）
- 当 `canAutoUpdate=false` 时显示"下载更新"链接（Linux），跳转到 GitHub Releases

**文件**：`frontend/src/utils/tauriClient.ts`

Changelog API 在桌面端被拦截返回空数据，更新检查完全由 `version.ts` 处理。

**i18n 新增翻译键**：

- `about.downloadManual` — "Download Update" / "下载更新" / "Télécharger la mise à jour" / "Güncellemeyi İndir"

#### 3.4.6 故障排除

**问题：updater 无法验证签名**

- 原因：公钥配置错误或私钥不匹配
- 解决：确认 `tauri.conf.json` 中的 `pubkey` 与 `src-tauri/updater/mcphub.key.pub` 中的公钥一致，确认仓库中的私钥与公钥配对

**问题：CI 构建 .sig 签名文件不生成（latest.json platforms 为空）**

- 原因：`src-tauri/updater/mcphub.key` 文件以 **base64 编码**存储私钥，但 `TAURI_SIGNING_PRIVATE_KEY` 环境变量需要原始格式（以 `untrusted comment:` 开头的两行文本）。解码后密钥末尾可能有多余的空白/换行符，导致 Tauri signer 无法解析密钥，跳过签名步骤，.sig 文件不会生成。
- 解决：release.yml 中使用 Python 脚本将 base64 编码的密钥解码后 **必须 `.strip()` 去除尾部空白**，再设置到 `TAURI_SIGNING_PRIVATE_KEY` 环境变量（通过 `GITHUB_ENV` 多行写入）。同时添加了验证步骤确认密钥格式正确。

**问题：CI 构建失败**

- 原因：签名密钥文件缺失或格式错误
- 解决：确认 `src-tauri/updater/mcphub.key` 文件存在于仓库中且为有效的 base64 编码私钥。本项目**不使用 GitHub Secrets**，签名密钥直接存储在仓库中。

**问题：用户无法收到更新**

- 原因：`latest.json` 文件不存在或格式错误
- 解决：检查 GitHub Release 是否包含 `latest.json` 文件，确认格式正确

**问题：Windows CI 构建 Decode signing key 步骤报 UnicodeEncodeError**

- 原因：Windows runner 上 Python 默认使用 cp1252 编码，无法输出 `✅`（U+2705）等 Unicode 字符，导致 `print()` 抛出 `UnicodeEncodeError: 'charmap' codec can't encode character '\u2705'`
- 解决：在 `build` job 级别添加 `env: PYTHONIOENCODING: utf-8`，确保所有步骤中 Python 使用 UTF-8 编码输出

**问题：构建矩阵只构建了一个平台**

- 原因：release.yml 中其他平台被注释掉了
- 解决：确保所有 6 个平台（macOS ARM64/x64、Linux x64/ARM64、Windows x64/ARM64）都未被注释

**详细文档**：参见 `doc/SIGNING_SETUP.md`

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

#### 3.5.5 DB 版本化迁移管理

**文件**：`src-tauri/src/db/migration.rs`

##### 设计目标

替代 `sqlx::migrate!()` 宏，实现可控的版本化数据库迁移管理：

- 使用 `schema_version` 表跟踪当前 DB 版本号
- 每个迁移是独立的异步函数，按版本号顺序执行
- 自动兼容旧版 `sqlx::migrate!()` 系统（检测 `_sqlx_migrations` 表）
- 启动时只执行缺失的迁移，幂等安全

##### 核心结构

```rust
// src-tauri/src/db/migration.rs

pub const TARGET_VERSION: i64 = 6; // 当前最新 schema 版本，每次新增迁移递增

/// 启动时调用，检测当前版本并执行所有缺失的迁移
pub async fn run_pending(pool: &SqlitePool) -> Result<()>

/// 获取当前 DB 版本（从 schema_version 表读取）
async fn get_current_version(pool: &SqlitePool) -> Result<i64>

/// 更新 schema_version 表
async fn set_version(pool: &SqlitePool, version: i64) -> Result<()>

/// 按版本号分发到对应的迁移函数
async fn apply_migration(pool: &SqlitePool, version: i64) -> Result<()>
```

##### 迁移函数命名规范

```rust
/// v{N-1} → v{N}: 迁移描述
async fn migrate_v{N}(pool: &SqlitePool) -> Result<()> { ... }
```

##### 当前迁移版本映射


| 版本 | 函数         | 对应旧 migration 文件        | 说明                                                                                                                         |
| ---- | ------------ | ---------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| v1   | `migrate_v1` | `0001_initial.sql`           | 初始 schema（users, servers, groups, system_config, bearer_keys, activity_log, app_log, builtin_prompts, builtin_resources） |
| v2   | `migrate_v2` | `0002_schema_fix.sql`        | schema 修复（mcprouter 字段, templates, server_tool_config）                                                                 |
| v3   | `migrate_v3` | `0003_config_json.sql`       | system_config 合并为 config_json                                                                                             |
| v4   | `migrate_v4` | `0004_default_admin.sql`     | 默认 admin 用户                                                                                                              |
| v5   | `migrate_v5` | `0005_default_skip_auth.sql` | 默认免登录                                                                                                                   |
| v6   | `migrate_v6` | `0006_openapi_column.sql`    | servers 表添加 openapi 列                                                                                                    |

##### 新增迁移步骤（MUST FOLLOW）

1. 在 `migration.rs` 中递增 `TARGET_VERSION`
2. 新增 `async fn migrate_v{N}(pool: &SqlitePool) -> Result<()>` 函数
3. 在 `apply_migration` 的 match 中添加 `N => migrate_v{N}(pool).await` 分支
4. 同步新增对应的 `migrations/000N_xxx.sql` 文件（供 `sqlx::migrate!` 兼容）
5. 更新本章节的版本映射表

##### 兼容性处理

- **旧版 → 新版**：`get_current_version()` 检测 `_sqlx_migrations` 表，自动初始化 `schema_version` 到对应版本
- **新版 → 旧版降级**：旧代码不引用新列，`schema_version` 表保留但旧代码忽略
- **全新安装**：`schema_version = 0`，执行全部迁移

##### 调用入口

```rust
// src-tauri/src/db/mod.rs
pub mod migration;

pub async fn initialize(app: &AppHandle) -> Result<()> {
    // ...
    migration::run_pending(&pool).await?;
    // ...
}
```

##### 与 server_service.rs 的关系

迁移完成后，`server_service.rs` 中的所有 SQL 查询可以直接引用所有列（包括 `openapi`），**不需要运行时列检测**。迁移保证了 schema 的完整性。

#### 3.5.6 OpenAPI 传输层

**文件**：`src-tauri/src/mcp/openapi_transport.rs`

- 使用 `rmcp-openapi` v0.31 作为**库**（非子进程）集成
- 实现 `McpTransport` trait，通过 `rmcp_openapi::Server` 解析 OpenAPI spec 并生成 MCP tools
- 支持两种 spec 输入模式：
  - **URL 模式**：`openapi.url` — 通过 HTTP 获取 spec JSON
  - **Schema 模式**：`openapi.schema` — 内联 JSON 直接使用
- `rmcp-openapi` 内部处理 HTTP 调用（使用 reqwest v0.13）

**已知限制**：

- `reqwest` 版本不兼容：项目用 v0.12，`rmcp-openapi` 用 v0.13，`HeaderMap` 类型不同
- 自定义 headers 无法透传到 `rmcp-openapi` 的 HTTP 客户端（类型不匹配）
- 认证应通过 OpenAPI spec 的 security schemes 配置，而非自定义 headers

**认证支持**：

- `rmcp-openapi` 原生支持 OpenAPI spec 中定义的 security schemes（apiKey, http, oauth2, openIdConnect）
- 前端配置的 `openapi.security` 映射到 `OpenApiSecurity` 模型，但当前未传递给 `rmcp-openapi`（待后续集成 `AuthorizationMode`）

**模型定义**：`src-tauri/src/models/server.rs` 新增 `OpenApiConfig`, `OpenApiSecurity` 等结构体

**数据库**：`servers.openapi` 列（JSON TEXT），由 `migrate_v6` 创建

#### 3.5.7 内置 HTTP 服务器[](https://)

**文件**：`src-tauri/src/services/http_server.rs`

- 使用 Axum 框架实现内置 HTTP 服务器
- 支持 MCP Streamable HTTP 协议（JSON-RPC 2.0）
- 支持 Bearer Key 认证
- 支持 Smart 路由（`/mcp`, `/mcp/$smart`, `/mcp/$smart/{group}`）
- 支持分组路由（`/mcp/{group}`）
- 支持单服务器路由（`/mcp/{server}`）

#### 3.5.8 日志自动清理

**文件**：`src-tauri/src/services/log_service.rs`、`src-tauri/src/lib.rs`

- 保留最近 **15 天** 的 `app_log` 和 `activity_log` 记录
- 清理后自动执行 `VACUUM` 瘦身数据库
- 手动清理（UI 按钮）也会执行 `VACUUM`

**触发时机：**


| 时机      | 说明                                  |
| --------- | ------------------------------------- |
| 每 6 小时 | 后台定时任务自动清理，首次延迟 5 分钟 |
| 手动触发  | 系统日志/活动管理页面的清除按钮       |

**清理 SQL：**

```sql
DELETE FROM app_log WHERE created_at < datetime('now', '-15 days');
DELETE FROM activity_log WHERE timestamp < datetime('now', '-15 days');
VACUUM;
```

**DB 迁移版本：**

- `TARGET_VERSION = 7`
- `0007_activity_source_ip.sql`：activity_log 添加 `source_ip` 列

#### 3.5.9 活动管理 UI 定制

**文件**：`frontend/src/pages/ActivityPage.tsx`

- **隐藏"来源用户"列** — 桌面端不需要用户追踪，已从列表和详情弹窗中移除
- **活动日志记录客户端 IP** — HTTP 端点调用时从 `x-forwarded-for` / `x-real-ip` 提取 IP 写入 `source_ip` 列
- **工具禁用状态同步** — `Tool` 模型添加 `enabled` 字段，`list_servers`/`get_server` 返回完整工具列表含启用状态，禁用工具在 HTTP 端点 `tools/list` 中不暴露、`tools/call` 中拒绝调用

#### 3.5.10 上下文占用（Context Footprint）

**文件**：`src-tauri/src/commands/cost.rs`、`frontend/src/utils/tauriClient.ts`

- 实现后端 `get_server_costs` / `get_group_costs` 命令
- 基于工具描述和输入 schema 估算 token 数（约 4 字符 = 1 token）
- `exposed` = 已启用项 token 总和，`gross` = 所有项 token 总和
- 禁用服务器显示 `0/{gross}`，不再显示 `—`

---

## 4. 上游 mcphub-origin 同步记录

### 4.1 同步策略

1. `mcphub-origin/` 是 git 子模块，仅作为代码参考与 diff 来源，**桌面端永远不直接修改子模块内容**。
2. 桌面端 `frontend/`、`locales/` 是 origin 对应目录的**有改造副本**：
   - 大部分文件保持与 origin 一致；
   - desktop 主动改造的文件（见第 3 节）保留差异，**同步时需手动合并**。
3. 后端由 Rust 重写在 `src-tauri/`，**Node 后端代码不直接同步**，但需评估安全相关 fix 是否要在 Rust 端镜像实现。
4. `package.json`、`pnpm-lock.yaml`、`docs/`、`Dockerfile`、`docker-compose*.yml` 等部署/文档文件**不同步**。

### 4.2 同步规则（MUST FOLLOW）

> ⚠️ **核心原则：禁止直接覆盖文件，必须逐文件检查差异后合并。**

#### 同步前检查清单

1. **识别桌面端自定义文件**：第 3 节列出的所有文件（标记为 ⚠️ 或 🆕 的）**绝对不能直接覆盖**
2. **逐文件对比**：对每个待同步文件，执行 `diff desktop-file origin-file` 确认差异来源
3. **分类处理**：
   - 桌面端无自定义修改的文件 → 可直接覆盖
   - 桌面端有自定义修改的文件 → 必须手动合并，保留桌面端差异
   - locales/*.json → 必须保留桌面端新增的 runtime* 翻译键

#### 桌面端自定义文件清单（同步时不可覆盖）


| 文件                                             | 自定义内容                                                |
| ------------------------------------------------ | --------------------------------------------------------- |
| `frontend/src/components/ServerCard.tsx`         | 移除 sponsor/wechat/discord、样式调整                     |
| `frontend/src/components/ServerForm.tsx`         | hub-* 样式、隐藏 visibility、保留 OAuth2                  |
| `frontend/src/components/LogViewer.tsx`          | source 类型改为 string[]、source filter UI 移除、滚动方向 |
| `frontend/src/components/layout/Header.tsx`      | GitHub 链接、移除文档按钮                                 |
| `frontend/src/components/layout/Sidebar.tsx`     | Logo 使用应用图标                                         |
| `frontend/src/components/ui/UserProfileMenu.tsx` | 移除 sponsor/wechat/discord 按钮                          |
| `frontend/src/components/ui/AboutDialog.tsx`     | MCPHub Desktop 标识、canAutoUpdate 逻辑                   |
| `frontend/src/contexts/AuthContext.tsx`          | skipAuth/guest 模式                                       |
| `frontend/src/contexts/SettingsContext.tsx`      | httpPort/exposeHttp 字段                                  |
| `frontend/src/services/configService.ts`         | getPublicConfig 使用 apiGet                               |
| `frontend/src/services/changelogService.ts`      | Tauri 中禁用                                              |
| `frontend/src/pages/SettingsPage.tsx`            | 隐藏未实现模块、RuntimeVersionManager、HTTP 端口          |
| `frontend/src/pages/LoginPage.tsx`               | admin 默认填充、密码提示、Logo 图标                       |
| `frontend/src/pages/Dashboard.tsx`               | 隐藏 SMART/Docs                                           |
| `frontend/src/pages/ActivityPage.tsx`            | 隐藏用户列、timestamp UTC 转换                            |
| `frontend/src/utils/tauriClient.ts`              | 桌面端新增                                                |
| `frontend/src/utils/fetchInterceptor.ts`         | isTauri() 拦截                                            |
| `frontend/src/utils/runtime.ts`                  | 运行时配置                                                |
| `locales/*.json`                                 | runtime* 翻译键（~18 个）                                 |

#### 同步后验证清单

1. `cd frontend && npm run build` — 前端构建通过
2. `cd src-tauri && cargo check` — Rust 编译通过
3. 检查 `locales/*.json` 中 runtime* 翻译键是否完整
4. 检查桌面端自定义文件未被覆盖（抽查关键文件的 diff）

### 4.3 同步操作流程（标准 SOP）

```bash
# 1. 更新 origin 子模块到 latest main
cd mcphub-origin && git fetch origin && git checkout origin/main && cd ..

# 2. 列出待同步提交（基线 = 上次记录的 commit）
cd mcphub-origin && git --no-pager log --oneline <last-sync-sha>..HEAD

# 3. 生成 frontend + locales 综合 patch
git --no-pager diff <last-sync-sha>..HEAD -- frontend/ locales/ > /tmp/origin_frontend.patch

# 4. dry-run 检查冲突
cd .. && patch -p1 --dry-run --batch --forward --no-backup-if-mismatch -F 5 < /tmp/origin_frontend.patch

# 5. ⚠️ 逐文件处理（禁止批量覆盖！）
#    - 对桌面端无自定义的文件：直接 cp 覆盖
#    - 对桌面端有自定义的文件：手动合并，保留桌面端差异
#    - 对 locales/*.json：只添加新增键值，不删除桌面端 runtime* 键

# 6. 评估 Node 后端 commit，决定是否在 Rust 端镜像实现

# 7. 验证
cd frontend && npm run build
cd src-tauri && cargo check

# 8. 更新本章节「最近同步基线」与「同步条目」
```

### 4.3 最近同步基线


| 项                             | 值                      |
| ------------------------------ | ----------------------- |
| **当前已同步到 origin commit** | `89deccd` (origin/main) |
| **对应 origin tag**            | `v0.12.15+11`           |
| **桌面端版本号**               | `1.0.18002`             |
| **同步执行日期**               | 2026-06-24              |

> 下次同步时，使用 `89deccd` 作为新的基线 SHA 起点（命令：`cd mcphub-origin && git --no-pager log --oneline 89deccd..HEAD`）。

### 4.4 最近同步记录

#### 2026-06-24：同步 `96c16d9` → `89deccd`（3 个 commit）

**已同步到 desktop（前端 / locales / Rust 后端）**


| 来源 commit | 说明                                                               | desktop 应用方式                                                                                          |
| ----------- | ------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------- |
| `75e497f`   | feat: introduce group server alias                                 | 前端：GroupCard.tsx, ServerToolConfig.tsx, types/index.ts 直接覆盖；locales 四语言翻译添加 alias 相关键值 |
| `89deccd`   | fix: record system-key user in activity logs and reserve usernames | Rust 后端：user_service.rs 添加保留用户名检查（system/admin/guest/root）；修复时间使用本地时间            |

**未同步（后端 / 不适用）**


| 来源 commit | 类型                            | 处理决策               |
| ----------- | ------------------------------- | ---------------------- |
| `537f393`   | chore: resolve pnpm audit vulns | **不同步**（依赖更新） |

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

// ✅ DB 迁移：使用 db::migration 模块（版本化管理），不再使用 sqlx::migrate!()
// 见 3.5.5 节
migration::run_pending(&pool).await?;
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

- [X]  基础架构（Tauri + SQLite + MCP 传输层）
- [X]  所有 Tauri 命令（auth, servers, groups, tools, users, config, logs）
- [X]  前端适配器（tauriClient.ts + fetchInterceptor.ts）
- [X]  系统托盘
- [X]  免登录模式（guest 模式）
- [X]  运行时版本管理（Node.js/Python）
- [X]  内置 HTTP 服务器（expose_http 模式）
- [X]  Bearer Keys 管理
- [X]  Builtin Prompts/Resources
- [X]  Activity Log
- [X]  Market（本地 MCP 市场）
- [X]  Registry Proxy
- [X]  Cloud Proxy（MCPRouter）
- [X]  SSE 传输改进
- [X]  DB 版本化迁移管理（schema_version + 迁移函数）
- [X]  OpenAPI 传输层（rmcp-openapi stdio 模式）
- [X]  MCP 服务器启动中状态（starting → connecting）
- [X]  日志自动清理（15 天保留 + VACUUM 瘦身）
- [X]  活动管理 UI 定制（隐藏用户列、记录客户端 IP）
- [X]  工具禁用状态同步（enabled 字段 + HTTP 端点过滤）
- [X]  上下文占用（Context Footprint）计算
- [X]  系统日志面板（app_logger 写入 DB + 轮询刷新）

### 待办

- [ ]  Smart Routing（智能路由）
- [ ]  OAuth Server
- [ ]  Better Auth 集成
- [ ]  Tool Result Compression
- [ ]  MCPB/DXT 文件安装
- [ ]  Templates（配置模板）
- [ ]  CI/CD 打包配置
