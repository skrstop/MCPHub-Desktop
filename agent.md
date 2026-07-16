# [](https://)[](https://)[](https://)MCPHub Desktop (Tauri) — Agent 开发文档

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

#### 3.2.8 Splash 加载画面

**文件**：`frontend/index.html`、`frontend/src/main.tsx`

- 在 `index.html` 中内嵌 CSS 动画的加载画面（Spinner + 文字），在 WebView 加载时立即显示
- **加载文字使用内联 `<script>` 实现国际化**（不依赖 React/i18next）：
  - 通过 `navigator.language` 检测浏览器语言
  - 支持 zh（正在加载中…）、en（Loading…）、fr（Chargement…）、tr（Yükleniyor…）
  - 默认回退到英文
- React 挂载后，`main.tsx` 中的 `removeSplash()` 函数添加 `fade-out` CSS 类实现 300ms 淡出动画后移除 DOM 元素
- 桌面端的 `index.html` 已加入「自定义文件清单」（同步时不可覆盖）

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

#### 3.5.11 Windows 打包定制

**文件**：`src-tauri/tauri.conf.json`、`src-tauri/src/mcp/stdio_transport.rs`、`src-tauri/src/services/runtime_env.rs`、`src-tauri/src/commands/runtime.rs`、`scripts/download-runtimes.sh`、`scripts/download-runtimes.ps1`

##### NSIS 安装路径选择

`tauri.conf.json` 中配置了 `installMode: "both"`，允许用户在安装时选择：

- **当前用户（AppData）**：`%LOCALAPPDATA%\MCPHub Desktop`，无需管理员权限
- **所有用户（Program Files）**：`C:\Program Files\MCPHub Desktop`，需要管理员权限

```json
{
  "bundle": {
    "windows": {
      "nsis": {
        "installMode": "both"
      }
    }
  }
}
```

##### Windows 静默进程执行（CREATE_NO_WINDOW）

**问题**：Windows 上每次执行 shell 命令（`powershell`、`node`、`python`、`taskkill` 等）都会弹出黑色 CMD 窗口并瞬间关闭，用户体验极差。

**解决方案**：在所有 `std::process::Command` 和 `tokio::process::Command` 调用中，对 Windows 平台添加 `creation_flags(0x0800_0000)`（`CREATE_NO_WINDOW` 标志）。

该标志保留 stdio 管道（stdin/stdout/stderr）但阻止创建可见的控制台窗口。

**已修改的文件和位置**：


| 文件                      | 函数/位置                         | 命令                   |
| ------------------------- | --------------------------------- | ---------------------- |
| `mcp/stdio_transport.rs`  | `connect()`                       | MCP 服务器子进程       |
| `mcp/stdio_transport.rs`  | `kill_process_tree()`             | `taskkill`             |
| `services/runtime_env.rs` | `get_windows_path()`              | `powershell` 获取 PATH |
| `commands/runtime.rs`     | `install_python_version()`        | `uv python install`    |
| `commands/runtime.rs`     | `uninstall_python_version()`      | `uv python uninstall`  |
| `commands/runtime.rs`     | `detect_system_node_version()`    | `node -v`              |
| `commands/runtime.rs`     | `detect_bundled_node_version()`   | 捆绑的`node -v`        |
| `commands/runtime.rs`     | `detect_system_python_version()`  | `python --version`     |
| `commands/runtime.rs`     | `node_version_installed()`        | 捆绑的`node -v`        |
| `commands/runtime.rs`     | `get_installed_python_versions()` | `uv python list`       |
| `commands/runtime.rs`     | `verify_node_version()`           | `node -v`              |
| `commands/runtime.rs`     | `verify_python_executable()`      | `python --version`     |
| `commands/runtime.rs`     | `get_windows_path()`              | `powershell` 获取 PATH |

**代码模式**：

```rust
// 同步进程 — 需要导入 std::os::windows::process::CommandExt
use std::os::windows::process::CommandExt; // #[cfg(windows)]
let mut c = std::process::Command::new("powershell");
c.args(["-NoProfile", "-Command", "..."])
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::null());
#[cfg(windows)]
{ c.creation_flags(0x0800_0000); } // CREATE_NO_WINDOW
let output = c.output()?;

// 异步进程（tokio）— 同样使用 std 的 CommandExt，tokio::process::Command 通过 Deref 继承
use std::os::windows::process::CommandExt; // #[cfg(windows)]
let mut c = tokio::process::Command::new(&uv);
c.args(["python", "install", &version])
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
#[cfg(windows)]
{ c.creation_flags(0x0800_0000); } // CREATE_NO_WINDOW
let child = c.spawn()?;
```

> ⚠️ **注意**：
>
> - `creation_flags` 方法仅在 Windows 上存在，必须使用 `#[cfg(windows)]` 条件编译，否则其他平台编译会失败。
> - `std::process::Command` 需要导入 `std::os::windows::process::CommandExt`
> - `tokio::process::Command` 通过 `Deref<Target=std::process::Command>` 继承了 `creation_flags`，所以只需导入 `std::os::windows::process::CommandExt`（`tokio::os` 模块是私有的，不能直接导入）

##### Python 运行时版本

**文件**：`scripts/download-runtimes.sh`、`scripts/download-runtimes.ps1`

捆绑的 Python 版本已更新为 `3.14`（最新稳定版），Node.js 更新为 `24.18.0`，uv 更新为 `0.11.24`。
详见 `scripts/download-runtimes.sh` 和 `scripts/download-runtimes.ps1` 中的默认版本配置。

### 3.6 stdio 包下载进度 / 更新检测 / 非阻塞连接（桌面端独有）

> ⚠️ **基线同步注意**：本节涉及的全部文件都带桌面端自定义，同步 origin 时**禁止批量覆盖**，必须手动合并保留以下差异。origin（Node.js）无对应实现。

#### 3.6.1 非阻塞保存/连接（保存类命令不再被连接阻塞）

**背景**：`pool::connect_server` 对 npx/uvx 会触发包下载、对不可达 sse/http 会重试 3×120s，若在保存命令里 `await` 它，前端保存按钮会卡死数分钟。

**文件**：`src-tauri/src/commands/servers.rs`
- `add_server`：原本就是后台 spawn 连接（参考范式）。
- `update_server`：**持久化后改为 `tauri::async_runtime::spawn` 后台连接**，立即返回 `starting` 状态（不再 `await connect_server`）。保存响应不再被连接阻塞。
- `reinstall_server`：清缓存后后台 spawn 重连，立即返回 `{success, cleared}`。
- `reload_server` 命令：`mcp_manager::reload_server` 非阻塞后，`get_status` 用 `starting` 兜底（防占位插入竞态）。

**文件**：`src-tauri/src/services/mcp_manager.rs`
- `reload_server`：后台 spawn 连接（不再 await）。
- `toggle_server`：enable 分支后台 spawn 连接；disable 分支保持原 `is_starting` 竞态保护。
- `start_all`：原本就 staggered spawn；其 `app: &AppHandle` 参数现用于注入全局事件句柄。

#### 3.6.2 stdio 下载进度事件（`server://install-progress`）

**文件**：`src-tauri/src/mcp/progress.rs`（新文件）
- 全局 `AppHandle`（`OnceLock`）：`set_app_handle` / `app_handle`，在 `lib.rs` setup 早期注入，避免给 `connect_server` 等所有调用方加参数。
- `ServerInstallProgress { server, phase, progress: Option<u8>, message }`，`phase` ∈ `downloading | done | error`。
- `emit_install_progress(payload)` 发 `server://install-progress`。
- `is_package_manager(command)`：判断 npx/uvx。

**文件**：`src-tauri/src/mcp/stdio_transport.rs`
- **不**在启动时无条件发 `downloading`（包已缓存时无下载，避免每次启动误报"下载中"）。
- stderr drain 仅在行匹配 `looks_like_download_progress()`（含 `download`/`downloading`/`added...package`/`installed...package` 或带百分比/`X/Y`）时才发 `downloading`，节流 300ms。服务自身输出到 stderr 的信息日志不算下载进度。
- `parse_progress_pct(line)`：从 stderr 行解析 `NN%` 或 `X/Y` 百分比。
- 握手 `initialize` 返回后捕获 `serverInfo.version` 存入 `self.server_version`。

**文件**：`src-tauri/src/mcp/client.rs`
- `McpTransport` trait 新增 `fn server_version(&self) -> Option<String> { None }`（默认实现）；`McpClient` 透传。

**文件**：`src-tauri/src/mcp/pool.rs`（`connect_server`）
- 成功分支：若 npx/uvx，发 `done`（progress=100），并 `spawn_update_check`。
- 失败/超时/build_client 错误分支：若 npx/uvx，发 `error`。

#### 3.6.3 包更新检测（仅在启动/连接时检查，非定时巡检）

**文件**：`src-tauri/src/mcp/progress.rs`
- `ServerUpdateInfo { server, hasUpdate, current, latest }`，发 `server://update-available`。⚠️ **必须带 `#[serde(rename_all = "camelCase")]`**，否则 `has_update` 序列化成蛇形、前端读 `hasUpdate` 永远 `undefined`（曾踩坑）。
- `spawn_update_check(server, command, args, running_version)`：连接成功后后台 spawn。
  - **不**用 `serverInfo.version`（`running_version`）做对比——服务自报版本与包版本常不同号（如 mcp-server-tapd 自报 `1.28.1`、PyPI 包 `8.0.79`），无可比性，仅用于日志。
  - 改为对比**持久化的"已安装包版本"**：`get_recorded_version` / `set_recorded_version`（存于 `system_config.config_json` 的 `packageVersions` map，`config_service::update` 深合并）。
  - 规则：无记录→记录当前最新、`hasUpdate=false`；`is_newer(latest, recorded)`→`hasUpdate=true`；否则 `hasUpdate=false`。
  - `mark_reinstalled(server)` / `take_reinstalled(server)`：内存 `Mutex<HashSet>` 标记"刚重装"。`reinstall_server` 调 `mark_reinstalled`，下次检查直接把最新版记为已安装、`hasUpdate=false`——**更新后不再重复提示，重启后也不会**（已持久化）。
  - `extract_package_name(command, args)`：npx 取首个非 flag 参数并剥 `@version`；uvx 取 `--from` 或首个位置参数。
  - `fetch_latest_version(command, pkg)`：npx→`registry.npmjs.org/<pkg>/latest`；uvx→`pypi.org/pypi/<pkg>/json`。带 8s 超时。
  - `is_newer(latest, current)`：自研轻量 semver 比较（解析 `major.minor.patch`，任一解析失败返回 false，不误报）。
- 所有结果都 `app_logger::log_to_db`（日志页可见）：`开始检查` / `检测到新版本：已安装 X，最新 Y` / `已是最新版本` / `首次记录包版本` / `更新完成，已记录已安装版本` / `更新检查失败`。

**文件**：`src-tauri/src/commands/servers.rs`
- `reinstall_server`：重连前调 `crate::mcp::progress::mark_reinstalled(&cfg.name)`。

**检查时机**：仅在 `connect_server` 成功后、对 npx/uvx 跑一次。触发点：启动 `start_all`、`add_server`、`update_server`、`toggle_server`、`reload_server`、`reinstall_server`、`enableSessionRebuild` 的 30s 重连。**不是定时巡检**。

#### 3.6.4 前端联动

**文件**：`frontend/src/contexts/ServerInstallProgressContext.tsx`（新文件）
- 监听 `server://install-progress` 与 `server://update-available`（`isTauri()` 时）。
- `progress`（按 server 存，`done/error` 1.5s 后清）、`updates`（按 server 存最新检查结果，含 `hasUpdate`/`current`/`latest`）。
- 暴露 `getProgress` / `getUpdate` / `isInstalling` / `dismissUpdate`。
- **已移除 `dismissed` 集合**：后端 `mark_reinstalled` 已能正确清角标，dismissed 会误杀真更新（如已记录版本被回退后同版本本应再提示）。

**文件**：`frontend/src/App.tsx`
- 在 `ServerProvider` 内包 `ServerInstallProgressProvider`。

**文件**：`frontend/src/components/ServerCard.tsx`
- 状态格：下载中显示紧凑进度条（`下载中 NN%` 或 indeterminate），不再与 transport 格重叠。
- "..." 按钮：有更新时右上角红色圆点角标。
- 菜单：有更新时新增「更新到 vX.Y.Z」项（强调色），点击走 reinstall 确认弹窗。
- 服务名后：npx/uvx 显示小字版本，优先用 `updateInfo.current`（已记录包版本），回退 `server.version`（`serverInfo.version`）。

**文件**：`frontend/src/types/index.ts`
- `Server` 增加 `version?: string`。

**文件**：`frontend/src/utils/tauriClient.ts`
- `toFrontendServer` 把 `status.serverVersion` 映射到 `version`。

**文件**：`src-tauri/src/models/server.rs`
- `ServerStatus` 新增 `server_version: Option<String>`（`#[serde(default, skip_serializing_if = "Option::is_none")]`，序列化为 `serverVersion`）。`pool.rs` 连接成功时填入握手版本，其余构造点填 `None`。

**文件**：`locales/zh.json`、`locales/en.json`
- 新增 `server.downloading`、`server.updateAvailable`、`server.updateTo`、`server.reinstallStarted`。

#### 3.6.5 模拟更新检测（调试用）

已记录版本存于 `~/Library/Application Support/app.mcphub.desktop/mcphub.db` 的 `system_config.config_json` → `packageVersions` map（key=服务名，value=包版本）。直接改 DB 即可模拟"有更新"：

```bash
DB="$HOME/Library/Application Support/app.mcphub.desktop/mcphub.db"
python3 - "$DB" <<'PY'
import sqlite3, json, sys
con = sqlite3.connect(sys.argv[1])
cfg = json.loads(con.execute("SELECT config_json FROM system_config WHERE id=1").fetchone()[0] or "{}")
cfg.setdefault("packageVersions", {})["chrome-devtools"] = "1.2.0"  # 改成低于最新的版本
con.execute("UPDATE system_config SET config_json=?, updated_at=datetime('now') WHERE id=1", (json.dumps(cfg, ensure_ascii=False),))
con.commit(); con.close()
PY
```

改后需**重连该服务**（启用/重载/重启）才会触发检查（"刷新"按钮只轮询、不重连，不触发检查）。

### 3.7 Per-session upstream client isolation（perSessionClient，origin #985 镜像）

> 镜像 origin `d74d1be`（PR #985）。当 server 配置 `perSessionClient: true` 时，HTTP MCP 路径下每个下游 session（`mcp-session-id`）获得**独立的上游 client/连接/进程**，而不是共享 pool 的单 client。适用于 Playwright 等有状态服务。前端 UI 在 `8d2ef15→9dd75bc` 基线同步时已镜像为 dormant（checkbox + config 字段），本节为 Rust 后端真正读取并生效的实现。

#### 3.7.1 作用域（与 origin 一致）
- **仅 HTTP MCP JSON-RPC 路径**（`dispatch_mcp` 的 `tools/call`，http_server.rs）有 session，才做隔离。
- REST 端点（`/rest/:server/call`、`/rest/group/:group/call`）无 session，保持共享 pool。
- Tauri UI 的 `call_tool` 命令（前端直调）无 session，保持共享 pool（本地单用户）。
- `tools/list` 用共享 pool 的缓存工具列表（同服务同工具，无需隔离）。

#### 3.7.2 Model + DB（持久化 per_session_client）
- `models/server.rs`：`ServerConfig` 加 `#[serde(default)] pub per_session_client: Option<bool>`（camelCase → JSON `perSessionClient`，前端已发）。
- `db/migration.rs`：`migrate_v10`（`ALTER TABLE servers ADD COLUMN per_session_client INTEGER NOT NULL DEFAULT 0`，`.ok()` 容错已存在）；`TARGET_VERSION` 9 → 10；`apply_migration` match 加 `10 => migrate_v10`。配套 `migrations/0008_per_session_client.sql`（供 `sqlx::migrate!` 兼容）。
- `services/server_service.rs`：3 个 SELECT 列清单加 `per_session_client`；`create`/`update` 的 INSERT/UPDATE 加列与 bind（`cfg.per_session_client.unwrap_or(false) as i64`）；`map_row` 读 `per_session_client` → `Some(r.try_get::<i64,_>("per_session_client")? != 0)`。
- `services/settings_import.rs`：导入旧 `mcp_settings.json` 时 `ServerConfig` 字面量补 `per_session_client: None`（共享 pool 兜底）。

#### 3.7.3 Pool 缓存标志 + 暴露 build_client
- `mcp/pool.rs`：
  - `PoolEntry` 加 `per_session_client: bool`（`connect_server` 开头从 `cfg.per_session_client.unwrap_or(false)` 设置；所有占位/失败分支也带 `per_session_client`）。
  - `fn build_client(cfg)` 改 `pub(crate)` 供 `session_pool` 复用（共用 stdio/sse/http/openapi 的 transport 构造逻辑，保证隔离 client 与共享 client 行为一致）。
  - 新增 `pub async fn is_per_session_client(name: &str) -> bool`：读 pool 缓存标志，**无 DB 查询**（连接热路径不读 DB）；不在 pool 的服务返回 false（它们 `tools/call` 不可达，隔离路由无意义）。

#### 3.7.4 新增 `mcp/session_pool.rs`（per-session 隔离 client 存储）
- `static SESSION_CLIENTS: OnceLock<RwLock<HashMap<(String,String), Arc<Mutex<McpClient>>>>>`，key = `(session_id, server_name)`。client 包 `Arc<Mutex<McpClient>>`——`disconnect` 是 `&mut self`，cleanup 时需可变借用；同一 session 的调用串行化对有状态服务本就更安全。
- `static CREATE_LOCKS: OnceLock<Mutex<HashMap<SessionKey, Arc<Mutex<()>>>>>`：per-(session,server) 创建锁，仿 origin `isolatedClientCreationLocks`，防并发首调重复创建。
- `pub async fn call_tool_isolated(session_id, server_name, tool, arguments) -> Result<ToolCallResult>`：
  1. 快速路径：读 map 命中 → clone Arc → `run_call`（锁外执行，不阻塞其他 session）。
  2. 未命中：取/建 per-key 创建锁 → `_guard.lock()` 串行 → 双重检查（另一持有者可能刚建好）。
  3. 新建：`server_service::get_by_name` 取 cfg（**仅新建时一次 DB 读**），`pool::build_client(&cfg)?`，`timeout(120s, client.connect())`，缓存 `Arc<Mutex<McpClient>>`，日志「Created isolated client for session X -> Y」。
  4. `run_call` 失败（连接类错误）：从 map 移除、日志「evicted」，下次重建（**基础重连**，不做 origin 的 40x/SSE 细粒度重试）。
- `pub async fn cleanup_session(session_id)`：遍历该 session 所有 client，`disconnect`（stdio 走 `kill_process_tree`，已在 `StdioTransport::disconnect` 内），移除；并清该 session 的 creation locks。disconnect I/O 在写锁释放后做，不阻塞其他 session。
- `mcp/mod.rs`：`pub mod session_pool;`。

#### 3.7.5 HTTP server 路由
- `services/http_server.rs`：
  - `dispatch_mcp`：开头提取 `let session_id = headers.get("mcp-session-id")...`（trimmed、非空）。
  - `tools/call` 站点：`if let Some(ref sid) = session_id { if pool::is_per_session_client(&sn).await { session_pool::call_tool_isolated(sid, &sn, &orig_name, args.clone()).await } else { pool::call_tool(...) } } else { pool::call_tool(...) }`——有 session 且服务标记 per_session → 走隔离；否则共享（行为不变）。
  - DELETE handlers `mcp_root_delete`/`mcp_scope_delete`：签名加 `headers: HeaderMap`，提取 `mcp-session-id`（`extract_session_id` helper），`session_pool::cleanup_session(&sid).await`（有则清理，无则 no-op）。返回 `StatusCode::OK` 不变。

#### 3.7.6 资源/边界
- stdio 隔离 = 每 session 一个独立子进程（成本高，但正是 stateful 服务所需）。`cleanup_session` 时 `StdioTransport::disconnect` 走 `kill_process_tree` 杀整树（含 npx/uvx wrapper 子进程）。
- 不影响既有 pool 的连接/状态/进度事件逻辑（`connect_server` 仅新增 `per_session_client` 字段透传）。
- activity_log 不加 perSessionClient 字段（origin 加了，属次要，跳过保持简单）。
- 编译验证：`cargo check` 通过（rustc 1.96.0）。

#### 3.7.7 手动验证（计划）
- 开 expose_http；配一个 stdio server 勾选「会话级客户端隔离」；用两个外部 MCP 客户端连 `/mcp` 各自 initialize（得不同 session-id）并 call 同一工具 → 应各起独立子进程（`ps` 可见两个进程）。DELETE session 后子进程被清理。
- 回归：不勾选 perSessionClient 的服务，HTTP call_tool 仍走共享 pool，行为不变。

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
| `frontend/src/components/ServerCard.tsx`         | 移除 sponsor/wechat/discord、样式调整；下载进度条 / 更新角标+「更新到」菜单项 / 名字后版本号（见 3.6） |
| `frontend/src/components/ServerForm.tsx`         | hub-* 样式、隐藏 visibility、保留 OAuth2                  |
| `frontend/src/components/LogViewer.tsx`          | source 类型改为 string[]、source filter UI 移除、滚动方向 |
| `frontend/src/components/layout/Header.tsx`      | GitHub 链接、移除文档按钮                                 |
| `frontend/src/components/layout/Sidebar.tsx`     | Logo 使用应用图标                                         |
| `frontend/src/components/ui/UserProfileMenu.tsx` | 移除 sponsor/wechat/discord 按钮                          |
| `frontend/src/components/ui/AboutDialog.tsx`     | MCPHub Desktop 标识、canAutoUpdate 逻辑                   |
| `frontend/src/contexts/AuthContext.tsx`          | skipAuth/guest 模式                                       |
| `frontend/src/contexts/SettingsContext.tsx`      | httpPort/exposeHttp 字段                                  |
| `frontend/src/contexts/ServerInstallProgressContext.tsx` | 桌面端新增（见 3.6）：监听 install-progress / update-available 事件 |
| `frontend/src/App.tsx`                           | 包入 ServerInstallProgressProvider（见 3.6）              |
| `frontend/src/types/index.ts`                    | `Server.version` 字段（见 3.6）                           |
| `frontend/src/services/configService.ts`         | getPublicConfig 使用 apiGet                               |
| `frontend/src/services/changelogService.ts`      | Tauri 中禁用                                              |
| `frontend/src/pages/SettingsPage.tsx`            | 隐藏未实现模块、RuntimeVersionManager、HTTP 端口          |
| `frontend/src/pages/LoginPage.tsx`               | admin 默认填充、密码提示、Logo 图标                       |
| `frontend/src/pages/Dashboard.tsx`               | 隐藏 SMART/Docs                                           |
| `frontend/src/pages/ActivityPage.tsx`            | 隐藏用户列、createdAt UTC 转换、字段名统一为 createdAt     |
| `frontend/src/utils/tauriClient.ts`              | 桌面端新增；`toFrontendServer` 映射 `serverVersion`（见 3.6） |
| `frontend/src/utils/serverFormPayload.ts`        | config 按 serverType 分支构建、无 visibility；`perSessionClient` 加在 return 前 |
| `frontend/src/utils/fetchInterceptor.ts`         | isTauri() 拦截                                            |
| `frontend/src/utils/runtime.ts`                  | 运行时配置                                                |
| `frontend/index.html`                            | Splash 加载画面（内嵌 CSS 动画 + 内联 i18n 脚本）         |
| `locales/*.json`                                 | runtime* 翻译键（~18 个）；`server.downloading`/`updateAvailable`/`updateTo`/`reinstallStarted`（见 3.6） |

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

每次基线同步后，必须同步原项目的版本号，即：/Users/jphoebe/opt/code/IdeaProjects/github/mcphub-desktop/mcphub-origin/locales/zh.json文件中{{version}}
桌面的版本号规则为：{{version}}xxx, xxx代表当前桌面端的版本号，从001开始递增
| 项                             | 值                      |
| ------------------------------ | ----------------------- |
| **当前已同步到 origin commit** | `9dd75bc` (origin/main) |
| **对应 origin tag**            | `v1.0.24`               |
| **桌面端版本号**               | `1.0.24001`             |
| **同步执行日期**               | 2026-07-16              |

> 下次同步时，使用 `9dd75bc` 作为新的基线 SHA 起点（命令：`cd mcphub-origin && git --no-pager log --oneline 9dd75bc..HEAD`）。

### 4.4 最近同步记录

#### 2026-07-16：同步 `8d2ef15` → `9dd75bc`（3 个 commit）

origin 版本 `v1.0.23` → `v1.0.24`；桌面端版本 `1.0.23001` → `1.0.24001`。

**已同步到 desktop（前端 / locales）**

| 来源 commit | 说明 | desktop 应用方式 |
| ----------- | ---- | ---------------- |
| `d74d1be` | feat: per-session upstream client isolation for stateful MCP servers (#985) | 前端：`types/index.ts` 三方合并（`ServerConfig.perSessionClient` + `ServerFormData.perSessionClient`，保留桌面端 `version` 字段）；`serverFormPayload.ts` 合并（config 末尾追加 `perSessionClient`，桌面端 config 按 serverType 分支构建、无 visibility，故加在 return 前）；`ServerForm.tsx` 三方合并（formData 初始化 + 「会话级客户端隔离」checkbox UI，保留 hub 样式差异）；locales 四语言（en/fr/tr/zh）新增 `perSessionClient` + `perSessionClientDescription` 2 键，保留桌面端 runtime*/server.* 自定义键 |
| `ac2cbd0` | fix: update Chinese translation rules for release notes sections (#986) | **无需同步**：仅改 `.claude/skills/release-notes/SKILL.md`，与 app 无关 |
| `9dd75bc` | fix(security): sanitize proxychains4 command args to prevent RCE (#987) | **不同步**：Node `mcpService.ts` 专属；桌面端 Rust 后端无 proxychains4 实现（`ProxychainsConfig` 仅存于前端 type、Rust `ServerConfig` 无 proxy 字段、不 spawn proxychains4），无 RCE 面 |

**已镜像到 desktop（Rust 后端）**

| 来源 commit | 说明 | desktop 应用方式 |
| ----------- | ---- | ---------------- |
| `d74d1be` | feat: per-session upstream client isolation for stateful MCP servers (#985) | **已镜像 Rust 后端**（详见 §3.7）。原 PR 为 Node `mcpService.ts` 大改（+270，`sessionIsolatedClients` map + `isolatedClientCreationLocks` + `callToolWithReconnect` 隔离分支 + `closeIsolatedClient`/`cleanupIsolatedSession` + activity_log 字段），桌面端 Rust `pool.rs` 架构不同（每服务单 client 共享）故重新实现而非移植：① `ServerConfig.per_session_client: Option<bool>` + `migrate_v10`（servers 表加列，`TARGET_VERSION`→10）+ `server_service`/`settings_import` 读写；② `pool.rs` 缓存标志（`PoolEntry.per_session_client`）+ `is_per_session_client(name)`（读缓存不读 DB）+ `build_client` 改 `pub(crate)`；③ 新增 `mcp/session_pool.rs`（`SESSION_CLIENTS` map + `CREATE_LOCKS` 创建锁 + `call_tool_isolated` get-or-create/双重检查/120s 连接超时/失败驱逐 + `cleanup_session` 杀子进程树）；④ `http_server.rs` `dispatch_mcp` 提取 `mcp-session-id`、`tools/call` 按标志路由、DELETE handlers `cleanup_session`。**简化项**：不做 origin 的 HTTP 40x/SSE 细粒度重连重试（仅失败驱逐、下次重建）；不写 activity_log 的 perSessionClient 字段（次要）。前端 UI 此前已同步为 dormant，本次让 Rust 真正读取并生效。`9dd75bc` 安全修复不适用（见上）。 |

**验证**：`cd frontend && npm run build` 通过（`✓ built`）；`cd src-tauri && cargo check` 通过（rustc 1.96.0）。

---

#### 2026-07-09：同步 `c182265` → `8d2ef15`（22 个 commit）

**已同步到 desktop（前端 / locales）**

| 来源 commit | 说明                                                               | desktop 应用方式                                                                                          |
| ----------- | ------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------- |
| `8d2ef15`   | feat: add upstream OAuth disconnect (#984)                         | 前端：ServerCard.tsx 三方合并（新增「断开 OAuth」菜单项 + 确认弹窗，保留桌面端 OpenAPI 复制/httpPort 差异）；ServersPage.tsx 直接覆盖（新增 `onOAuthDisconnect` 透传）；serverOAuthService.ts 🆕 直接新增；ServerContext.tsx 手动合并（新增 `handleServerOAuthDisconnect` handler）；types/index.ts 三方合并（`oauth.revocationEndpoint` + `Server.oauth.connected`）；locales 四语言新增 `disconnectOAuth*` 4 键 |
| `fb58b7a`   | [codex] Fix remote keep-alive status updates (#966)                | 前端：ServerForm.tsx 三方合并（`enableKeepAlive === true` 严格判断 + 措辞改为「Connection Health」）；serverFormPayload.ts 三方合并（`keepAliveEnabled` 严格判断）；locales 四语言更新 `keepAlive*` 5 键措辞 |
| `ceca8be`   | fix: improve layout of GroupCard component for better responsiveness (#967) | 前端：GroupCard.tsx 三方合并（`flex h-full flex-col` + 路由图 `minHeight: 180`，保留桌面端 httpPort baseUrl 差异） |
| `d479fa6`   | fix discord link (#951)                                            | UserProfileMenu.tsx：桌面端已移除 sponsor/wechat/discord，**无需同步**                                    |
| (i18n OAuth callback #977 / INSTALL_BASE_URL #976 等) | i18n、配置类 commit                              | 前端无对应改动，**无需同步**                                                                              |

**已镜像到 desktop（Rust 后端）**

| 来源 commit | 说明                                                               | desktop 应用方式                                                                                          |
| ----------- | ------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------- |
| `274d950`   | fix: allow editing OpenAPI servers with recursive schemas (#959) (#960) | origin 修复两项：① `servers[0].url` 含 `{variable}` 模板时须用 `variables.*.default` 替换后再 parse；② Node `SwaggerParser.dereference` 产生的 live circular refs 需 `createSafeJSON`。桌面端用 `rmcp-openapi` crate，② 不存在（serde 原生序列化无循环引用）；① 确为真 bug —— `extract_base_url` 直接 `Url::parse(url)`，含模板变量会解析失败报"no base URL"无法连接。已镜像 ①：`src-tauri/src/mcp/openapi_transport.rs::extract_base_url` 增加 `variables` 模板替换，新增 4 个单元测试（含模板变量 / 普通 URL / 相对 URL / Swagger 2.0 host 回退）全部通过。 |

**未同步（后端 — 经评估无需 / 无法同步）**

| 来源 commit | 说明                                                               | 处理决策               | 原因分析                                                                                           |
| ----------- | ------------------------------------------------------------------ | ---------------------- | -------------------------------------------------------------------------------------------------- |
| `8d2ef15`   | feat: add upstream OAuth disconnect (#984) — 后端 `oauth/disconnect` 端点 + `upstreamOAuthDisconnectService.ts` | **后端不同步** | Rust 后端无上游 OAuth token 存储 / revocation 端点实现，`toFrontendServer` 不填充 `oauth.connected`，故 UI 按钮恒不显示。前端路由 `POST /servers/:name/oauth/disconnect` 已映射到 `__stub__`（返回「desktop mode 不可用」），仅作防误触的兜底，不会命中。待 Rust 端实现上游 OAuth 后再镜像实现。 |
| `0bbe272`   | [codex] fix automatic reconnect for remote servers (#972)          | **后端不同步** | 桌面端 Rust `ServerConfig` **无 keep-alive 字段**（`enableKeepAlive`/`keepAliveInterval` 配置被 serde 静默丢弃），无 per-server keep-alive 机制可修；仅有全局 `enableSessionRebuild` 后台重连（不同机制，见 `mcp_manager.rs:40`）。该 fix 修复的是 Node `keepAliveService` 的重连触发逻辑，架构不对应。 |
| `349f8b7`   | fix: preserve in-flight OAuth state on full reload (#981)          | **后端不同步** | Rust 使用 Tauri IPC 同步管理连接，无 Node WebSocket 广播竞态                                          |
| `38ffa6c`   | [codex] fix OpenAPI OAuth2 token refresh after 401 (#957)           | **后端不同步** | 桌面端 `OpenApiOAuth2` 仅 `{ token }`，无 `tokenUrl/clientId/clientSecret/expiresAt`；`call_tool` 用 `Authorization::None`（静态 token 走 headers）。无刷新能力基础，属功能新增而非 bug 镜像；待上游 OAuth 功能在 Rust 端落地时一并实现。 |
| `5b417e7`   | [codex] support OpenAPI YAML endpoints (#950)                      | **后端不同步** | Rust OpenAPI transport 是否支持 YAML 待单独评估（功能增强，非 bug 修复）                               |
| `a3ca516`   | feat: add INSTALL_BASE_URL support (#976)                         | **后端不同步** | 桌面端无 INSTALL_BASE_URL 部署场景（本地 Tauri 应用）                                                  |
| `61c5cfc` / `df8844a` | 默认 admin 用户 / 移除预置 admin 凭据                  | **后端不同步** | 桌面端使用独立的首启 admin 初始化逻辑（见 `migration` / `commands::auth`），Node `User` 模型不适用       |
| `f0128de`   | feat(activity): surface OAuth auth method in activity log (#958)   | **后端不同步** | 桌面端 Activity 字段已统一为 `createdAt`，无 Node `sseService` 的 API key 字段映射                     |
| `6220791` / `8700cc7` / `d479fa6`(docs) | docs / Docker / README 更新                         | **不同步**   | 部署 / 文档文件不同步（见 4.1 策略 4）                                                                |
| 依赖 bump（`d7b6cde`/`48b482a`/`a7ad03f`/`e3226d9`/`910e8bb`/`9033af2`） | pnpm-lock / package.json 依赖升级 | **不同步**   | 桌面端使用 npm（`package-lock.json`）+ Rust `Cargo.toml`，不共享 origin 的 pnpm 依赖图                  |

**完整 22 个 commit 分类总表**（与 git 客户端「最近 25 个」对照用）

> git 客户端默认显示 `HEAD~25..HEAD`（最近 25 个 commit），其中末尾 3 个（`c182265`/`1f90ab8`/`13be052`）属于上次基线 `c182265` 及之前，已于 2026-06-29 同步评估处理，**本次不重复**。下表是本次真正待同步的 22 个（`c182265..8d2ef15`），按时间倒序，每个 commit 显式归类，可与客户端逐条对账。

| # | commit | PR | 分类 |
| - | ------ | -- | ---- |
| 1 | `8d2ef15` | #984 | 前端已同步（OAuth 断开 UI）+ 后端不同步（无 Rust OAuth token 存储，前端路由 stub 兜底） |
| 2 | `349f8b7` | #981 | 后端不同步（Tauri IPC 同步管理，无 WebSocket 竞态） |
| 3 | `9af6fb4` | #977 | 前端无对应改动 + 后端不同步（i18n OAuth callback 页，Node `oauthCallbackController`/`i18n` 中间件） |
| 4 | `a3ca516` | #976 | 前端无对应改动 + 后端不同步（无 INSTALL_BASE_URL 部署场景） |
| 5 | `61c5cfc` | #973 | 前端已同步（ServerCard.tsx `index.css` 微调随 #984 一并合并）+ 后端不同步（独立首启 admin 逻辑） |
| 6 | `0bbe272` | #972 | 前端已同步（keep-alive 措辞）+ 后端不同步（Rust 无 per-server keep-alive 字段） |
| 7 | `df8844a` | #969 | 后端不同步（同 #973，独立 admin 凭据逻辑） |
| 8 | `6220791` | #968 | 不同步（docs / README 部署文件） |
| 9 | `ceca8be` | #967 | 前端已同步（GroupCard 响应式布局三方合并） |
| 10 | `fb58b7a` | #966 | 前端已同步（ServerForm/serverFormPayload keep-alive `===true` + 措辞）+ 后端不同步（Node `keepAliveService` 状态更新） |
| 11 | `d7b6cde` | #963 | 不同步（pnpm 依赖 bump typescript） |
| 12 | `48b482a` | #961 | 不同步（pnpm 依赖 bump openai） |
| 13 | `a7ad03f` | #964 | 不同步（pnpm 依赖 bump i18next-browser-languagedetector） |
| 14 | `e3226d9` | #965 | 不同步（pnpm 依赖 bump pg） |
| 15 | `274d950` | #959/#960 | **已镜像到 Rust**（`extract_base_url` server URL `{variable}` 模板替换 + 4 单元测试） |
| 16 | `f0128de` | #958 | 后端不同步（Activity 已统一 `createdAt`，无 API key 字段映射） |
| 17 | `8700cc7` | #956 | 不同步（Docker image 加 Cargo） |
| 18 | `38ffa6c` | #957 | 后端不同步（`OpenApiOAuth2` 仅 `{token}`，无 401 刷新基础） |
| 19 | `910e8bb` | #953 | 不同步（pnpm 依赖 bump i18next-fs-backend） |
| 20 | `9033af2` | #952 | 不同步（pnpm 依赖 bump typeorm） |
| 21 | `5b417e7` | #950 | 后端不同步（OpenAPI YAML 支持为功能增强，待评估） |
| 22 | `d479fa6` | #951 | 前端无需同步（桌面端已移除 discord 链）+ 不同步（docs） |

**统计核对**：前端已同步 6 个（#1/#5/#6/#9/#10/#22）｜Rust 镜像 1 个（#15）｜后端不同步 7 个（#2/#3/#4/#7/#16/#18/#21）｜部署/文档不同步 3 个（#8/#17，#22 兼）｜依赖 bump 不同步 6 个（#11/#12/#13/#14/#19/#20）。跨类说明：#1/#6/#10 前端已同步、后端部分不同步；#22 前端无需同步、docs 不同步。**去重后唯一 commit 合计 22 个，全覆盖。**

> 对照客户端的 25 个：末尾 3 个 `c182265`(#947)/`1f90ab8`(#936)/`13be052`(#945) 属于上次基线（2026-06-29 `8ff743f→c182265`，7 个 commit）已评估范围，其中 `c182265`/`1f90ab8`/`13be052` 在上次记录中明确标「不同步」，本次不重复处理。

**同步方式说明**

- 桌面端自定义文件（ServerCard / ServerForm / GroupCard / index.css / serverFormPayload / types）采用 `git merge-file --diff3` 三方合并（base=`c182265`，ours=desktop，theirs=origin HEAD），冲突仅 `index.css`（桌面端 9 列网格 vs origin 10 列响应式网格）需手动解，已保留桌面端「移除 visibility 列」的结构差异并采纳 origin 的 `.hub-server-card-status-cell` / `.hub-server-card-transport-cell` / `min-height` 响应式改进。
- 无自定义文件（ServersPage / serverOAuthService）直接覆盖 / 新增。
- locales 四语言仅「追加新增键 + 更新既有 keepAlive 措辞」，**未删除任何桌面端 `runtime*` 键**（同步后各文件仍含 16 个 runtime* 键）。

**同步后验证**

- `cd frontend && npm run build` ✓（2.22s）
- `cd src-tauri && CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse cargo check` ✓（57.60s）
- `cargo test --lib openapi_transport` ✓（4 passed；含 server URL 模板变量替换回归测试）
- `npx tsc --noEmit`：净增真实类型错误为 0（`handleServerOAuthDisconnect` 已通过 ServerContext 接入消除；ServerCard `server.type/openapi` 等为预存的桌面端自定义 OpenAPI 复制代码错误，仅行号位移）
- locales JSON 完整性 ✓，runtime* 键保留 ✓，自定义文件未被覆盖 ✓

#### 4.4.1 逐 commit 同步明细（origin 改动点 -> desktop 落点）

> 每条记录：origin 接口/文件改动点 → desktop 同步落点 → 修改的功能点。按时间正序（旧->新）。

**`d479fa6` fix discord link (#951)** — `2026-06-29`
- origin 改动点：`frontend/src/components/ui/UserProfileMenu.tsx`（discord URL `c8GKyzyFF`->`2BJehJZVH5`）+ 7 个 docs/README 文件
- desktop 落点：**无需同步**。桌面端 `UserProfileMenu.tsx` 已移除 sponsor/wechat/discord 整块 UI（自定义差异），不含该 discord `<a>` 标签
- 功能点：discord 社区链接更新 —— 桌面端无此 UI，不适用

**`5b417e7` [codex] support OpenAPI YAML endpoints (#950)** — `2026-06-29`
- origin 改动点：`src/controllers/openApiController.ts`（+46/-，YAML content-type 解析）、`src/routes/index.ts`（路由）、`package.json`（`@apidevtools/swagger-parser` 等）、`tests/controllers/openApiController.yaml.test.ts`（+119）
- desktop 落点：**后端不同步**。Rust `openapi_transport.rs::fetch_spec` 仅处理 JSON spec；YAML 支持属功能增强
- 功能点：OpenAPI spec 支持 YAML 端点 —— Rust 端待单独评估（非 bug 修复）

**`9033af2` chore(deps): bump typeorm 0.3.28->0.3.29 (#952)** — `2026-06-30`
- origin 改动点：`pnpm-lock.yaml`（+50/-）
- desktop 落点：**不同步**。桌面端用 npm（`package-lock.json`）+ Rust `Cargo.toml`，不共享 origin pnpm 依赖图
- 功能点：Node ORM 依赖升级 —— 架构不对应

**`910e8bb` chore(deps): bump i18next-fs-backend 2.6.4->2.6.6 (#953)** — `2026-06-30`
- origin 改动点：`pnpm-lock.yaml`（+8/-）
- desktop 落点：**不同步**。同上（pnpm 依赖图不共享）
- 功能点：Node 后端 i18n 文件后端依赖升级 —— 桌面端前端用 `i18next`，不涉及

**`38ffa6c` [codex] fix OpenAPI OAuth2 token refresh after 401 (#957)** — `2026-06-30`
- origin 改动点：`src/clients/openapi.ts`（+50：`invalidateRefreshableOAuth2Token` / `callTool` 401 重试）、`src/utils/serialization.ts`、`src/clients/__tests__/openapi-oauth2.test.ts`（+378）
- desktop 落点：**后端不同步**。`OpenApiOAuth2` 仅 `{ token }`，无 `tokenUrl/clientId/clientSecret/expiresAt`；`call_tool` 用 `Authorization::None`（静态 token 走 headers）。无刷新能力基础
- 功能点：OpenAPI OAuth2 access token 过期后自动刷新重试 —— 桌面端无该机制，待上游 OAuth 功能落地时一并实现

**`8700cc7` [codex] Add Cargo to Docker image (#956)** — `2026-06-30`
- origin 改动点：`Dockerfile`（+8）、`.github/DOCKER_CLI_TEST.md`、`docs/configuration/docker-setup.mdx`（中英）
- desktop 落点：**不同步**。部署/文档文件不同步（4.1 策略 4）
- 功能点：Docker 镜像加 Cargo 工具链 —— 桌面端为 Tauri 桌面应用，无 Docker 部署

**`f0128de` feat(activity): surface OAuth auth method in activity log API key field (#958)** — `2026-06-30`
- origin 改动点：`src/services/sseService.ts`（+14/-，活动日志 API key 字段记录 OAuth auth method）、`tests/services/keepalive.test.ts`
- desktop 落点：**后端不同步**。桌面端 Activity 字段已统一为 `createdAt`（自定义差异），Rust `activity_log` 模型无 Node `sseService` 的 API key 字段映射
- 功能点：活动日志的 API key 字段区分 OAuth 授权方式 —— 桌面端活动模型结构不同

**`274d950` fix: allow editing OpenAPI servers with recursive schemas (#959) (#960)** — `2026-07-01`
- origin 改动点：`src/clients/openapi.ts`（+27/-5：① server URL `{variable}` 模板用 `variables.*.default` 替换；② `createSafeJSON` 包裹 `inputSchema` 破 Node `SwaggerParser.dereference` 产生的循环引用）、`src/controllers/serverController.ts`、新增 2 个测试文件
- desktop 落点：**已镜像 ①**。`src-tauri/src/mcp/openapi_transport.rs::extract_base_url` 增加 `servers[0].variables` 模板替换（`{name}` -> `default`），新增 4 个单元测试；② 不存在（Rust 用 `rmcp-openapi` crate 原生 serde 序列化，无 live circular refs）
- 功能点：① 修复含 `{variable}` 模板 server URL 的 OpenAPI 服务器无法连接（`Url::parse` 失败报"no base URL"）；② 递归 schema 编辑 —— Rust 架构无此问题

**`e3226d9` chore(deps): bump pg and @types/pg (#965)** — `2026-07-02`
- origin 改动点：`pnpm-lock.yaml`（+72/-）
- desktop 落点：**不同步**。pnpm 依赖图不共享
- 功能点：PostgreSQL 驱动依赖升级 —— 桌面端用 SQLite（`sqlx`）

**`a7ad03f` chore(deps-dev): bump i18next-browser-languagedetector 8.2.0->8.2.1 (#964)** — `2026-07-02`
- origin 改动点：`pnpm-lock.yaml`（+16/-）
- desktop 落点：**不同步**。pnpm 依赖图不共享
- 功能点：i18n 语言检测库升级 —— 桌面端前端若需可单独评估 npm 版本

**`48b482a` chore(deps): bump openai 6.7.0->6.45.0 (#961)** — `2026-07-02`
- origin 改动点：`pnpm-lock.yaml`（+18/-）
- desktop 落点：**不同步**。pnpm 依赖图不共享
- 功能点：openai SDK 升级 —— 桌面端无 Smart Routing（待办），不使用

**`d7b6cde` chore(deps-dev): bump typescript 5.9.2->5.9.3 (#963)** — `2026-07-02`
- origin 改动点：`pnpm-lock.yaml`（+146/-）
- desktop 落点：**不同步**。pnpm 依赖图不共享
- 功能点：TypeScript 编译器升级 —— 桌面端用 npm，独立管理

**`fb58b7a` [codex] Fix remote keep-alive status updates (#966)** — `2026-07-02`
- origin 改动点：`frontend/src/components/ServerForm.tsx`（+2/-2：keepAlive `enabled` 严格判断 `=== true`）、`frontend/src/utils/serverFormPayload.ts`（+10/-10：`keepAliveEnabled` 严格判断）、`src/services/keepAliveService.ts`（+70）、`src/utils/serialization.ts`、`src/utils/serverConfigPersistence.ts`
- desktop 落点：**前端已同步**。`ServerForm.tsx` 三方合并（`enableKeepAlive === true`）、`serverFormPayload.ts` 三方合并（`keepAliveEnabled` 严格判断）；后端 `keepAliveService.ts` 不同步（Node 服务）
- 功能点：keep-alive 启用状态严格布尔判断（修复 falsy 误判）+ 远端状态更新 —— 前端表单逻辑同步，措辞改「Connection Health」

**`ceca8be` fix: improve layout of GroupCard component for better responsiveness (#967)** — `2026-07-03`
- origin 改动点：`frontend/src/components/GroupCard.tsx`（+6/-3：卡片 `flex h-full flex-col` + 路由图 `minHeight: 180`）
- desktop 落点：**前端已同步**。`GroupCard.tsx` 三方合并，保留桌面端 httpPort baseUrl 差异
- 功能点：GroupCard 组件响应式布局改进（等高卡片 + 路由图最小高度）

**`6220791` docs: update README files with Docker image variants (#968)** — `2026-07-04`
- origin 改动点：`README.md`/`README.fr.md`/`README.zh.md`、`AGENTS.md`、`CLAUDE.md`（+/-）
- desktop 落点：**不同步**。文档文件不同步（4.1 策略 4）
- 功能点：README 补充 Docker 镜像变体说明 —— 桌面端无 Docker 部署

**`df8844a` fix: remove pre-seeded admin credentials from shipped mcp_settings.json (#969)** — `2026-07-04`
- origin 改动点：`mcp_settings.json`（-8，移除预置 admin 凭据）、`tests/models/user-default-password.test.ts`
- desktop 落点：**后端不同步**。桌面端用独立首启 admin 初始化逻辑（`migration` / `commands::auth`），无 `mcp_settings.json` 预置凭据机制
- 功能点：移除 shipped 配置的预置 admin 密码（安全） —— 桌面端首启流程不同

**`0bbe272` [codex] fix automatic reconnect for remote servers (#972)** — `2026-07-05`
- origin 改动点：`frontend/src/components/ServerForm.tsx`（+10/-10：keep-alive 措辞）、`frontend/src/types/index.ts`（+4/-4：keepAlive 字段注释）、`locales/*.json`（4 语言，keepAlive 措辞）、`src/services/keepAliveService.ts`（+45：`reconnectServer` 回调 + 断线重连触发）、`src/services/mcpService.ts`（+27）、`src/types/index.ts`、多处 mcpService 测试
- desktop 落点：**前端已同步**（ServerForm 措辞 + types 注释 + locales keepAlive 5 键措辞）；**后端不同步**（`keepAliveService.ts`/`mcpService.ts` reconnect —— Rust `ServerConfig` 无 keep-alive 字段，配置被 serde 丢弃，仅有全局 `enableSessionRebuild` 后台重连，机制不同）
- 功能点：远端（SSE/Streamable HTTP）服务器断线后自动重连 + keep-alive 措辞改为「健康检查与自动重连」—— 前端 UI/文案同步，后端重连逻辑架构不对应

**`61c5cfc` feat: add default admin user for local development (#973)** — `2026-07-05`
- origin 改动点：`frontend/src/components/ServerCard.tsx`（+4/-4）、`frontend/src/index.css`（+32/-8：`.hub-server-card-row` 网格 + status/transport cell 类）、`src/models/User.ts`（+30：默认 admin）、`src/utils/path.ts`、`mcp_settings.json`、`scripts/dev-backend.js`（+105）、docs
- desktop 落点：**前端已同步**（ServerCard 的 index.css 网格改动随 #984 一并三方合并，status/transport cell 类已采纳）；**后端不同步**（`User` 模型默认 admin / `dev-backend.js` —— 桌面端独立首启 admin 逻辑）
- 功能点：本地开发默认 admin 用户 + ServerCard 网格响应式 —— 前端样式同步，后端用户初始化不同

**`a3ca516` feat: add INSTALL_BASE_URL support for dynamic configuration (#976)** — `2026-07-07`
- origin 改动点：`src/utils/installBaseUrl.ts`（+42，新增）、`src/betterAuth.ts`、`src/controllers/oauthServerController.ts`/`oauthDynamicRegistrationController.ts`/`serverController.ts`、`src/services/betterAuthConfig.ts`/`mcpOAuthProvider.ts`/`oauthClientRegistration.ts`/`openApiGeneratorService.ts`、4 个测试文件
- desktop 落点：**前端无对应改动 + 后端不同步**。桌面端无 INSTALL_BASE_URL 部署场景（本地 Tauri 应用，baseUrl 固定 `http://localhost:{httpPort}`）
- 功能点：动态 INSTALL_BASE_URL 配置（反向代理/自定义域名部署）—— 桌面端不适用

**`9af6fb4` fix: i18n OAuth callback page and per-request language resolution (#977)** — `2026-07-07`
- origin 改动点：`src/controllers/oauthCallbackController.ts`（+72/-，OAuth 回调页 i18n）、`src/middlewares/i18n.ts`（+24/-，按请求解析语言）、`src/utils/i18n.ts`（+64/-）
- desktop 落点：**前端无对应改动 + 后端不同步**。桌面端无 OAuth 回调页（Rust 后端 `oauth_protected_resource` 端点不涉及回调页渲染），i18n 由前端 `i18next` 处理
- 功能点：OAuth 授权回调页多语言 + 按请求语言解析 —— 桌面端无该页面

**`349f8b7` fix: preserve in-flight OAuth state on full reload (#981)** — `2026-07-08`
- origin 改动点：`src/services/mcpService.ts`（+21/-，全量重载时保留进行中的 OAuth 状态）、`tests/services/mcpService-toggle.test.ts`（+72）
- desktop 落点：**后端不同步**。Rust 用 Tauri IPC 同步管理连接，`toggle_server`/`reload_server` 中 `connect_server` 同步等待，无 Node WebSocket 广播竞态，不存在进行中 OAuth 状态丢失问题
- 功能点：全量重载时保留进行中 OAuth 授权状态 —— 桌面端连接管理架构不同，无此竞态

**`8d2ef15` feat: add upstream OAuth disconnect (#984)** — `2026-07-08`
- origin 改动点：`frontend/src/components/ServerCard.tsx`（+53：LogOut 图标 + onOAuthDisconnect prop + supportsOAuthDisconnect + handleOAuthDisconnect + 菜单项 + ConfirmDialog）、`frontend/src/contexts/ServerContext.tsx`（+25：handleServerOAuthDisconnect handler）、`frontend/src/pages/ServersPage.tsx`（+2：透传 onOAuthDisconnect）、`frontend/src/services/serverOAuthService.ts`（+11，🆕 disconnectServerOAuth）、`frontend/src/types/index.ts`（+2：`oauth.connected`）、`locales/*.json`（4 语言 `disconnectOAuth*` 4 键）、后端 `src/controllers/serverController.ts`/`src/services/upstreamOAuthDisconnectService.ts`（+217）/`mcpService.ts`/`routes/index.ts` 等
- desktop 落点：**前端已同步** —— ServerCard 三方合并（保留 OpenAPI 复制/httpPort 差异）、ServersPage 直接覆盖、serverOAuthService 🆕 新增、ServerContext 手动合并 handler、types 三方合并、locales 4 语言加键、`tauriClient.ts` 映射 `POST /servers/:name/oauth/disconnect` 到 `__stub__`（防误触兜底）；**后端不同步** —— Rust 无上游 OAuth token 存储/revocation 端点，`toFrontendServer` 不填充 `oauth.connected`，UI 按钮恒不显示
- 功能点：上游 MCP 服务器 OAuth 授权的断开（撤销 token / 重新授权）—— 前端 UI 同步（dormant），后端待 Rust 实现上游 OAuth 后镜像

---

### 4.5 历史同步记录

#### 2026-06-29：同步 `8ff743f` → `c182265`（7 个 commit）

**已同步到 desktop（前端 / locales）**

| 来源 commit | 说明                                                               | desktop 应用方式                                                                                          |
| ----------- | ------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------- |
| `212f760`   | fix: filter servers across all pages, not just the current page    | 前端：ServersPage.tsx, serverFilters.ts 直接覆盖，实现跨页面过滤功能                                      |
| `218e0c9`   | feat: store tool call payloads verbatim, gated by a config switch  | 前端：SettingsContext.tsx, SettingsPage.tsx 手动合并；locales 四语言翻译添加 storeToolPayload 相关键值     |

**未同步（后端 — 经评估无需同步）**

| 来源 commit | 说明                                                               | 处理决策               | 原因分析                                                                                           |
| ----------- | ------------------------------------------------------------------ | ---------------------- | -------------------------------------------------------------------------------------------------- |
| `c182265`   | fix: smart call_tool routing                                       | **不同步** | Rust `http_server.rs` 已正确处理 `$smart`（行 482/511/569），`mcp_scope_servers` 和 `mcp_scope_server_filters` 均在 `$smart` 时返回所有已连接服务器 |
| `1f90ab8`   | fix: skip HF tokenizer download for short queries in Smart Routing | **不同步** | Node.js 特有优化（HuggingFace tokenizer 下载），Rust 后端不使用 HF tokenizer                       |
| `13be052`   | fix: only flag true cycles, not diamond/shared refs, in safe serialization | **不同步** | Rust 使用 `serde_json` 原生序列化，不存在 Node.js `JSON.stringify` 自定义 replacer 的循环引用误判问题 |
| `718ebf7`   | fix: broadcast list changes after upstream data is loaded, not before | **不同步** | Rust 后端使用 Tauri IPC 而非 WebSocket 广播，`toggle_server` 中 `connect_server` 是同步等待的，不存在竞态条件 |
| `195ffbb`   | fix: scope server toggle to target instead of re-initializing the fleet | **不同步** | Rust `mcp_manager::toggle_server`（行 106-136）已限定到目标服务器，不会重新初始化整个连接池            |

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
- [X]  启动 Splash 加载画面（index.html 内嵌动画 + 内联 i18n + main.tsx 移除）
- [X]  首页统计面板空状态修复（hasLoaded 逻辑简化）
- [X]  stdio 包下载进度 / 更新检测 / 非阻塞连接（保存类命令后台连接、`server://install-progress` 下载进度、`server://update-available` 启动时检查、持久化 packageVersions + mark_reinstalled；详见 3.6）

### 待办

- [ ]  Smart Routing（智能路由）
- [ ]  OAuth Server
- [ ]  Better Auth 集成
- [ ]  Tool Result Compression
- [ ]  MCPB/DXT 文件安装
- [ ]  Templates（配置模板）
- [ ]  CI/CD 打包配置
