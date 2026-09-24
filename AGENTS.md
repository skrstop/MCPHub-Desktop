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
| -------- |---------------------------------------------------------|
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
- 当 `skipAuth=true` 时，自动创建 guest 用户（`username: '免登陆模式'`, `isAdmin: true`），并在 `AuthState.skipAuth` 字段标记 `true`
- 默认启用免登录模式（桌面端不需要登录）

**文件**：`frontend/src/types/index.ts`

- 🆕 `AuthState` 接口新增可选 `skipAuth?: boolean` 字段（见 3.5.12）：供 SettingsPage 等组件判断是否处于免登录模式

#### 3.1.4 配置服务

**文件**：`frontend/src/services/configService.ts`

- `getPublicConfig()` 使用 `apiGet` 而非 `fetchWithInterceptors`（适配 Tauri IPC）
- 默认返回 `skipAuth: true`（桌面端默认免登录）

### 3.2 UI/样式差异

#### 3.2.1 ServerForm（服务器表单）

**文件**：`frontend/src/components/ServerForm.tsx`

- 使用 mcphub-origin 的 `hub-*` 设计系统样式（`hub-card`, `hub-btn`, `hub-icon-btn` 等）
- **表单结构采用上游 #1034 的 3 分区布局**（2026-08-13 同步，#1055 增量 2026-08-20）：Section 1 Basic Info（#1055 改为 3 列网格：name 1 格 / description 2 格）/ Section 2 Connection / Section 3 Advanced Options（可折叠，`isAdvancedExpanded` state）。桌面端在此结构上定点保留差异（见下）。
- **`+` 按钮统一 hub 样式**（#1055）：env/header/env 的 `+` 按钮从 `bg-gray-200 hover:bg-gray-300 ... btn-primary` 改为 `hub-btn primary !w-[30px] !h-[30px] !p-0 justify-center text-base font-bold`（4 处）。
- **OAuth2 ↔ OpenID Connect 配置块顺序调整**（#1055）：OpenID Connect 块移到 OAuth2 块之前（与上游一致）。
- **passthrough headers + OAuth 配置移入 Advanced 分区按 serverType 路由**（#1055）：从 openapi/sse 分支内移出，Advanced 分区内统一块——`serverType === 'openapi'` 透传 `openapi.passthroughHeaders`，其余用 `formData.passthroughHeaders`；OAuth 折叠块仅 `serverType !== 'openapi'` 渲染。
- **不镜像 cookieSession UI**：上游 #1047 体系的 OpenAPI cookie 持久化 toggle，桌面 `rmcp-openapi` 传输无 cookie 落点，注释隐藏（`{/* Cookie Session Handling - hidden ... */}`）。
- **隐藏了可见性选择器**（Private/Group/Public）——桌面端默认所有服务器为公开。上游 #1054 把 group 改为「共享给指定用户」+ 候选 UI；桌面端**删除整个 visibility 选择器块 + sharedWithUsers 候选 UI**，仅留 `{/* Visibility section hidden in desktop client - all servers are public by default */}` 注释。
- 可见性默认值从 `private` 改为 `public`
- 保留桌面端新增的 OAuth2 完整配置（`oauth2TokenUrl`, `oauth2ClientId`, `oauth2ClientSecret`）
- `getInitialServerType` 显式返回类型 `: 'stdio' | 'sse' | 'streamable-http' | 'openapi'` 并跳过 `builtin`（ServerForm 仅用于自定义 server）
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
- **更新检查职责上移到根级**（见 3.4.7）：本组件不再做启动检查、不再渲染 `AboutDialog`，改为消费 `useUpdateCheck()`：红点徽标用 `showUpdateBadge`（头像 + 「关于」按钮两处），点「关于」调 `openAbout()`（由根级 `UpdateCheckProvider` 控制全局唯一 `AboutDialog`）。`version` prop 保留以兼容 `Sidebar` 调用，对话框实际版本由 provider 用 `PACKAGE_VERSION` 提供。

#### 3.2.4 AboutDialog（关于对话框）

**文件**：`frontend/src/components/ui/AboutDialog.tsx`

- 添加了 "MCPHub Desktop" 标识文字
- **release notes 按 Markdown 渲染**：新增 `Markdown` 组件（见 3.4.7），`latestEntry.summary`（即 `latest.json` 的 `notes`）渲染为 markdown 而非纯文本
- **布局**：卡片 `max-h-[85vh] flex flex-col`，标题+关闭按钮固定（`shrink-0`），中间内容区（新版本说明+历史列表）`flex-1 overflow-y-auto` 滚动，底部按钮行 `shrink-0 border-t` 固定——长说明不再顶跑标题与按钮
- 「最近更新」多版本卡片在 tauri-fallback 路径隐藏（`source !== 'tauri-fallback'`），因桌面端单条 entry 与上方「新版本可用」重复
- 安装中状态：「安装更新」按钮图标用 `Loader2` spinner（`isInstalling`），未安装态与「下载更新」链接用 `Download`
- 🆕 **下载/安装进度可视化**（见 3.4.8）：点击「安装更新」后，AboutDialog 内容区显示进度卡片——下载阶段显示百分比进度条 + 已下载/总字节 + 实时下载速度（EMA 平滑），安装阶段显示 indeterminate spinner，完成/失败显示对应状态。按钮文案随阶段切换（下载中→安装中→安装更新）。`installAppUpdate(onEvent)` 的 `DownloadEvent` 流被驱动为 `installPhase` state（`downloading`/`installing`/`done`/`error`）。
- **移除「忽略此版本」按钮**（见 3.4.7）：更新与否由用户决定，应用只提示

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
- 🆕 **「修改密码」区块在免登录模式下隐藏**（见 3.5.12）：用 `{!auth.skipAuth && (...)}` 包裹「Change Password」卡片
- 🆕 **导出配置 JSON 格式化修复**（见 3.5.12）：`fetchMcpSettings` 中 `result.data` 已是 Rust 返回的 pretty-printed 字符串时直接使用，不再二次 `JSON.stringify`（否则会转义成带反斜杠的扁平字符串）
- 🆕 **「下载 JSON」走原生保存对话框**（见 3.5.12）：`handleDownloadConfig` 在 `isTauri()` 下 `invoke('save_settings_json', ...)`，web 端保留 Blob 兜底；引入 `invoke` from `@tauri-apps/api/core`

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

#### 3.4.7 启动更新检查 / 自动提示 / 更新日志（桌面端自定义）

**背景**：origin 的更新检查只在用户手动打开「关于」时触发（`AboutDialog` 的 `useEffect([isOpen])`）。桌面端要求应用一启动即检查并提示新版本，且**不依赖登录态**（登录页也要能提示）；同时去掉 origin 的「忽略此版本」功能——更新与否由用户决定，应用只提示。

**文件**：`frontend/src/contexts/UpdateCheckContext.tsx`（⚠️ 新增）

- `UpdateCheckProvider` 挂载在 `App` 根级（`AuthProvider` 内、`Router` 外，与 `EmbeddingSyncAlertListener` 同级），见 `App.tsx`。
- 挂载即调用 `checkForAppUpdate('startup')`，不依赖登录/路由。
- 桌面端（`isTauri()`）走 Tauri updater；结果经 `buildChangelogFromTauriUpdate()` 转成 `ChangelogUpdateInfo` 存入 `updateInfo`。web 端走 changelog API。
- **检测到新版本即自动弹出「关于」对话框**（`setShowAbout(true)`）；`autoOpenedRef` 守卫保证每会话最多自动弹一次。
- provider 内部渲染**全局唯一的 `AboutDialog`**（根级，不再由 `UserProfileMenu` 渲染）。
- 暴露 `useUpdateCheck()`：`updateInfo`、`showUpdateBadge`、`openAbout()`。
- **⚠️ StrictMode 注意**：effect 故意**不加** `startedRef`/run-once 守卫。dev 下 `<React.StrictMode>` 双调用 effect（setup→cleanup(`cancelled=true`)→setup），若用 run-once 守卫会让第二次 setup 直接 return，导致第一次（已被 cancelled）的检查虽跑了（有 `checking`/`new version available` 日志）但在 `if (cancelled || !update) return` 处跳过 `setUpdateInfo`/`setShowAbout`，造成 dev 下「检查跑了但无红点、不弹框」。去掉守卫让存活的那次 setup 真正执行。prod 无 StrictMode 不受影响。

**文件**：`frontend/src/services/changelogService.ts`（⚠️ 修改）

- **移除「忽略此版本」功能**：删除 `dismissUpdateVersion`、`isUpdateDismissed`、`DISMISSED_UPDATE_KEY`。
- `shouldShowUpdateBadge(info)` 简化为 `Boolean(info?.hasUpdate && info.latestVersion)`——检测到新版本即亮红点，无「被忽略则不亮」逻辑。
- 新增 `buildChangelogFromTauriUpdate(update: UpdateInfo): ChangelogUpdateInfo`：桌面端 changelog API 被桩（`tauriClient.ts` 返回空），由 Tauri updater 结果构造 `ChangelogUpdateInfo`（`hasUpdate:true`、`source:'tauri-fallback'`、单条 entry 的 `summary` 即 `notes`）。`AboutDialog` 与启动检查共用此 helper，避免逻辑漂移。

**文件**：`frontend/src/utils/version.ts`（⚠️ 修改）

- `checkForAppUpdate(source: 'startup' | 'about' | 'manual' = 'about')`：新增 `source` 参数用于日志归因。`AboutDialog` 自动检查传 `'about'`、点按钮传 `'manual'`、启动检查传 `'startup'`。
- 全流程写 `[update]` 日志：开始检查、检测到新版本（含 `当前 -> 目标 (autoUpdate=...)`）、已是最新、检查失败（warn）、安装开始/完成/失败（error）。
- 导出 `logUpdateEvent(level, message)` 供 `UpdateCheckContext` 复用。

**更新检查日志（写入应用日志，日志页可见）**

**文件**：`src-tauri/src/commands/logs.rs`（⚠️ 修改）+ `src-tauri/src/lib.rs`（⚠️ 修改）

- 新增 Tauri command `log_event(level: String, message: String)`，内部调用 `app_logger::log_to_db()`，把前端日志写进 `app_log` 表（与 `get_logs` 同源，日志页可见）。
- 在 `lib.rs` 的 `invoke_handler` 注册 `commands::logs::log_event`。
- 前端 `logUpdateEvent`（`version.ts`）`invoke('log_event', ...)`，**fire-and-forget**（失败只 `console.warn`，绝不阻断检查）；非 Tauri 环境 no-op。
- 日志消息统一 `[update]` 前缀（沿用 `[startup]` 惯例）。`app_logger::extract_server_name` 会把 `[update]` 解析为 `serverName='update'`，故日志页可按来源 `update` 过滤。
- 示例日志：
  ```
  [update] checking for updates (source=startup)
  [update] new version available: 1.0.24001 -> 1.0.24099 (autoUpdate=true)
  [update] startup result: new version 1.0.24099, autoOpened=true
  [update] installing update: 1.0.24001 -> 1.0.24099
  [update] update installed, relaunching (-> 1.0.24099)
  ```

**release notes 按 Markdown 渲染**

**文件**：`frontend/src/components/ui/Markdown.tsx`（⚠️ 新增）+ `AboutDialog.tsx`

- 新增依赖 `react-markdown@^10` + `remark-gfm@^4`。⚠️ dev 模式下新增依赖后须清 `frontend/node_modules/.vite` 缓存再重启 `tauri dev`，否则 HMR 无法热替换、webview 跑陈旧中间态模块。
- `Markdown` 组件渲染 `latestEntry.summary`（即 `latest.json` 的 `notes`，本就是 `doc/upgrade/{version}.md` 全文）。GFM 启用（表格/删除线/任务列表/自动链接）。用 hub 设计 token 着色，链接强制 `target=_blank rel=noopener noreferrer`。
- `react-markdown` 渲染成 React 节点、不注入原始 HTML，对远端 `latest.json` 内容天然防 XSS，无需 DOMPurify。
- `inline` 模式（`<p>` 拍平为 `<span>`）用于 `entry.highlights` 列表项内联渲染。

**版本号同步**

四个版本源须保持一致（当前 `1.0.33002`）：

- `src-tauri/tauri.conf.json`（应用版本，也是 `import.meta.env.PACKAGE_VERSION` 的来源——`vite.config.ts` 从此注入）
- `src-tauri/Cargo.toml`
- `package.json`（根）
- `frontend/package.json`

`doc/upgrade/{version}.md` 存在对应版本时，CI 会将其全文作为 `latest.json` 的 `notes` 发布（见 3.4.3/3.4.4）。

**`doc/upgrade/{version}.md` 格式规范（MUST FOLLOW）**

changelog 采用固定分节格式（参考既有文件如 `doc/upgrade/1.0.33101.md`、`1.0.32003.md`），二级标题固定为以下几节，条目为普通 bullet：

```markdown
## 新功能
- 新增能力，粗体标注关键特性名，一句话说清用户可感知的行为

## 修复
- 用户可感知的 bug 修复，写「现象 + 原因一句话」，不写实现细节

## 限制（可选）
- 已知限制 / 平台差异

## 基线同步（仅当该版本包含 origin 同步时）
- 一句话概述同步来源 commit 范围，详情指向 AGENTS.md §4.4 对应条目
```

规范：

- 文件名与版本号一致（如 `1.0.34001.md`），在四个版本源同步递增后创建
- 面向用户写：不写实现细节/文件名堆砌，一个功能点一条 bullet，关键特性名加粗
- 分节顺序固定：新功能 → 修复 → 限制（可选）→ 基线同步（仅含 origin 同步时）；无关的节省略
- 发布时 CI 将全文作为 `latest.json` 的 `notes`，最终在「关于」对话框按 Markdown 渲染

#### 3.4.8 安装进度可视化（下载进度 / 下载速度 / 安装阶段）

> 背景：之前点击「安装更新」后 UI 只有一个 spinner，用户无法判断更新是否正常进行（下载是否卡住、装到哪一步）。现把 Tauri updater 的 `DownloadEvent` 流驱动成可见的进度状态。

**文件**：`frontend/src/components/ui/AboutDialog.tsx`（⚠️ 修改）

- 新增 `installPhase` state：`'idle' | 'downloading' | 'installing' | 'done' | 'error'`，替代原来的 `isInstalling` 布尔（`isInstalling` 派生为 `downloading || installing`，保留给按钮 disabled 逻辑）。
- 新增 `downloaded` / `totalBytes` / `speedBps` state + `lastTsRef` / `speedEmaRef` ref。
- `handleInstallUpdate` 重写：重置状态后调 `installAppUpdate(onEvent)`，`onEvent` 按 `DownloadEvent` 分支更新：
  - `Started`（`data.contentLength`）→ 记录总字节、进入 `downloading`。
  - `Progress`（`data.chunkLength`）→ 用 `performance.now()` 计算 chunk 间隔 `dt`，瞬时速度 `= chunkLength/dt`，EMA（α=0.3）平滑后写 `speedBps`；累加 `downloaded`。
  - `Finished` → 下载完成，进入 `installing`（indeterminate）。**Tauri updater 不暴露安装阶段百分比**（`Finished` 后插件静默校验签名+解压安装，无逐步事件），故安装阶段只能用 indeterminate spinner 表示，不造假百分比。
  - `installAppUpdate` resolve → `done`；reject → `error`。
- 进度卡片 UI（下载阶段）：百分比进度条（`downloaded/totalBytes*100`，totalBytes 未知时 indeterminate `animate-pulse`）+ 「已下载 / 总字节」+ `NN%` + `速度/s`（`formatBytes`/`formatSpeed` 模块级 helper，B/KB/MB/GB）。
- 安装阶段：`Loader2` spinner + `about.installing` 文案；完成：`CheckCircle2` + `about.installed`；失败：`X` + `about.installFailed`。
- 「安装更新」按钮文案随阶段切换：`downloading`→`about.downloading`、`installing`→`about.installing`、其余→`about.installUpdate`。

**i18n 新增翻译键**（en/zh/fr/tr 四语言，均带 inline fallback 防漏语种）：

- `about.downloading` — "正在下载..." / "Downloading..." / "Téléchargement..." / "İndiriliyor..."
- `about.installed` — "更新已安装，即将重启..." / "Update installed. Relaunching..." / ...
- `about.installFailed` — "安装失败，请重试。" / "Update failed. Please try again." / ...
- `about.installing` 复用既有键（"正在安装更新..."）。

> 注：仅 macOS / Windows（`canAutoUpdate=true`）走此流程；Linux 回退为「下载更新」外链，无此进度卡片。

### 3.5 Rust 后端差异

#### 3.5.1 免登录模式

**文件**：`src-tauri/src/commands/config.rs`

- 新增 `get_public_config` 命令：返回 `skipAuth` 和 `permissions` 配置
- 默认 `skipAuth: true`（桌面端默认免登录）
- 🆕 **`require_admin` 在 skipAuth 模式下直接放行**（见 3.5.12）：免登录时 `AuthContext` 只设假用户、不调用 `get_current_user`，Rust 侧 `SessionState` 始终为 `None`，`export_settings` 等读操作会因 "Not authenticated" 失败；现于 `require_admin` 起始处检查 `is_skip_auth_enabled()`，为真即 `Ok(())` 返回，使免登录下导出配置可用
- 🆕 新增 `save_settings_json` 命令（见 3.5.12）：原生「另存为」对话框 + 写盘

**文件**：`src-tauri/src/lib.rs`

- 注册了 `get_public_config` 命令
- 🆕 注册了 `save_settings_json` 命令

**文件**：`src-tauri/migrations/0005_default_skip_auth.sql`

- 数据库迁移：默认设置 `routing.skipAuth = true`

#### 3.5.2 Guest 用户支持

**文件**：`src-tauri/src/models/user.rs`

- `UserRole` 枚举新增 `Guest` 变体

**文件**：`src-tauri/src/commands/auth.rs`

- `login` 命令处理 `UserRole::Guest` 匹配
- `get_current_user` 命令在无 token 且 skipAuth 启用时返回 guest 用户
- 🆕 `is_skip_auth_enabled()` 由私有 `async fn` 改为 `pub(crate)`，供 `commands::config::require_admin` 复用（见 3.5.12）

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
| v7-v19 | `migrate_v7..v19` | （v7/v8/v13/v20 有对应 sql，其余无） | 活动日志 source_ip、per_session_client、start_on_demand/idle_timeout_ms、RAG 权重/端口默认值修正、skills 等（详见 `migration.rs` 源码） |
| v20  | `migrate_v20` | `0020_server_proxy_column.sql` | servers 表添加 `proxy` 列（Proxychains4 配置 JSON，上游 #1055 proxy round-trip 落点）                                       |

> 注：§3.5.5 的版本映射表为早期快照（仅到 v6），实际 `TARGET_VERSION` 已随历次镜像推进至 20。新增迁移以 `migration.rs` 源码与 `pub const TARGET_VERSION` 为准，本表仅补登 v20（本次同步引入）。

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

#### 3.5.12 免登录模式下的设置页可用性（导出配置 + 修改密码）

> ⚠️ **基线同步注意**：本节涉及的全部文件都带桌面端自定义，同步 origin 时**禁止批量覆盖**，必须手动合并保留以下差异。origin（Node.js）用 `requireAdmin` 中间件 + guest admin 用户实现免登录授权，路径不同；桌面端在 Rust 命令层放行，不可直接套用 origin 实现。

**背景**：免登录（skipAuth）模式下，「导出配置」的「复制到剪切板 / 下载 JSON」按钮始终禁用、「下载 JSON」点击无效。根因有两处：

1. **导出数据拿不到**：桌面端 `export_settings` 调 `require_admin(&session)` 要求 `role=="admin"` 的 token，但免登录时 `AuthContext.loadUser` 只设假用户、**从不调用 `get_current_user`**，Rust 侧 `SessionState` 一直为 `None` → 报 "Not authenticated"；即便走 guest 分支，guest token 的 `role` 是 `"guest"`，仍会被 `require_admin` 拒绝。
2. **下载无效果**：Tauri webview（WKWebView/WebView2）**不支持程序化的 blob-URL 下载**，浏览器里 `link.click()` 能触发下载是因为浏览器内核支持，Tauri webview 拦不到「保存文件」动作，导致 `handleDownloadConfig` 静默失败、只剩 toast 提示。

##### 改动一：`require_admin` 在 skipAuth 下放行

**文件**：`src-tauri/src/commands/config.rs`

`require_admin` 起始处新增短路：skipAuth 为真即直接 `Ok(())`，与前端把免登录用户视为 admin 一致。受影响命令：`export_settings`、`get_server_config_for_copy` 等。

```rust
async fn require_admin(session: &SessionState) -> Result<(), String> {
    if crate::commands::auth::is_skip_auth_enabled().await {
        return Ok(());
    }
    // ... 原 token 校验逻辑
}
```

> 注：`commands::bearer_keys::require_admin` 是**独立副本**（另定义于 `bearer_keys.rs`），同样会因免登录而失效。本次仅按报告的导出按钮定点修复 `config.rs`；若需要免登录下也能管理 Bearer Keys，需用同样方式放开 `bearer_keys.rs` 的 `require_admin`。

##### 改动二：`is_skip_auth_enabled` 提为 `pub(crate)`

**文件**：`src-tauri/src/commands/auth.rs`

原私有 `async fn is_skip_auth_enabled()` 改为 `pub(crate) async fn`，供 `commands::config::require_admin` 复用，避免逻辑重复。

##### 改动三：新增 `save_settings_json` 命令（原生保存对话框）

**文件**：`src-tauri/src/commands/config.rs`、`src-tauri/src/lib.rs`

新增 `save_settings_json(app, content, file_name)` Tauri 命令：用 `tauri-plugin-dialog`（`DialogExt`）弹原生「另存为」对话框（JSON 过滤器 + 默认文件名 `mcp_settings.json`），取到路径后 `std::fs::write` 写盘。用户取消对话框时返回 `Err("cancelled")`，前端据此静默不报错。依赖的 `tauri-plugin-dialog` / `tauri-plugin-fs` 已在 `lib.rs` `tauri::Builder` 注册、`capabilities/default.json` 已授权（`dialog:*`、`fs:allow-write-text-file` 等）；前端无 `@tauri-apps/plugin-dialog` JS 绑定，故走自定义命令而非插件 JS API。

```rust
#[tauri::command]
pub async fn save_settings_json(
    app: AppHandle,
    content: String,
    file_name: Option<String>,
) -> Result<String, String> {
    let default_name = file_name.unwrap_or_else(|| "mcp_settings.json".to_string());
    let file_path = app.dialog().file()
        .add_filter("JSON", &["json"])
        .set_file_name(default_name)
        .blocking_save_file();
    let Some(file_path) = file_path else {
        return Err("cancelled".to_string());
    };
    let path = file_path.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&path, content.as_bytes()).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}
```

并在 `generate_handler!` 中注册 `commands::config::save_settings_json`。

##### 改动四：`AuthState` 类型新增 `skipAuth` 字段

**文件**：`frontend/src/types/index.ts`

`AuthState` 接口新增可选 `skipAuth?: boolean`。`AuthContext` 在免登录分支已设置该字段，但类型未声明，补齐以便组件判断。

##### 改动五：SettingsPage 三处前端改动

**文件**：`frontend/src/pages/SettingsPage.tsx`

1. **「修改密码」隐藏**：用 `{!auth.skipAuth && (...)}` 包裹「Change Password」卡片，免登录模式下不显示。
2. **导出 JSON 格式化修复**：`fetchMcpSettings` 中 `result.data` 已是字符串（Rust `export_settings` 返回 pretty-printed 字符串）时直接使用，**不再二次 `JSON.stringify`**（否则会把内部的 `"` 转义成 `\"`、换行变 `\n`，`<pre>` 里显示成带反斜杠的扁平字符串）：
   ```ts
   const configJson =
     typeof result.data === 'string' ? result.data : JSON.stringify(result.data, null, 2);
   ```
   > 注：按服务器导出（带 `serverName`）走 `get_server_config_for_copy`，Rust 返回 `serde_json::Value`（对象）而非字符串，`ServerCard.tsx` 原有的 `JSON.stringify(result.data, null, 2)` 正好需要保留 —— `typeof` 判断对对象分支行为不变。
3. **「下载 JSON」走原生对话框**：`handleDownloadConfig` 改为 `async`，桌面端 `invoke('save_settings_json', { content, fileName })`，取消时 `String(e) === 'cancelled'` 静默；web 端保留原 Blob 下载兜底。新增 `import { invoke } from '@tauri-apps/api/core'`。

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

### 3.8 On-demand stdio 按需启动（startOnDemand，origin #1012 镜像）

> 镜像 origin `976b4ac`（PR #1012）。当 stdio server 配置 `startOnDemand: true` 时，启动**不**连接该服务，而是插入「睡眠」占位（`client: None`、`connected: false`、`start_on_demand: true`），首次工具调用时才懒建 client + 连接，并在空闲超时（默认 5 分钟）后自动关闭进程；工具列表缓存保留，下次调用自动冷启动。降低低频服务的常驻内存占用。

#### 3.8.1 作用域（与 origin 一致）
- **仅 stdio server**：HTTP/SSE 无重型进程，不参与（`connect_server` 仅对 `ServerType::Stdio` 生效睡眠占位）。
- **共享 pool 调用路径**：Tauri `call_tool` 命令 + HTTP 非隔离 `tools/call` 走 `pool::call_tool` -> 路由到 `on_demand::call_tool_on_demand`。
- **与 perSessionClient 互斥**：`server_service::create`/`update` 校验 `perSessionClient && startOnDemand` 同时为真则报错（两者语义冲突：一个是每 session 独立进程，一个是共享懒启动）。
- `tools/list` 用共享 pool 的缓存工具列表（睡眠 server 的工具在首次唤醒后缓存，之后即使再睡眠也保留）。

#### 3.8.2 Model + DB
- `models/server.rs`：`ServerConfig` 加 `#[serde(default)] pub start_on_demand: Option<bool>` + `pub idle_timeout_ms: Option<u64>`（camelCase -> JSON `startOnDemand`/`idleTimeoutMs`，前端已发）；`ServerStatus` 加 `pub start_on_demand: bool`（`#[serde(default)]`，序列化为 `startOnDemand`，供 HTTP 路由 + 前端 StatusDot 判断 sleeping）。
- `db/migration.rs`：`migrate_v17`（`ALTER TABLE servers ADD COLUMN start_on_demand INTEGER NOT NULL DEFAULT 0` + `idle_timeout_ms INTEGER NOT NULL DEFAULT 0`，均 `.ok()` 容错已存在）；`TARGET_VERSION` 16 -> 17；`apply_migration` match 加 `17 => migrate_v17`。
- `services/server_service.rs`：3 个 SELECT 列清单加 `start_on_demand, idle_timeout_ms`；`create`/`update` 的 INSERT/UPDATE 加列与 bind（`cfg.start_on_demand.unwrap_or(false) as i64` / `cfg.idle_timeout_ms.unwrap_or(0) as i64`）；`map_row` 读两列 -> `Some(r.try_get::<i64,_>("start_on_demand")? != 0)` / `if ms > 0 { Some(ms as u64) } else { None }`；`create`/`update` 起始处互斥校验。
- `services/settings_import.rs`：导入旧 `mcp_settings.json` 时 `ServerConfig` 字面量补 `start_on_demand: None` + `idle_timeout_ms: None`。
- `rag/service.rs`：builtin server 的 `ServerConfig` 字面量补两字段 `None`、`ServerStatus` 补 `start_on_demand: false`（builtin 永不按需启动）。

#### 3.8.3 Pool 占位 + 路由
- `mcp/pool.rs`：
  - `PoolEntry` 加 `start_on_demand: bool`（`connect_server` 开头从 `cfg.start_on_demand.unwrap_or(false) && cfg.server_type == ServerType::Stdio` 计算；所有占位/成功/失败分支也带该字段）。
  - `connect_server`：在 `disconnect_server` 清理后、插入「starting」占位前，若 `start_on_demand` 为真，插入「sleeping」占位（`client: None`、`connected: false`、`starting: false`、`start_on_demand: true`）并直接返回，**不**走 build_client/连接/重试逻辑。
  - `call_tool`：开头读 pool 缓存的 `start_on_demand` 标志，为真则 `return super::on_demand::call_tool_on_demand(...)`；否则走原共享 client 路径。
  - `list_all_tools`：过滤条件从 `e.status.connected` 放宽为 `e.status.connected || (e.start_on_demand && !e.tools.is_empty())`，睡眠 server 的缓存工具仍可被发现。
  - `disconnect_server`：新增 `super::on_demand::shutdown_on_demand_lifecycle(name)`（在 `session_pool::cleanup_server` 之后），reap 活跃的 on-demand 子进程。
  - `disconnect_all`：新增 `super::on_demand::cleanup_all_on_demand()`（在 `session_pool::cleanup_all` 之后）。
  - 新增 `pub(crate) async fn mark_on_demand_awake(name, tools, server_version)` / `mark_on_demand_sleeping(name)` / `mark_on_demand_error(name, error)`：供 on-demand 模块更新 pool 占位的 status（connected/tools/error），不暴露 pool 内部锁。

#### 3.8.4 新增 `mcp/on_demand.rs`（按需 client 存储）
- 结构仿 `session_pool.rs`：`static ON_DEMAND_CLIENTS: OnceLock<RwLock<HashMap<String, OnDemandEntry>>>`，key = `server_name`。`OnDemandEntry { client: Arc<Mutex<McpClient>>, last_used: Instant, idle_ms: u64, idle_handle: Mutex<Option<JoinHandle<()>>> }`。
- `static CREATE_LOCKS`：per-server 创建锁，防并发首调重复 spawn。
- `pub async fn call_tool_on_demand(server_name, tool, arguments) -> Result<ToolCallResult>`：
  1. 快速路径：读 map 命中 -> clone Arc + 读 `idle_ms` -> `run_call`（锁外执行）。
  2. 未命中：取/建 per-server 创建锁 -> `_guard.lock()` 串行 -> 双重检查。
  3. 新建：`server_service::get_by_name` 取 cfg（**仅新建时一次 DB 读**），`pool::build_client(&cfg)?`，`timeout(120s, client.connect())`，`list_tools()` + `server_version()`，缓存 entry，`pool::mark_on_demand_awake`。
  4. `run_call` 失败（连接类错误）：从 map 移除 + disconnect + `pool::mark_on_demand_sleeping`，下次重建。
  5. `run_call` 成功：更新 `last_used` + `schedule_idle`（重置空闲定时器）。
- `schedule_idle`：abort 旧 `idle_handle`，spawn 新定时器任务（捕获 `last_used` 作为 generation）。
- `shutdown_on_demand_idle(name, snapshot)`：定时器回调，双重检查 `last_used == snapshot`（newer call 重置过则跳过），移除 entry + disconnect + `pool::mark_on_demand_sleeping`（**保留缓存工具**）。
- `shutdown_on_demand_lifecycle(name)`：disable/reload/delete/update 时由 `disconnect_server` 调用，移除 + disconnect + 清创建锁（不碰 pool 占位，由 `disconnect_server` 负责）。
- `cleanup_all_on_demand()`：shutdown 时由 `disconnect_all` 调用。
- `mcp/mod.rs`：`pub mod on_demand;`。

#### 3.8.5 mcp_manager / http_server
- `services/mcp_manager.rs`：`start_all` 连接结果日志区分 sleeping（`cfg.start_on_demand` 为真且 `!status.connected` 时记 "sleeping (on-demand)"）；自动重连循环（`enableSessionRebuild` 30s 周期）**跳过** on-demand server（`continue`，不唤醒睡眠服务）。
- `services/http_server.rs`：`mcp_scope_server_filters` 全局/单服务器 scope 的过滤条件从 `s.connected` 放宽为 `s.connected || s.start_on_demand`，睡眠 on-demand server 仍可被 `tools/call` 冷启动、其缓存工具仍可被 `tools/list` 暴露。

#### 3.8.6 前端
- `frontend/src/types/index.ts`：`ServerConfig` 加 `startOnDemand?: boolean` + `idleTimeoutMs?: number`；`ServerFormData` 加同名字段。
- `frontend/src/components/ui/StatusDot.tsx`：`ServerStatusDotProps` 加 `startOnDemand?: boolean`；`startOnDemand && status === 'disconnected'` 时渲染 `kind="muted"` + 💤 + `t('status.sleeping', 'Sleeping')`（origin 用内联默认值，未加 locale 键）。
- `frontend/src/components/ServerCard.tsx`：`<ServerStatusDot>` 传 `startOnDemand={server.config?.startOnDemand === true}`。
- `frontend/src/components/ServerForm.tsx`：stdio 专属「按需启动」checkbox + idle timeout 数字输入框（min 10000，step 1000，默认 300000）+ 初始化从 `initialData?.config?.startOnDemand` / `idleTimeoutMs`。⚠️ 自定义文件手动合并，保留 hub 样式 / 隐藏 visibility / OAuth2 / 下载进度条等差异。
- `frontend/src/utils/serverFormPayload.ts`：`buildServerPayload` 的 config 携带 `startOnDemand` / `idleTimeoutMs`（仅 `startOnDemand === true` 时发 `idleTimeoutMs`）。

#### 3.8.7 资源/边界
- on-demand 活跃 client 存于独立 store（`ON_DEMAND_CLIENTS`），pool 占位 `client` 始终 `None`；`disconnect_server` 同时清理两处。
- 空闲关闭保留缓存工具（`mark_on_demand_sleeping` 不清 `entry.tools`），server 仍可被发现并冷启动。
- 不影响既有 pool 的连接/状态/进度事件逻辑（`connect_server` 仅新增睡眠占位分支 + `start_on_demand` 字段透传）。
- 编译验证：`ORT_SKIP_DOWNLOAD=1 cargo check` 通过（asdf cargo 1.96.0）；`npm run build` 通过。

#### 3.8.8 手动验证（计划）
- 配一个 stdio server 勾选「按需启动」；启动应用 -> 服务状态显示 💤 Sleeping（而非 connecting/connected）；`ps` 无该子进程。
- 调用该服务任一工具 -> 状态变 connected、子进程出现、工具返回正常。
- 等待 idle timeout（可配 10000ms 加速）-> 子进程消失、状态回 Sleeping、工具列表仍可见。
- 再次调用 -> 冷启动重建。
- 回归：不勾选 startOnDemand 的服务，启动即连接，行为不变。
- 互斥：同时勾选 perSessionClient + startOnDemand 保存应报错。

### 3.9 stdio 连接错误包含上游 stderr（origin #1015 镜像）

> 镜像 origin `a4a628a`（PR #1015）。stdio server 连接失败时，把上游进程的 stderr 尾部拼进 error message，便于排查 Python traceback / 缺失依赖等问题（origin 的 `stdioDiagnostics` 等价实现）。

**文件**：`src-tauri/src/mcp/stdio_transport.rs`
- `StdioTransport` 新增 `stderr_tail: Arc<std::sync::Mutex<String>>`（~32KB 滚动缓存；用 `std::sync::Mutex` 而非 `tokio::sync::Mutex`，以便在 `map_err` 同步闭包中读取，guard 不跨 await）。
- `new()` 初始化为空 String。
- stderr drain task（`tokio::spawn`）：每行 `push_str` + `\n`，超 32KB 从头部 `drain`。
- `connect()` 的 `initialize` 握手失败 `map_err` 路径：锁 `stderr_tail`，`trim_end` 后非空则返回 `anyhow!("... handshake failed: {e}\n--- upstream stderr ---\n{tail}")`，否则原样返回 `e`。
- 该 error 经 `pool::connect_server` 的 `Ok(Err(e))` 分支存入 `ServerStatus.error`，前端 ServerCard 显示。

### 3.10 RAG 文件创建/更新工具 + 文件查看可视化（桌面端独有）

> 新增 2 个 MCP 工具（`rag_file_create` / `rag_file_update`）+ 文件详情/搜索片段的按类型可视化渲染。工具经 MCP `tools/call`（`http_server.rs` dispatch_mcp）暴露，与 `rag_search`/`rag_get`/`rag_tag_search` 同级。

#### 3.10.1 存储/命名（与上传的区别）
- **位置**：`<app_data>/rag/files/`（与上传同目录）。
- **meta 文件名**：统一 `{id}.meta`（id=uuid，与上传一致，`get_doc`/`delete_doc`/`set_tags` 等按 id 定位 meta 不变）。
- **content 文件名**：上传用 `{id}`（uuid），`rag_file_create` 用 `{docName}.{docType}`（人类可读）。新增 `content_path_for(dir, id, meta_name)` helper：先试 `dir/{id}`，不存在再试 `dir/{meta.name}`，兼容两种命名。`get_doc`/`delete_doc`/`open_file_location`/`find_doc_ids_by_name` 删除均改用该 helper。
- **DocMeta 加 `file_type: Option<String>`**（`#[serde(default)]`，向后兼容旧 meta）：`rag_file_create`/`rag_file_update` 从 docType 查 `file_support.json` 得标签（如 "Markdown"）持久化。`list_docs`/`get_doc` 优先用 `meta.file_type`，否则回退 `file_type_label(name)`。

#### 3.10.2 `rag_file_create` 工具
- **入参**：`docName`（必选，不含扩展名）、`docType`（必选，裸扩展名如 "md"/"java"/"py"/"txt"）、`docContent`（必选，UTF-8）。
- **出参**：`{ docId }`。
- **流程**（`service::create_doc_from_content`）：sanitize docName -> 文件名 `{docName}.{docType}`（若 docName 已以 .{docType} 结尾则不重复拼）-> 大小校验（`MAX_UPLOAD_BYTES` 64MiB）-> 同名覆盖 -> `write_doc_and_index` -> 删旧向量 + `recompute_tag_stats` -> 返回 docId。跳过编码检测（入参 UTF-8）。

#### 3.10.3 `rag_file_update` 工具
- **入参**：`docId`（必选）、`docName`/`docType`/`docContent`（可选）、`docContentAppend`（bool，默认 false，仅 docContent 有值时生效：true=追加，false=替换）。
- **流程**（`service::update_doc`）：按 id 读 meta -> 更新 name/file_type -> 若 docContent 有值：写新 content + `reindex_doc`（内部 delete_by_doc + add_chunks 重建向量）+ 更新 size/chunk_count -> 若 name 变且 content 按旧名命名（非 uuid）则重命名 content 文件 -> 写 meta。docId 不变。

#### 3.10.4 `write_doc_and_index` helper
- `upload_one_path_inner` 与 `create_doc_from_content` 共用的「写 content + reindex_doc + 写 meta」抽取为 helper。upload 传 `file_stem=id`/`file_type=None`；create 传 `file_stem={name}.{ext}`/`file_type=Some(label)`。

#### 3.10.5 文件查看可视化
- **前端依赖**：新增 `rehype-highlight` + `highlight.js/styles/atom-one-dark-reasonable.css` 主题。
- **新组件 `frontend/src/components/ui/FileTypeRenderer.tsx`**：Markdown -> `<Markdown>` 组件；代码 -> `<ReactMarkdown rehypePlugins=[rehypeHighlight]>` 包成 ``` ```{lang} ``` ``` fenced 代码块着色；纯文本 -> `<pre>`。`inline` 模式供搜索片段用。
- **新 helper `frontend/src/utils/fileType.ts`**：`extOf`/`isMarkdown`/`hlLangFor`（扩展名 + Dockerfile/Makefile 特殊名 -> highlight.js 语言别名）。
- **ViewDialog**：`<pre>{doc.content}</pre>` -> `<FileTypeRenderer content={doc.content} fileName={doc.name} fileType={doc.fileType} />`。
- **VectorSearchDialog**：`{r.snippet}` -> `<FileTypeRenderer content={r.snippet} fileName={r.docName} inline />`（fileType 从 docName 推导）。

#### 3.10.6 工具分发
- `http_server.rs` dispatch_mcp 在 `rag_tag_search` 后加 `rag_file_create`/`rag_file_update` 分支：取参（必选校验）-> 调 service -> 返回 `{docId}`/`{docId, updated}` 作为 text content。RAG 关闭返回 `-32603`。

#### 3.10.7 边界
- docName sanitize 防路径穿越。
- `rag_file_update` 改 name：仅 create 文档（content 按名命名）重命名 content 文件；上传文档（content 按 id）不重命名。
- `content_path_for` 兼容两种命名，旧/新文档都能正确定位。
- 编译验证：`ORT_SKIP_DOWNLOAD=1 cargo check` 通过；`npm run build` 通过。

### 3.11 编辑服务器避免无谓重连 + proxy 持久化（origin #1055 镜像）

> 镜像 origin `bfd153c`（PR #1055，= `v1.0.30` tag）。编辑服务器时，若只改了非连接相关字段（如 description），不再杀掉实时 MCP 连接 / 重启 stdio 子进程；同时把 `proxy`（Proxychains4）配置真正持久化，使前端 round-trip 生效。

#### 3.11.1 连接相关字段比对（`update_server` 快路径）
- **文件**：`src-tauri/src/commands/servers.rs::update_server`
- 原 `update_server` 无条件 `disconnect_server` + 后台 `connect_server` —— 改个 description 也会断连重启。
- 改为：先 `server_service::get_by_name(&name)` 取既有配置 → `has_connection_relevant_change(prev, next)` 比对 → 无连接相关变更时仅 `server_service::update` 持久化 + 保留实时连接（不 spawn 重连），日志 `[{}] update_server: no connection-relevant change, kept live runtime`；有变更或 `enabled` 改变才 `disconnect_server(&name)` + 后台 spawn 重连。
- `starting` 状态按 `connection_relevant_changed` 派生（无变更时为 false）。
- **rename 仍天然覆盖**：上游 #1055 另需 `closeServer(name)`（旧名）是因为其 `addOrUpdateServer` 按新名 close 才漏掉旧名进程；桌面端 `update_server` 开头就对旧名 `disconnect_server`，不存在该泄漏。
- **`enabled` 故意保留在比对内**：通过 `update_server` 改 enabled（true→false）应断连，false→true 应重连，与历史行为一致（独立 `toggle_server` 走 `mcp_manager`，不受影响）。

#### 3.11.2 `has_connection_relevant_change` 实现
- 序列化整个 `ServerConfig` 为 JSON（`#[serde(rename_all="camelCase")]`）→ 剔除访问/元数据字段 `id`/`name`/`description`（visibility/owner/sharedWithUsers 桌面端 Rust 模型本就没有）→ 默认请求超时 `60000` 视为「未设置」（与上游 `toConnectionRelevantConfig` 一致：显式 60000 与 absent 比对为相等）→ `strip_nulls` 去掉 null（unset 与 null 比对为相等）→ 深比对两 JSON。
- 留在比对内的连接相关字段：`type`/`url`/`command`/`args`/`env`/`headers`/`options`/`openapi`/`perSessionClient`/`startOnDemand`/`idleTimeoutMs`/`proxy`/`enableKeepAlive`/`keepAliveInterval`/`enabled`。

#### 3.11.3 proxy 持久化（使前端 round-trip 生效）
- **背景**：上游 #1055 前端 `serverFormPayload` 引入 `config.proxy = formData.proxy` round-trip（无表单编辑器，编辑任何字段都原样带回，避免丢字段触发重连）。但桌面端 Rust `ServerConfig` 此前无 `proxy` 字段，serde 默默丢弃前端发来的 `proxy` —— round-trip 名存实亡。
- **模型**：`src-tauri/src/models/server.rs` 新增 `ProxychainsConfig`（`enabled`/`type`(rename `proxy_type`)/`host`/`port`/`username`/`password`/`config_path`，camelCase）+ `ServerConfig.proxy: Option<ProxychainsConfig>`。
- **DB**：`db/migration.rs` `migrate_v20`（`add_column_if_missing servers proxy TEXT`，幂等）+ `TARGET_VERSION` 19 → 20 + `apply_migration` 加 `20 => migrate_v20`。配套 `migrations/0020_server_proxy_column.sql`。
- **持久化**：`services/server_service.rs` 3 个 SELECT 列清单 + INSERT/UPDATE bind + `map_row` 读取 `proxy`（serde_json from_str）。
- **字面量补齐**：`services/settings_import.rs` + `rag/service.rs` builtin 的 `ServerConfig` 补 `proxy: None`。
- ⚠️ **运行时未消费**：桌面端 `mcp/stdio_transport.rs` 暂未 spawn proxychains（`grep proxy` 无命中），`proxy` 仅作配置存储 + round-trip，不实际路由流量。待后续 runtime 接入 proxychains 时落地。

#### 3.11.4 边界
- 不影响 `add_server`/`reload_server`/`reinstall_server`（仍各自后台 spawn 连接）。
- `list_servers` 从 DB 重读 config，故仅改 description 的编辑在前端立即反映，无需重连。
- 编译验证：`ORT_SKIP_DOWNLOAD=1 cargo check` 通过（asdf cargo 1.96.0，11m45s）。

#### 3.11.5 手动验证（计划）
- 配一个已连接的 stdio server，编辑仅改 description 保存 → 状态不应变回 starting/connecting，子进程不重启，连接保持。
- 编辑改 command → 应断开重连（旧进程关闭、新进程起）。
- 编辑改 args（仅 stdio）→ 应重连。
- 配一个带 `proxy` 的 server（API/导入 JSON 注入），编辑仅改 description 保存 → DB 中 `proxy` 列值不变（round-trip 生效），不触发重连。

---

### 3.12 RAG 文件导入方式（软链接 / 文件拷贝）+ md5 更新检测 + 批量更新（桌面端独有）

> RAG 文档导入支持选择「软链接」（默认，只记 `original_path` 不拷贝实体）或「文件拷贝」（复制到 `rag/files`），并加 md5 内容指纹做更新检测 + 单条/批量更新流程。详见 `doc/rag_import_method_20260821.md`。

#### 3.12.1 数据模型（DocMeta + 序列化模型）
- **文件**：`src-tauri/src/rag/service.rs` `DocMeta` 加 `#[serde(default)] method: Option<String>`（`"symlink"`/`"copy"`，None=老版本/拷贝兜底）、`original_path: Option<String>`、`md5: Option<String>`。字段进 meta JSON，**不进 DB 列**（RAG 文档不进 DB 表），无需迁移。
- **文件**：`src-tauri/src/models/rag.rs` `RagDocInfo`/`RagDoc` 加 `method`/`original_path`/`md5`/`lost_original`/`content_available`（camelCase，`#[serde(default)]`）；新增 `RagUpdateCheck`（method/hasOriginalPath/originalExists/hasMd5/originalChanged/lostOriginal）+ `BatchPreview`（total/toUpdate/skipped/lost）。`lost_original`（两种 method 都算丢失：`has_original_path && !original_exists`，用于 ⚠️ 徽章 + 批量跳过）；`content_available`（内容现在是否可读：symlink=原始存在、copy=拷贝存在，**决定查看/打开置灰**--copy 原始丢失但拷贝在 -> 查看仍可用，仅无法自动更新）。
- **依赖**：`src-tauri/Cargo.toml` 加 `md-5 = "0.10"`；新增 `compute_md5(bytes)`/`md5_of_file(path)`/`classify_original(meta) -> RagUpdateCheck` helper（集中三条老版本兼容规则：无 method→copy、无 original_path→hasOriginalPath=false、无 md5→originalChanged=true if exists）。

#### 3.12.2 上传 / 导入方式
- **文件**：`src-tauri/src/commands/rag.rs` `upload_rag_doc(app, file_path, tags, method: Option<String>)`（`method` 缺省默认 `"symlink"`）。
- `upload_one_path`/`upload_one_path_inner` 增 `method` 入参：**symlink** 不 `std::fs::write` 拷贝、直接用读到的 raw 走 `reindex_doc`、meta 记 `method=Some("symlink")`/`original_path`/`md5`、不落 `{id}.{ext}`；**copy** 维持现状并补记 `original_path`/`md5`。
- `write_doc_and_index` 签名增 `method`/`original_path`/`md5`（`write_content = method != "symlink"`）；`rag_file_create`（无源文件）记 `method=copy`/`md5`/无 original_path。
- `list_docs`/`get_doc`：symlink 的 `file_name` 为空、content 从 `original_path` 读；计算 `lost_original = has_original_path && !original_exists` + `content_available`（symlink=原始存在；copy=拷贝存在）。

#### 3.12.3 删除 / 打开 / 查看 / 丢失检测
- `delete_doc`：读 meta，**symlink 跳过删 content 候选**（只删 meta + 向量 + tag stats，原始文件不动）；copy 维持现状（删 content + meta + 向量）。
- `open_file_location`：symlink 且 `original_path` 存在 → `reveal_in_file_manager(original_path)`；不存在 → `Err("original file does not exist")`；copy → reveal `rag/files` 拷贝。
- `get_doc`：symlink 从 `original_path` 读（lost 返回空 content）；`reindex_all`：symlink 从 `original_path` 读、丢失则跳过。
- `update_doc`（rag_file_update 工具）：⚠️ **symlink 只读**--`content.is_some()` 时直接 Err（防 MCP 工具覆盖用户原始文件）；丢失 symlink 的 tag-only 更新跳过重索引（防空内容清空向量，chunks 保留旧 tags）；copy 文档 content 变更时刷新 `meta.md5`。

#### 3.12.4 单条更新弹框逻辑
- 新增 `update_doc_from_original(app, id)`（`mode="original"`：从 `meta.original_path` 读字节重新索引 + 刷新 md5 + copy 重写拷贝文件 + version+1；无 original_path / 源丢失 → Err）。
- `update_doc_from_file`（`mode="file"` 手传）：读新 file_path，**记新 `original_path=Some(file_path)` + `md5`**（兼容老版本「新上传需记原始地址」）；method 沿用 meta，老版本 method=None 补 `Some("copy")`。
- 新增命令 `check_rag_update(app, id) -> RagUpdateCheck`（调 `classify_original`，供前端弹框分支）。

#### 3.12.5 批量更新异步任务
- 新增 `preview_batch_update(app) -> BatchPreview`（同步快速扫描分类统计，供前端确认框）。
- 新增 `batch_update_rag_docs(app)`：`tauri::async_runtime::spawn` 后台 `run_batch_update`——逐条 `classify_original`，changed 则 `update_doc_from_original` 重新索引，lost / 无 original_path 跳过。
- `static BATCH_UPDATE_RUNNING: AtomicBool` CAS 守卫（已 running 时再触发 no-op，前端按钮再点只重开弹框）；spawn task 内 RAII guard（Drop 复位，panic 也恢复）+ Err 分支发 `phase="error"` 事件（前端清 running 并显示失败态，**不是** done--避免失败显示绿色完成）。
- 发 `rag://batch-update-progress` 事件 `{ current, total, name, phase: "checking"|"reindexing"|"done" }`（char 级子进度复用 `rag://upload-progress`）。
- 命令注册 `lib.rs`：`check_rag_update`/`preview_batch_update`/`batch_update_rag_docs`；`upload_rag_doc` 加 `method`；`update_rag_doc` 改 `mode`/`file_path`。

#### 3.12.6 前端
- `types/index.ts`：`RagDoc`/`RagDocInfo` 加 `method?`/`originalPath?`/`md5?`/`lostOriginal?`；新增 `RagUpdateCheck`/`BatchPreview`。
- `ragService.ts`：`uploadRagDoc(filePath, tags, method)`；`updateRagDoc(id, {mode, filePath?})`；新增 `checkRagUpdate`/`previewBatchUpdate`/`batchUpdateRagDocs`。
- `tauriClient.ts`：新增 `/rag/docs/check-update`/`/rag/docs/batch-preview`/`/rag/docs/batch-update` 路由；`/rag/docs/upload` 带 `method`；`/rag/docs/update` 改 `{id, mode, filePath}`。
- `useRagData.tsx`：`upload(files, tags, method)`；`updateDoc(id, name, {mode, filePath})`；新增 `checkUpdate`/`getBatchPreview`/`batchUpdate`/`batchUpdateRunning`/`batchProgress`；监听 `rag://batch-update-progress`（done 时清 running + refetch）。
- `RagPage.tsx`：
  - 上传弹窗 `UploadDialog`：导入方式**分段切换**（仿 skill InstallDialog toggle，非 radio）+ `RagMethodHelpIcon`（悬浮/点击 popover）**标题+帮助+切换器同行**；默认 `symlink`。
  - 列表行：method 徽章（`Link2`/`Copy`，hover 原始地址）+ `lostOriginal` ⚠️「原始丢失」标记；**仅「查看」「打开文件夹」按钮置灰**（`hub-icon-btn:disabled` CSS 已补），分片/更新/向量不受影响。
  - 单条更新 `UpdateDialog`：调 `checkUpdate` 后按分支渲染（丢失→手动上传为主按钮；有更新→「从原始」+「手动上传」；无更新→提示+手动上传；老版本无 original_path→仅手动上传+兼容提示）；`runUpdateAction` 调 `updateDoc(mode)`。
  - 头部右上「批量更新」按钮：未运行调 `getBatchPreview` 弹 `BatchUpdateConfirmDialog`（2×2 统计）→ 确认后 `batchUpdate` 启动后台；运行中文字变「查看进度」+ spinner → 重开进度弹框；`BatchUpdateDialog` 用**上传同款双进度条**（文件进度 + 向量进度），可关闭（不中断后台）。
  - `TagSearchSelect`：标签**可搜索的多选下拉框**（输入过滤 + 复选 + 已选 chip + 清除），多选 OR 与文件名搜索 AND 联合；选项列表 `maxHeight:200` + `overscrollBehavior:contain`。
  - 「文档上传」文案统一改「文档导入」（按钮/弹框标题/确认/失败/进度）。
- `index.css`：补 `.hub-icon-btn:disabled`（`opacity:0.45`/`cursor:not-allowed`）+ `:disabled:hover` 还原。

#### 3.12.7 边界 / 兼容
- 旧 `.meta` 无 `method`/`original_path`/`md5` → `#[serde(default)]` 兼容；`classify_original` 对「无 md5→有更新」「无 original_path→走手传兼容分支」先于一切判断；单条手动上传后**必须回写 original_path+md5**，否则下次仍是老版本语义。
- 软链接文档在 `rag/files/` 下**无实体文件**（只存 meta），上传/索引/查看直接从 `original_path` 读字节。
- 编译验证：`ORT_SKIP_DOWNLOAD=1 cargo check` 通过（md-5 v0.10.6）；`npm run build` 通过。
- 版本：`1.0.30001 → 1.0.30002`（tauri.conf.json / Cargo.toml / 根 package.json / frontend package.json / Cargo.lock）。

#### 3.12.8 并发与原子性（六轮复核加固）
- **META_LOCK 顺序锁**：`static META_LOCK: OnceLock<Mutex<()>>` + `meta_lock()` helper。`update_doc_from_original`/`update_doc_from_file`/`update_doc`(rag_file_update)/`delete_doc`/`set_doc_tags` 全程持锁，串行化 meta 读-改-写，根治「批量+单条更新交错写」「批量+删除已删文档复活」两个竞争。**锁序恒为 meta -> runtime**（持 META 后再 await reindex_doc/runtime lock），严禁反向（ABBA 死锁）。
- **`write_meta_atomic(meta_path, &meta)`**：写 `{id}.meta.tmp` + rename 原子替换；崩溃/抢占不可能留下半截 JSON。`.meta.tmp` 扩展名为 tmp，所有扫描器（按 `ext=="meta"` 过滤）天然跳过。`write_doc_and_index`/`update_doc`/`set_doc_tags`/`reindex_all` 的 meta 写全部走此函数。
- **`list_docs` skip 语义**：坏/半截 meta 跳过（`let Ok(..) else { continue }`），与 `recompute_tag_stats`/`run_batch_update`/`preview_batch_update`/`reindex_all`/`find_doc_ids_by_name` 全部扫描器一致--一条坏 meta 不再拖垮整个列表（曾因 `?` 传播导致全列表清空）。
- **批量进度 error phase**：`rag://batch-update-progress` 的 phase 增加 `"error"`（任务早期失败）；前端 useRagData 接受并清 running（不 refetch），BatchUpdateDialog 渲染 ⚠️ 失败态。
- **i18n**：`batchUpdateFailed`/`contentUnavailableView`/`contentUnavailableOpen` 三个新键（4 语言，rag keys=179）。「查看/打开」置灰 tooltip 用通用文案（覆盖 symlink 原始丢失与 copy 拷贝丢失两种成因）。

### 3.13 RAG 文档自动定时更新 + 文档详情 KB 分页 + 分片分页（桌面端独有）

> 三个功能（2026-08-26）：①文档自动定时更新（设置开关+间隔，后台异步线程执行，与手动批量更新共用按钮状态与进度弹框）；②文档详情按 KB 分页加载（默认 200KB/页，「加载更多」逐页追加，防大文档一次加载崩溃）；③分片弹框分页展示（每页 5 片，滚动到底自动加载下一页）。

#### 3.13.1 设置模型（RagSettings 新增 3 字段）
- `src-tauri/src/models/rag.rs` `RagSettings` 加 `auto_update_enabled: bool`（默认 `true`）、`auto_update_interval_secs: u64`（默认 `300`=5 分钟，读取/保存时 clamp `[60, 86400]`）、`doc_load_chunk_kb: u32`（默认 `200`，clamp `[10, 65536]`）。serde `default` 函数 + `Default` impl 同步补齐，旧配置缺 key 自动取默认。
- `get_settings`/`save_settings`（`rag/service.rs`）读写 `autoUpdateEnabled`/`autoUpdateIntervalSecs`/`docLoadChunkKb`；`save_settings_and_rearm` = save + `restart_auto_update_timer`。

#### 3.13.2 自动定时更新（功能1）
- **定时器**：`rag/service.rs` `restart_auto_update_timer(app)`——`tauri::async_runtime::spawn` 常驻循环：读 settings→按 interval 分 500ms 切片睡眠（每片检查 `AUTO_UPDATE_GEN` 代数，设置变更即退出让位新循环）→到点 tick：`is_enabled()` 不满足静默跳过；满足则 CAS `BATCH_UPDATE_RUNNING`（与手动批量更新互斥，手动在跑则跳过该 tick）→ `preview_batch_update` 预扫描，`to_update == 0` **不发事件直接放行**（空转不打扰 UI）→ 否则跑与手动按钮完全相同的 `run_batch_update`（同一进度事件/按钮状态/弹框）。
- **触发点**：`save_settings_and_rearm`（设置保存即重挂定时器，改间隔立即生效）；`start()` 成功尾部（RAG 开启/重启/开机自动恢复）；`lib.rs` 开机 auto-restore 分支兜底。
- **前端状态同步**：`useRagData.tsx` 的 `rag://batch-update-progress` 监听改为**非终态事件也 setBatchUpdateRunning(true)**——自动更新触发时（前端未发起）头部按钮同样变「查看进度」+ spinner，点击重开进度弹框；done/error 清 running + refetch。手动/自动共用 `batchUpdateRunning`/`batchProgress`，天然同步。
- **设置 UI**（`RagPage.tsx` SearchSettingsDialog）：新分区「文档更新与加载」= 自动更新 checkbox（hint 说明与手动共用进度）+ 间隔分钟滑条（1–1440，禁用自动更新时置灰，保存时 ×60 转秒）+ 文档加载大小 KB 滑条（10–65536，step 10）。

#### 3.13.3 文档详情 KB 分页（功能2）
- **后端**：`get_doc_inner(app, id, range: Option<(u64, u64)>)` 统一实现（`get_doc` = 全量；新增 `get_doc_paged(app, id, offset_bytes, limit_bytes)`，`limit_bytes=0` 用设置里的 `doc_load_chunk_kb`）。字节窗口切片**按 UTF-8 字符边界对齐**（offset 向前回退、end 向后扩展到 `is_char_boundary`），永不切出半个多字节字符。`RagDoc` 加 `truncated`（本次是否截断）/`next_offset`（已加载内容末尾的字节偏移，下页传回）/`content_total_bytes`（全量字节，供「已加载 X / 全部 Y」）。
- **命令/路由**：`get_rag_doc_paged` 命令 + `lib.rs` 注册；`tauriClient.ts` `POST /rag/docs/get-paged` → `{id, offsetBytes, limitBytes}`。
- **前端**：`ragService.getRagDocPaged`；`useRagData.view(id)` 改调分页接口（offset 0 / limit 0=默认页大小），新增 `loadMoreView()`（从 `viewedDoc.nextOffset` 取下一页，**append** 到已有 content，刷新 truncated/nextOffset/totals）+ `viewMoreLoading`。`ViewDialog` 加 `moreLoading`/`onLoadMore` props：`doc.truncated` 时在内容区下方渲染 footer（`utf8Bytes(doc.content)` 算已加载量 vs `contentTotalBytes` 总量 + 「加载更多」按钮，loading 时 spinner）。标签保存后 `view()` 重新拉首页（重置分页状态）。

#### 3.13.4 分片分页（功能3）
- **后端**：`get_doc_chunks_inner(id, offset, limit)` 共享核心（读全量 records 排序后内存切片——lancedb 查询 API 无 LIMIT 下推，单文档分片数几百级、代价可忽略）；新增 `get_doc_chunks_paged(id, offset, page_size) -> RagChunkPage {items, total, offset, pageSize}`（`page_size` clamp `[1,200]`）。模型 `RagChunkPage`（`models/rag.rs`）。
- **命令/路由**：`get_rag_chunks_paged` + 注册；`tauriClient.ts` `POST /rag/docs/chunks-paged` → `{id, offset, pageSize}`，响应 transform 分支（offset 版，区别于 page 版）。
- **前端**：`ragService.getRagChunksPaged`；`useRagData.viewChunks` 改取首页 5 片（`CHUNKS_PAGE_SIZE = 5`）+ `chunksTotal`；新增 `loadMoreChunks()`（以 `chunksList.length` 为 offset 追加下一页，`chunksMoreLoading` 守并发）。`RagPage` 分片弹框抽出 `ChunksScrollList` 组件：滚动容器 `onScroll` 距底 <40px 自动 `onLoadMore`；尾部有下一页时显示 spinner / 「加载更多 N 片」按钮（兜底），全部加载显示 ✓ 提示；页脚计数改「已加载 X / 共 Y 片」（`chunksShownCount`）。

#### 3.13.5 i18n（4 语言）
- 新键：`docUpdateSection`/`autoUpdateEnabled`/`autoUpdateHint`/`autoUpdateInterval`/`autoUpdateIntervalHint`/`unitMinutes`/`docLoadChunkKb`/`docLoadChunkKbHint`/`viewLoadedHint`/`viewLoadMore`/`viewLoadMoreHint`/`chunksShownCount`/`chunksLoadMore`/`chunksAllLoaded`。

#### 3.13.6 边界 / 兼容
- 旧配置无新 key → serde default（自动更新默认开、5 分钟、200KB）；`get_doc`（MCP `rag_get` 工具）语义不变仍返回全量。
- 自动更新 tick 与手动批量共用 `BATCH_UPDATE_RUNNING` CAS 守卫——互斥不重叠；RAG 关闭期间 tick 静默跳过（循环不退出，下次 enable 生效）。
- `utf8Bytes` 前端用 `TextEncoder`（与后端字节偏移分页口径一致）。
- 编译验证：`ORT_SKIP_DOWNLOAD=1 cargo check` 通过；`npx tsc --noEmit`（RAG 相关 0 错误）+ `npm run build` 通过。
- `loadMoreView`/`loadMoreChunks` 用 **ref 锁**（`viewFetchingRef`/`chunksFetchingRef`）防同渲染周期内滚动事件堆积导致重复拉页（state 闭包值同步性不足）。
- 版本：维持 `1.0.32003` 不变（用户要求，tauri.conf.json / Cargo.toml / 根 package.json / frontend package.json / Cargo.lock）。

---

### 3.14 RAG 文档导入支持 PDF/Office 解析 + 图片 OCR（桌面端独有，2026-09-05）

> 实施计划见 `doc/rag_office_document_import_plan_20260905.md`（v2.3）。RAG 导入从「仅纯文本」扩展到 **PDF / Word(docx/doc) / Excel(xlsx/xls) / PowerPoint(pptx/ppt) / 常见图片**，提取为 Markdown 后走既有 chunker → embedding → lancedb 管线；内嵌图片过 OCR。查看/分片/搜索与普通文本完全一致。

#### 3.14.1 依赖（相对计划 v2.3 有一处重要偏离）

| 格式 | 计划选型 | **实际落地** | 说明 |
|---|---|---|---|
| PDF | pdf_oxide 0.3 | **pdf_oxide 0.3.77** | `PdfDocument::from_bytes` + `to_markdown_all(&ConversionOptions::default())`（detect_headings + 表格开、图片关——图片自管） |
| Office 六格式 | office_oxide 0.1 | **office_oxide 0.1.9** | `Document::from_reader(Cursor, DocumentFormat)` + `to_ir()`；Markdown 用 crate 自带的 **`DocumentIR::to_markdown()`**（计划写的是自行遍历 IR 渲染，实际直接复用 crate 渲染器更稳）；图片仍自行遍历 IR 收集 |
| 图片 OCR | uni-ocr 0.1.5 | **自研三平台实现（弃用 uni-ocr）** | ⚠️ uni-ocr 的 macOS 后端 `cidre` 构建脚本硬依赖完整 Xcode 的 `xcodebuild`（编译内置 pomace 工程），CommandLineTools 环境无法编译（`xcode-select -p` 无 Xcode）。改为：macOS `objc2`+`objc2-vision`+`objc2-foundation`（纯绑定，链接系统 Vision）；Windows 复用已有 `windows` crate 加 WinRT 特性（Windows.Media.Ocr）；Linux 自调 `tesseract` 子进程（无 uni-ocr 的 unwrap panic 风险）。**对外接口与计划一致**（available/status/image_bytes + OCR_MISSING 哨兵） |
| image | — | **image 0.25**（default-features=false，features=png/jpeg/gif/bmp/webp/tiff） | 校验上传字节 + Linux 端 PNG 重编码；与 pdf_oxide 的 image 子集合并单份编译 |
| zip | Windows-only | **全平台** `{ version="8", default-features=false, features=["deflate"] }` | 从 Windows target 段提升；与 office_oxide 内部声明一致单份编译，砍掉 zstd-sys 等 C 依赖。`commands/runtime.rs` 用法不变 |

macOS OCR 代码参考了 `macocr` 0.4.7 的 Vision 用法（VNRecognizeTextRequest Revision3 + automaticallyDetectsLanguage，中英文自动覆盖）。

#### 3.14.2 提取层（策略模式，`src/rag/extract/`）

- `mod.rs`：`ContentExtractor` trait（`can_handle`/`extract`/`name`）+ 静态注册表 `EXTRACTORS`（优先级：pdf → office → image → text 兜底）+ `run(filename, bytes)` 派发（内部 `tauri::async_runtime::spawn_blocking`，重活不占 async worker）+ `can_extract(filename)`（由注册表推导，供 service 判断「可提取格式」）。哨兵：`UNSUPPORTED_FORMAT:` / `EXTRACT_FAILED:` / `OCR_MISSING:`。
- `ocr.rs`：`OcrStatus { available, platform, distro, engine, missingLangs }`（camelCase 序列化）；`status()` Linux 探测 OnceLock 缓存（`tesseract --version` + `--list-langs` 校验 chi_sim/eng）；`image_bytes(bytes)` 同步（策略已在 blocking 线程内，不再嵌套 spawn_blocking）——先 `image::load_from_memory` 校验，macOS 传原始字节给 Vision，Windows WinRT 流管线（CoInitializeEx MTA + DataWriter→BitmapDecoder→OcrEngine，RPC_E_CHANGED_MODE 容错），Linux 重编码 PNG 写临时文件 + `tesseract <png> stdout -l chi_sim+eng --psm 1`。`clean_text` 统一去空行。
- `pdf.rs`：文字层空（扫描件）时——OCR 可用则逐页取内嵌图 OCR 恢复整篇；不可用报 `EXTRACT_FAILED: pdf has no text layer (scanned document)...`。文字层正常时内嵌图逐张 OCR，以 `> OCR(图片N):` 引用块追加文末（单图失败 log+跳过不阻断）；OCR 不可用但有内嵌图时文末注 `[图片未识别：当前平台 OCR 引擎不可用]`。
- `office.rs`：`DocumentFormat::from_extension`（仅六格式，.ods/.xlsm 落空走文本兜底拒绝）→ IR 深遍历（Table→Row→Cell / List→Item→nested / TextBox / Note 嵌套）收集 `Element::Image.data`，**Emf/Wmf 矢量格式跳过**；OCR 文本 `[图片OCR]` 块追加文末。
- `image.rs`：直连 OCR 核心；空结果报 `EXTRACT_FAILED: no text recognized in image`。
- `text.rs`：原通用文本路径原样搬入（`is_likely_text` 嗅探 + `decode_text`），行为与旧版逐字节一致。

#### 3.14.3 service.rs 打通（4 处）

- **`content_from_source(name, &raw) -> Result<(String, &'static str)>`** helper：`can_extract` → `extract::run`（encoding 标签 "extracted-md"）；否则维持旧嗅探+decode_text（保留真实编码标签进导入摘要）。三个入口（`upload_one_path_inner` / `update_doc_from_file` / `update_doc_from_original`）的 `is_likely_text → UNSUPPORTED_FORMAT → decode_text` 块全部替换。
- **内容文件命名**：extractable 源落盘 `{id}.md`（内容是派生 Markdown），文本源维持 `{id}{ext}`；`content_path_for` 在 candidate 1（`{id}{ext}`）与 legacy `{id}` 之间插入 candidate `dir/{id}.md`。
- **method 强制 copy**：extractable 源在三个写入入口一律 `method = Some("copy")`（内容文件是派生物，symlink 语义无意义）；md5 仍对原始字节计算，更新检测（单条/批量/自动定时）零改动自动覆盖「从原始重提取」。
- **`scan_folder` 8KiB 嗅探**：`can_extract` 命中的文件跳过 `is_likely_text`（PDF 头 8KiB 必有 NUL 会被误杀）；文本类保持嗅探。`file_support.json` 加 15 条扩展（7 文档 + 8 图片，共 196 条）后，扫描对话框才会列出这类文件。
- `is_likely_text`/`rag_log` 提为 `pub(crate)`（text 策略 / extract 模块复用）。

#### 3.14.4 命令 + 前端

- **`get_ocr_status` 命令**（`commands/rag.rs` + `lib.rs` 注册）：返回 `OcrStatus`；`tauriClient.ts` 映射 `GET /rag/ocr-status`；`ragService.getOcrStatus()`。
- **useRagData.tsx**：`ocrMissing` state + `reportOcrMissingIf(err)`（OCR_MISSING 前缀 → 拉状态开弹框，返回是否命中供调用方分流）+ `dismissOcrMissing`。`upload()` 错误分支：`OCR_MISSING` → 弹框；`EXTRACT_FAILED` → toast `pages.rag.extractFailed`（带后端具体原因，如扫描件无文字层）；`UNSUPPORTED_FORMAT` 不变。`RagPage.runUpdateAction` 的 catch 也走 `reportOcrMissingIf`（更新图片文档同样弹框）。
- **RagPage.tsx**：新增 `OcrMissingDialog`（标题/正文/缺失语言包行 + 按 distro 选 apt/dnf/pacman 安装命令等宽块 + 一键复制；macOS/Windows 不触发）；渲染于根级（BatchUpdateDialog 之后）。`UploadDialog` 打开时预检 `getOcrStatus`，`!available` 时在提示区渲染 `ocrPreflightHint` 警示行。
- **fileType.ts**：`EXTRACTED_MARKDOWN_EXTS`（7 文档扩展）——`isMarkdown` 对它们返回 true（查看/搜索片段按 Markdown 渲染），`hlLangFor` 返回 undefined。图片扩展不在其中（OCR 文本按纯文本 `<pre>` 渲染）。
- **locales 四语言**：`unsupportedFile` 改为「无法解析的二进制文件」（去掉"仅支持纯文本"旧说法）、`uploadHint` 更新为全格式支持文案；新增 `extractFailed`/`ocrMissingTitle`/`ocrMissingBody`/`ocrMissingLangs`/`ocrPreflightHint`/`ocrInstallCopy(Failed)`/`ocrInstallHint`/`ocrInstallApt/Dnf/Pacman/Other` + `common.ok`（12+ 键 × 4 语言，rag keys 223→235）。

#### 3.14.5 边界

- 提取文档 size 字段 = 原始源文件字节数（语义不变）；`file_type` 标签走既有 `file_type_label`（PDF/Word 等，来自 file_support.json）。
- `rag_file_create`（MCP 工具）不受影响（输入本就是 UTF-8 文本）。
- OCR 语言固定中英双语（tesseract chi_sim+eng；macOS automaticallyDetectsLanguage；Windows 系统语言兜底 zh-Hans/en-US）。
- `EXTRACT_FAILED` 的 toast 按文件逐条弹（与 UNSUPPORTED_FORMAT 行为一致）。

#### 3.14.6 验证

- `ORT_SKIP_DOWNLOAD=1 cargo check --lib` 通过；Windows/Linux cfg 代码经 scratch-crate 交叉 type-check 通过（本机无法全量 cross-check：blake3/zstd-sys C 构建脚本需对应平台编译器）。
- 临时集成测试 `rag/extract/tests_tmp.rs`（**发布前删除**）：真实 PDF（doc/test/*.pdf 中文样本）+ Python zipfile 构造的最小 docx/xlsx/pptx（中文/表格/多 sheet/幻灯片）+ 空 PNG OCR 管线（预期 EXTRACT_FAILED）+ 文本兜底回归。
- 前端 `npm run build` 通过。
- 真机手动验证（用户）：`tauri dev` 后 RAG 页上传 pdf/docx/xlsx/pptx → 导入/分片/搜索/查看（Markdown 渲染）链路 + 扫描件 PDF 失败 toast + Linux tesseract 缺失弹框。
- 版本：`1.0.33002 → 1.0.33101`（tauri.conf.json / Cargo.toml / 根 package.json / frontend package.json + Cargo.lock）；changelog `doc/upgrade/1.0.33101.md`。

#### 3.14.7 复合验证（2026-09-01，3 轮）

- **Round 1 提取策略内部**：`ext_format` 点号修复生效（点号文件名 "pdf" 不再被 office 误认）；`ConversionOptions::default()` 确认 = 语义模式（detect_headings=true、extract_tables=true、include_images=false，图片由 extract_images 单独处理，无双提取）；macOS Vision `autoreleasepool`（ocr.rs）生效，spawn_blocking 线程池复用不泄漏 autorelease 对象；image.rs（独立图片无文本 → EXTRACT_FAILED 合理）、text.rs（NUL sniff 历史路径 byte-for-byte 保真）、office.rs（Emf/Wmf 跳过、Table/List/TextBox/Footnote 递归完整）、mod.rs（spawn_blocking + 策略归因日志 + sentinel 规范）逐文件审查通过。
- **Round 2 service.rs 集成**：全链路验证——`content_path_for` 四候选（`{id}{ext}`→`{id}.md`→`{id}`→`{name}`）对全部场景闭环（text.md 内容与 extractable 提取内容同路径不冲突、pdf→txt/pdf→md 互转无孤儿、rag_file_create 按名命名走候选4）；upload/update/manual/original/batch/auto-update 全部复用同一 `content_from_source` 派发链；scan_folder 对 extractable 跳过 NUL sniff（PDF 头部有 NUL）；update_doc_from_file 原本就有旧 content 删除（早期怀疑的孤儿 bug 是窗口截图不全导致的误报，未引入改动）。
- **Round 3 回归**：`cargo check` 0 错 0 警；`cargo test --lib` 19 passed；临时冒烟测试（真实 PDF doc/test/苏州2.pdf 提取成功、点号名拒绝、损坏 PDF EXTRACT_FAILED sentinel、二进制拒绝、文本往返、最小 docx 提取成功）全过，**跑完已删**；`npx tsc --noEmit` 24 错误 = 基线 24（全存量）；`npm run build` 通过。
- 本轮复合验证结论：**未发现新的逻辑漏洞**；Round 1 期间修复的 2 处（ext_format 点号误认、update_doc_from_original 按显示名派发）+ delete/open/reindex 的 content_path_for 收敛已覆盖全部发现的问题。

#### 3.14.8 Windows CI 构建失败修复（2026-09-07，windows 0.62 API 对齐）

> 症状：Windows runner（`windows-latest`）CI 构建失败——E0432（`windows::Win32::System::Com` 未启用 feature）+ 9 个 E0433（`OcrEngine`/`Language`/`BitmapDecoder`/`DataWriter`/`InMemoryRandomAccessStream` cannot find type）+ 4 个 unused import 警告。macOS/Linux 不受影响。

**根因两处**：

1. **feature 缺失**：`Cargo.toml` 的 windows crate features 未启用 `Win32_System_Com`（`CoInitializeEx`/`CoUninitialize` 公寓初始化需要）。
2. **import 作用域错位**：WinRT 类型 import 写在了 `recognize()` 里，但使用点全在 `recognize_inner()` 里——类型不可见时编译器不检查调用点类型，把真正的 API 不匹配全部掩盖了。

**scope 修复后暴露的 windows 0.62 API 不匹配（scratch-crate 交叉 type-check 发现，共 10 处）**：

| 原写法（错误） | windows 0.62 正确 API |
| --- | --- |
| `match CoInitializeEx(..) { Ok(()) => .., Err(e) if e.code().0 == .. }` | 返回 `HRESULT`（非 Result）：`hr.is_ok()` / `hr == RPC_E_CHANGED_MODE`（常量在 `Win32::Foundation`） |
| `TryCreateFromUserProfileLanguages()?.or_else(..)` | 返回 `Result<OcrEngine>`（无内层 Option）→ engine 回退链改 Option 链（`.ok()` + `.or_else(││ ..)`） |
| `Language::new(&HSTRING)` | `Language::CreateLanguage(&HSTRING) -> Result<Language>` |
| `TryCreateFromLanguage(&l).ok().flatten()` | 返回 `Result<OcrEngine>` → `.ok()` 即可（无 flatten） |
| `DataWriter::CreateAtStream(&stream)` | `DataWriter::CreateDataWriter(&stream)`（`P0: Param<IOutputStream>`，`&InMemoryRandomAccessStream` 直接满足） |
| `IAsyncOperation::get()`（5 处阻塞等待） | windows-future 0.3 改名 `.join()` |

**验证**：macOS `ORT_SKIP_DOWNLOAD=1 cargo check --lib` 通过（回归不受影响）；scratch crate（windows 0.62.2 + 同 feature 集）对 `x86_64-pc-windows-msvc` target 交叉 type-check 通过（跑完已删）。

### 3.15 SQLite 全文索引（FTS5 + 中英/拼音分词）+ 活动日志筛选优化（桌面端独有，2026-09-05）

> 执行计划与五轮复核记录见 `doc/sqlite_fts_pinyin_plan_20260905.md`（Phase A-G 全部完成）。兑现 `doc/sql_index_rag_search_plan_20260823.md` 中「FTS5 暂不引入」的可选项。

#### 3.15.1 概述

- **FTS5 全文索引**：bundled SQLite（libsqlite3-sys 0.37 无条件 `SQLITE_ENABLE_FTS5`）建 7 张 FTS5 虚拟表 `fts_{servers,groups,rag_docs,skills,prompts,resources,app_log}`，列 = `ref_id UNINDEXED + zh + py + ini`。
- **分词方案（方案 B，写入时 Rust 分词）**：charabia 0.10 全语言（default features + `latin-camelcase`/`latin-snakecase`，**严禁** `chinese-normalization-pinyin`）+ pinyin 0.10（feature `plain`）。`fts_service::tokenize_fields(text) -> (zh, py, ini)`：charabia tokenize → `is_separator()` 过滤 → `lemma()`（kvariants 繁体规范形 + lowercase）→ 含 CJK 词元经 `ToPinyin`（char trait，`c.to_pinyin() -> Option<Pinyin>`，`plain()`）逐字转全拼/首字母。
- **查询路由** `build_match_query`：含 CJK → `zh:("词1" "词2"*)`；纯 ASCII → `(py:"t"* OR ini:"t"* OR zh:"t"*)`（每 token 一组）；混合 → AND 连接；token `"`→`""` 转义防注入；空/纯符号 → None（调用方查全部）。**已知边界**：拼音词中前缀搜不到（FTS5 前缀只从 token 头匹配）；多音字取默认读音；zh 列实际存繁体规范形（"数据"→"數据"），写入/查询同管道归一故简繁输入互通。
- **迁移前备份**：`db::initialize` 在 `run_pending` 前调 `migration::backup_before_migration`——`has_pending()` 为真时 `VACUUM INTO ?1` 生成 `mcphub.db.bak`（先删旧 .bak，路径 bind 参数防引号注入）；无待迁移 no-op 不覆盖。回滚：关应用 → `.bak` 覆盖 `mcphub.db`（删 `-wal`/`-shm`）→ 重启。

#### 3.15.2 fts_service 模块（`src/services/fts_service.rs`）

- `FtsTable` 白名单枚举（Servers/Groups/RagDocs/Skills/Prompts/Resources/AppLog），SQL 由 `format!.leak()` 生成 `&'static str`（sqlx 0.9 约束），每种语句每表仅泄漏一次。
- **FTS5 无 UPDATE/无 WHERE 删除**：`sync_upsert(_tx)` = 查 rowid → 命中则 DELETE → INSERT；`sync_delete(_tx)` = 查 rowid → 删；`clear_table(_tx)` 全表删。`_tx` 版本接收 `&mut sqlx::SqliteConnection`，供调用方与源表写**同事务**提交（§4.3 一致性铁律）。
- `search_ref_ids(table, input, limit)`：MATCH + `ORDER BY rank` + ref_id 投影。`table_is_empty` 供空表兜底；`clear_table` 供清理。
- `rebuild_all()`（lib.rs setup 后台 spawn，fire-and-forget）：只重建 5 张实体表（fts_rag_docs 归 RAG 服务、fts_app_log 归日志写路径），逐实体计时日志 `[fts] rebuild fts_x: N rows in Xms`；`rebuild_one(pool, table)` 读 `SELECT *` 按 TEXT 列拼接（P3 多字段单空格连接）。
- `backfill_app_log_if_empty()`：fts_app_log 为空且 app_log 非空 → 单事务全量回填（升级后首次启动）。
- **charabia Tokenizer 生命周期**：`Tokenizer<'tb>` 借用 builder → `Box::leak(Box::new(TokenizerBuilder::default()))` 得 `Tokenizer<'static>`（builder 默认值是 Owned Cow 不实际借用数据），OnceLock 持有。

#### 3.15.3 写路径同步清单（§4.3 铁律，全部同事务）

| 实体 | 同步点 | 说明 |
| --- | --- | --- |
| servers | `server_service::create/update/delete` | upsert（text=name+description）；**update 比对 name，改名=删旧插新**；`search_configs` 升级 FTS+rank |
| groups | `group_service::create/update/delete` | upsert（text=name+description，ref_id=id）；`search_paged` 升级（FTS rank 序 + Rust 分页） |
| rag_docs | `rag/service.rs` 的 `upsert_doc_sql`/`remove_doc_sql`/`rebuild_rag_sql_index`（全部写路径收敛于此） | upsert/delete/clear+insert（ref_id=id，text=name）；`search_docs_paged` 升级（FTS rank 序 + tag 白名单 Rust 过滤 + .meta 富化） |
| skills | `skill_service` 安装 ok 信号 / 导入失败删 / reconcile 两处删 / uninstall | **特例**：安装为 FS 耦合多步非事务流，FTS 同步 best-effort（失败不回滚安装）；删除路径同事务；`search_library_paged` 升级（dir_name IN + FS 过滤 + rank 序） |
| prompts/resources | `prompt_service`/`resource_service` create/update/delete | ref_id=name → **update/delete 前同事务先查 name**（改名删旧插新）；`search_paged` 升级（enabled 过滤 Rust 侧） |
| app_log | `add_log`（唯一写入口）/`clear_logs`/`cleanup_old_logs` | add_log 同事务 upsert；clear_logs 同事务清空（VACUUM 在事务外）；cleanup 先取待删 id 集 → QueryBuilder 批量 `rowid IN (SELECT rowid WHERE ref_id IN)` 两步删 |

#### 3.15.4 应用日志全文检索（Phase F）

- `LogQuery` 加 `search: Option<String>`；`query_logs` search 非空时走 `search_ref_ids(AppLog)` → `QueryBuilder` IN 回表（含 level/server_name 过滤）→ FTS rank 序 → 内存分页；空表/Err 降级 `LOWER(message) LIKE`。
- `cleanup_old_logs` 先 `SELECT id WHERE created_at < cutoff` 取待删集 → 同事务删 app_log + 按 ref_id 批量删 fts_app_log。
- 前端契约不变（`/logs` 响应结构未动；`search` 为可选新增）。🆕 **2026-09-06：日志页搜索框接入后端**——此前 `LogViewer` 仅对已拉取的最新 50 条做客户端子串过滤（`/logs` GET 在 tauriClient 硬编码 `query:{}`，后端 FTS 从未被 UI 调用），多词输入「检查 更新」按整串 includes 几乎必空（实测最近 50 条恰含「检查」的仅 1 条 playwright 日志）。现 `fetchLogs(search?)` 带 `?search=&pageSize=200` → tauriClient 解析 query → `get_logs` FTS weighted（相关度优先、同数 created_at DESC）；`LogViewer` 桌面端搜索态走后端结果（仅再套 type/source 过滤，300ms 防抖 + 竞态守卫），web 端保留原客户端过滤行为。
- `[http-server-watch]` heartbeat 周期调试任务已删（此前每 30s 一条刷屏）；保留 3 处一次性事件日志（SSE 流结束/serve task 结束）。

#### 3.15.5 活动日志筛选优化（Phase G）

- **去掉「用户」筛选**：ActivityPage username 筛选块删除（桌面端无用户追踪语义）。
- **四条件可搜索分页下拉**：新组件 `components/ui/SearchableSelect.tsx`（loadOptions(search,page,pageSize) → {options,total}；输入防抖 300ms；滚动到底加载下一页；请求序号竞态守卫；键盘上下/回车/Esc；点击外部关闭；选中值清除）。server/tool/group/keyName 四筛选接入，候选来自新命令 `get_activity_filter_options(field, search, page, page_size)`（field 白名单枚举 server/tool/group/keyName → DISTINCT + LOWER LIKE + LIMIT/OFFSET）。
- `tauriClient.ts`：`get_tool_activities` 透传 `group`/`keyName` → args `groupName`/`keyName`（此前被静默丢弃）；stats 同步透传（`get_activity_stats` Rust 命令加 group_name/key_name 参数，统计条与筛选联动）；新增 `activities/filter-options` 路由 + `FilterOptionsPage` 响应转换。
- 旧 `get_activity_filters` 命令保留（web 兼容），前端不再调用。

#### 3.15.6 验证

- `ORT_SKIP_DOWNLOAD=1 cargo check` 通过（0 错 0 警）；`cargo test --lib` 33 passed（含 12 个 fts_service 单测：分词/拼音/注入/round-trip；v24 迁移幂等+备份语义测试；rebuild_one 行数对账测试）。
- `npm run build` 通过（813ms）；`npx tsc --noEmit` 24 错误 = 基线 24（零新增）。
- 版本：`1.0.33101 → 1.0.33102`（tauri.conf.json / Cargo.toml / 根 package.json / frontend package.json + Cargo.lock）；changelog `doc/upgrade/1.0.33102.md`（后与 1.0.34001 合并为单文件 `doc/upgrade/1.0.34001.md`，原文件已删）。
- 手动回归（E4，待用户）：中文/拼音/首字母搜索命中、server 改名/删除后搜索即时正确、旧库升级生成 .bak、活动日志四下拉真过滤。

#### 3.15.7 五轮代码级复核（2026-09-05/06）

> 用户要求对已完成的 FTS 功能做 5 轮详细复核（逻辑/性能/一致性/扩展性/回归，精准到代码级）。共发现 5 个问题，修复 4 个、验证放行 1 个。

**第 1 轮（逻辑正确性）**

- **F1 [严重，已修复]** `rebuild_one` 用 `SELECT *` 拼接源表全部 TEXT 列 → servers 表会把 `env`/`headers`/`openapi`/`proxy` JSON（**含密钥与超大 spec**）索引进 FTS（违反 P3 列白名单规范 + 敏感信息进索引 + 索引膨胀）；prompts 的 `template`、resources 的 `content` 同理。
  - 修复：`FtsTable` 新增 `text_columns()` 返回每表白名单列（servers/groups=name+description、rag_docs=name、skills=dir_name+name+description、prompts=name+title+description、resources=name+uri+description、app_log=message），`rebuild_one` 改为 `SELECT {ref_column} AS fts_ref, {白名单列...} FROM {src}`，按位（0=ref，1..=白名单列）读取拼接，NULL/空列跳过、非 TEXT 列容错。
- **F3 [已验证放行]** FTS5 多 token 隐式 AND 语义疑点：`zh:("數据" "庫"*)` 空格分隔多短语是否隐式 AND。用临时集成测试探针（建 FTS5 表 + charabia 同参数分词插入 + 三种查询形态）验证：多短语=隐式 AND（只命中同时含两词的行）、`zh:"a b"`=相邻 token 短语匹配、单 token 通配=前缀匹配，语义符合计划 §2 查询路由设计。**探针跑完已删**。

**第 2 轮（性能与资源）**

- **F2 [高，已修复]** 所有 `sql_*` SQL 生成函数每次调用 `format!(...).leak()` 泄漏——`add_log` 高频路径每写一条日志泄漏 3 个串（delete_by_rowid/insert/select_rowid），长期运行真实内存泄漏。
  - 修复：SQL 生成区整块替换为 `FtsSql` 缓存结构体（`delete_by_rowid`/`insert`/`select_rowid`/`search`/`count`/`clear` 六个 `&'static str` 字段），`sqls(t) -> &'static FtsSql` 经 `OnceLock<Vec<FtsSql>>` + `FtsTable::ALL` 常量数组 + `idx()` 下标，**进程内只格式化一次**；同时删除重复/死代码（`sql_tx_delete_delete` 与 `sql_upsert_delete` 重复、`table_is_empty_sql` pub 未用）。
- **F4 [已识别，按需优化]** servers/groups 搜索 FTS 命中路径 fetch 全表再 Rust 过滤；量大时可改 `WHERE name IN (ids)` 回表。当前表量级（几十~几百行）不构成瓶颈，保留现状。
- **F5 [已随 F2 解决]** 死代码清理（见 F2）。

**第 3/4/5 轮（一致性/扩展性/回归）**：写路径事务边界复核（settings_import 复用 server_service::create 继承 FTS 同步；7 张源表无 service 外直接写者；migration seed 由启动 rebuild_all 对账兜底）、新实体接入同步点核对（FtsTable 枚举/ALL/text_columns/REBUILD_TABLES 四处）、`cargo check`/`cargo test --lib` 33 passed/`npm run build` 回归通过。

**收尾验证（复核修复全部落盘后）**：`cargo check` 0 错 0 警（含清除 `Column` 冗余导入）；`cargo test --lib` **33 passed / 0 failed**（12 个 fts_service 单测全过，含 `rebuild_one_reconciles_servers` 白名单路径回归）；`npm run build` 通过（839ms）；`npx tsc --noEmit` 24 = 基线。

#### 3.15.8 多词 OR 语义 + LIKE 降级 + 相关度优先排序（2026-09-06）

> 用户实测反馈驱动的四项修复：①多词查询查不到数据；②RAG 搜「中心数据」查不到存量文档；③要求搜索结果按相关度（命中词数）优先排序。

**① build_match_query 多 token 改 OR 语义**
- 原实现：多 zh token 生成 `zh:("a" "b"*)` 空格分隔 = FTS5 隐式 AND（必须全词命中）→「中心数据」只命中同时含两词的行，用户感知为"查不到"。
- 修复：组内多 token 改 `zh:("中心"* OR "數据"*)`；zh 组与 ascii 组之间也 OR。单 token 形态不变。`match_query_cjk_route` 单测更新为断言 OR + 每 token 带 `*`。
- charabia 行为注记：kvariants 归一简→繁（数据→數据），查询与写入同管道，简繁互通；归一不完全映射（中心→中心不变）。

**② 全部 7 个搜索入口零结果降级 LIKE**
- 原实现：FTS 命中 0 条时硬返回空。修复：`Ok(weighted)` 为空 / 空表（`table_is_empty`）/ `Err` 三种情况均落到原 LIKE 子串查询（补 FTS 词前缀只能从 token 头匹配的 CJK 内部子串盲区）。仅 `Err` 写 `[fts]` 警告日志，零结果静默降级。

**③ fts_rag_docs 存量回填缺口（用户 DB 实测发现）**
- 现象：用户 DB 中 `fts_rag_docs` 0 行而 `rag_docs` 8 行——`rebuild_rag_sql_index` 仅在 RAG enable 时运行，存量文档从未入索引。
- 修复：临时集成测试回填 8 行（跑完已删）；应用重启 + RAG enable 时 rebuild 自愈清空重灌。

**④ 相关度（命中词数）优先排序**
- 需求：命中查询词元数第一优先——双词命中必须排在单词命中之前；同相关度层内保持各表原生序。
- 实现：`fts_service` 抽出 `extract_tokens()`；新增 `search_ref_ids_weighted()`（逐 token 独立索引查询合并命中数，count 降序），`search_ref_ids()` 变为薄包装。不用单条 OR + bm25（文档长度/IDF 归一化会让稀有单词命中压过双词命中）。
- 调用方改造（全部 8 处）：`server/group/prompt×2/resource/skill/log/rag` 命中分支改为 `HashMap counts` + `sort_by_key(Reverse(count))` **稳定排序**保留表原生序 tiebreak（servers/groups/prompts/resources=name ASC；skills=dir_name ASC；logs=created_at DESC；rag=uploaded_at DESC,id）。
- `search_ref_ids_weighted_on(table, pool, ...)` 内部版本可注入 pool（单测用内存池），生产走全局池包装。
- 新增单测 `weighted_orders_by_token_hits`：双 token 命中行排前、单 token 命中行排后，期望命中数用 `extract_tokens` 同口径动态计算（不硬编码切词结果）。
- 途中修复：`resource_service.rs` `counts.contains(n)` → `contains_key(n)`（HashMap 无 `contains`）。

**验证**：`cargo check` 0 错 0 警；`cargo test --lib` **34 passed / 0 failed**；`npm run build` 通过。**用户需重启应用**（旧二进制在跑；重启后 RAG enable 触发 rebuild 自愈索引）。

#### 3.15.9 服务器搜索相关度排序被命令层破坏的修复（2026-09-06）

> 用户实测："idea sse stream" 排序第一是 `Idea-mcp-server`（只命中 1 词），`-sse`/`-stream` 变体（各命中 2 词）反而在后面。

**根因**：§3.15.8-④ 只升级了 service 层（`server_service::search_configs` 返回 weighted 序），但命令层 `commands/servers.rs::search_servers` 在「工具名兜底候选合并」后执行 `candidates.sort_by_key(|c| c.name.to_lowercase())`——**按名字母序重排，把 weighted 相关度顺序完全覆盖**。`Idea-mcp-server` 恰好是三个 `Idea-*` 中字母序最靠前的，故排第一。真实 DB 模拟证实 weighted 计数：`Idea-mcp-server-sse`(idea+sse=2)、`Idea-mcp-server-stream`(idea+stream=2)、`Idea-mcp-server`(idea=1)。

**修复（`commands/servers.rs::search_servers`）**：
- 命令层重新调 `fts_service::search_ref_ids_weighted(Servers, key, 1000)` 取 name/description 相关度计数（与 search_configs 主路径同源；FTS5 索引查询 µs 级，重复查询代价可忽略）。
- **FTS 降级路径补齐计数**：weighted 为空（LIKE 兜底）时，用 Rust 侧空白分词对 name+description 逐 token 计数——LIKE 候选必含完整 key（=全部 token）计数相同，稳定排序保持 name 序；防止工具名命中反超名称命中。
- **工具名兜底升级为 token 级计数**：原实现要求完整 key 子串匹配工具名（多词查询几乎永不命中），改为逐 token 计数（提高召回），并入同一 counts。
- **统一排序**：删除按名重排，改 `sort_by_key(Reverse(counts[...]))` 稳定排序——命中 token 数降序，同数保持候选进入序（FTS 候选=weighted+name 序；LIKE 候选=name 序；工具兜底=list_all 序）。
- 语义注记：FTS 路径下工具名命中的服务按自身命中数参与全局排序（如工具含 2 个查询词的服务会排在仅名称命中 1 词的服务之前）；降级路径名称命中（全 token）恒在工具命中之前。

**验证**：`cargo check` 0 错 0 警；`cargo test --lib` 34 passed；真实 DB 模拟 "idea sse stream" 最终排序 = `Idea-mcp-server-sse`(2) → `Idea-mcp-server-stream`(2) → `Idea-mcp-server`(1)。

#### 3.15.10 清空日志死锁修复 + 活动页状态筛选统一样式（2026-09-24）

**① 清空日志失败（`DELETE FROM fts_app_log` 等锁 5.2s 后 database is locked）**

- **根因**：`log_service.rs::clear_logs` 在事务 `tx` 内先执行 `DELETE FROM app_log`（tx 连接持有 SQLite 写锁），随后调用的却是 `fts_service::clear_table`（**pool 版**，即另一连接）执行 `DELETE FROM fts_app_log`——第二个连接等 tx 释放写锁，而 tx 又在等 clear_table 返回后才 commit，**死锁**；sqlx SQLite 默认 busy_timeout 5s，超时后报 `database is locked`，整个清空失败（tx 回滚，app_log 也没删掉）。日志表现为 slow statement 警告（elapsed≈5.2s、rows_affected=0）。`cleanup_old_logs` 一直用的是 `&mut *tx` 同事务两步删，无此问题。
- **修复**：`clear_logs` 改用 `clear_table_tx(&mut tx, FtsTable::AppLog)` 同事务执行；**删除 pool 版 `clear_table`**（fts_service.rs，已无调用方）——它是本次死锁的隐患源头，保留会诱使未来调用方重蹈覆辙，`clear_table_tx` 文档注释已加警示。
- **验证**：`cargo check` 0 错 0 警；`cargo test --lib` 51 passed / 0 failed。

**② 活动页「状态」筛选框样式统一（去掉原生 datalist）**

- 原实现：`ActivityPage.tsx` 状态筛选用 `<input type="text">` + 原生 `<datalist>`（各平台渲染不一致、与其余四个 SearchableSelect 筛选框视觉割裂）。
- 修复：`SearchableSelect.tsx` 新增 `searchable?: boolean` prop（默认 `true`）——`false` 时下拉面板不渲染搜索输入框，打开即加载全部候选（状态只有固定两项无需搜索）。`ActivityPage.tsx` 状态筛选改用 `<SearchableSelect loadOptions={loadStatusOptions} searchable={false}>`，候选为固定翻译标签（`activity.statusSuccess`/`statusError`）；`handleSearch` 既有的「翻译标签 → raw status」映射逻辑不变，天然兼容。顺带清理死代码：`STATUS_OPTIONS`/`isValidStatus`（后者本就无调用方）与 `ActivityStatus` import。
- **验证**：`npx tsc --noEmit` 24 = 基线（改动文件 0 错误）；`npm run build` 通过。

### 3.16 已知 agent 目录补齐（对齐上游 vercel-labs/skills，2026-09-07）

> skills 页的「已知 agent」catalog（`runtimes/skill/install.json`，编译期 `include_str!` 进二进制）对齐上游 `vercel-labs/skills` `src/agents.ts` 的 77 个 agent：补齐 19 个缺失条目（Amp/Antigravity/Antigravity CLI/Cline/Codex/Cursor/Deep Agents/Dexto/Firebender/Gemini CLI/GitHub Copilot/Kimi Code CLI/Loaf/MiniMax Code/OpenCode/Posit Assistant/Replit/Warp/Zed）+ 桌面端自定义两个通用目录条目：`"Common Agent": ".agents/skills"` 与 `"Common Agent Config": ".config/agents/skills"`（`.agents` 与 `.config/agents` 均为通用型目录，被 Cline/Dexto/Kimi Code CLI/Loaf/Warp/Zed、Amp/Replit 等共用；为免复合名过长且不指向具体 agent，统一以通用名展示）。**同路径合并**：catalog 内路径不允许重复，共享同一目录的 agent 合并为一条 `/` 分割的复合名（`Qoder/Qoder CN`、`Trae/Trae CN`、`Zencoder/Zenflow`），最终 65 条。**有意跳过** Eve / PromptScript（项目级 cwd 路径，桌面 home 扫描不适用）与 Universal（meta 条目，`~/.agents/skills` 语义已被 `.agents/skills` 复合条目覆盖）。新增条目路径采用上游 `globalSkillsDir`（用户级目录，符合桌面扫描语义）；存量条目路径（如 Devin/Crush/Goose 用项目级 `skillsDir`）保持不动，避免影响已持久化配置。

- **catalog**：`src-tauri/runtimes/skill/install.json` 65 条（同路径复合名合并后）。`skill_service::default_agents()` 自动解析；slugify 对 `/` 产出连字符，全部 id 无冲突（如 "Common Agent" → `common-agent`）。
- **存量库回填**：`db/migration.rs` 新增 `migrate_v25`（`TARGET_VERSION` 24 → 25，`apply_migration` 加 `25 => migrate_v25`，配套 `migrations/0025_generic_agent_backfill.sql` 占位）。**为何配置变更也要迁移**：agents 列表由 v13/v14 持久化进 `system_config.config_json.skills.agents`，`list_agents` 仅在该键缺失时才回退内置 catalog——存量库已存 56 条快照，不迁移则永远看不到新条目（含 Common Agent 与 19 个补齐条目）。v25 复用 v14 的「用户自定义」分支：只追加缺失的 catalog id（幂等），保留用户已有项与顺序。
- **替代方案（已否决）**：运行时在 `list_agents` 动态合并 catalog——用户删除内置 agent 会被每次调用「复活」，语义更差。
- **验证**：`cargo check` 通过；`cargo test --lib migration` 4 passed（v21/v23/v24 回归）；catalog JSON 解析 + 上游 77 agent 全量 diff（missing: none）。
- 用户操作：重启应用后 DB 迁移自动执行，skills 页 agent 列表出现 20 个补齐条目（含 `.agents/skills` 复合条目，其名内含 Common Agent；目录不存在时安装/导出按既有逻辑处理）。

### 3.17 RAG 数据源统一 + Git 数据源 + 树形列表视图（桌面端独有，2026-09-09）

> 设计与执行计划见 `doc/rag_data_source_tree_plan_20260909.md`。三大需求：①导入入口统一为「数据源选择」（文件/文件夹只是选项）；②新增 Git 数据源（gix 纯 Rust 集成、支持认证）；③列表增加树形视图（存量归「文件」节点）。版本 `1.0.35002`（后并入 1.0.35003 发布，1.0.35003 的 changelog 又随 2026-09-24 基线同步并入 `doc/upgrade/1.0.39001.md`，原 `1.0.35003.md` 已删）。

#### 3.17.1 数据模型 + 存量兼容（P1）

- `models/rag.rs`：`DocSource`（kind/label/root/relPath/git{url,branch,commit}）+ `RagGitSource`（id=repo_hash/url/branch/label/addedAt，前端下拉用）；`DocMeta.source: Option<DocSource>`（serde default，旧 meta 兼容，**不进 DB 无迁移**）；`RagScanFile.relPath` + `RagFolderScan.commit`（git 扫描的 HEAD sha，展示用）。
- `rag/service.rs::classify_source(meta)`（集中式 helper）：有 source → 直接用；存量有 original_path → kind="folder"（label=父目录）、无 → kind="file"（label=「文件」）。**惰性分类**（读时填充 `RagDocInfo`/`RagDoc` 的 sourceKind/Label/Root/relPath/gitUrl/gitBranch 6 展示字段），不改写 meta 文件。
- 前端 `types/index.ts`：`RagDocInfo`/`RagDoc`/`RagScanFile`/`DocSource`/`RagGitSource` 对应字段。

#### 3.17.2 写路径注入（P2）+ rag_file_create 不覆盖（P2b）

- `write_doc_and_index` 加末位 `source` 参数（全部 4 调用点）；`upload_rag_doc` 命令加 `source: Option<DocSource>`；手动/from-original 更新保留原 source。
- `rag_file_create` 注入 kind="tool" label「MCP 工具」（新工具文档归独立树节点）。
- **P2b 不覆盖**：删除 `create_doc_from_content` 的 `find_doc_ids_by_name` 覆盖块（文件/向量/meta/SQL 清理 + rag_log overwriting），重名一律并存；`find_doc_ids_by_name` 函数随之删除（无调用方）。

#### 3.17.3 Git 集成（P3，`src-tauri/src/rag/git.rs` 新模块）

- **gix 0.73**（gitoxide，纯 Rust，无外部 git 依赖）：features `blocking-network-client` + **`blocking-http-transport-reqwest-rust-tls`**（⚠️ blocking-network-client 不带 HTTP backend，必须显式加；gix-transport 0.48 用 reqwest 0.12 blocking 与项目主 reqwest 0.13 共存，已验证归并 OK）+ `worktree-mutation`。
- **Clone API 要点**：`gix::clone::PrepareFetch::new(url, dest, Kind::WithWorktree, create::Options, open::Options)` → `.with_ref_name(Some(branch))`（⚠️ 选 checkout 分支用 ref_name，`with_remote_name` 是设 remote 名不是分支）→ `.with_shallow(Shallow::DepthAtRemote(NonZeroU32))` → `fetch_then_checkout(...)` + `main_worktree(...)`（⚠️ `fetch_only` 不 checkout 工作区，树是空的——冒烟测试踩过）。
- **认证**：`build_clone_url` 把账密 percent-encode 注入 URL userinfo——gix HTTP transport 读 `url.user().zip(url.password())` 直接作 basic auth（已读 gix-transport 0.48 源码确认）。仅支持 https；拒内嵌凭证 URL。
- **两段式存储**：clone 到 OS 临时目录 `std::env::temp_dir()/mcphub-rag-git/{repo_hash}`（clone 前清临时根）；确认导入时 `ensure_persisted` copy 到 `<app_data>/rag/git/{hash}` + `map_temp_to_persistent` 改写上传路径（doc 的 original_path 落持久目录，md5 更新检测/open-location 天然可用）。取消导入零残留（OS 自愈临时目录）。
- **更新刷新**：`refresh_persistent` = 重克隆到 `{hash}.new` + 原子 rename swap（main→.old→删）；`sweep_stale_dirs` 清崩溃残留（lib.rs setup 调用）。浅克隆 depth=1 下重克隆比 fetch 便宜且免疫 dirty-tree/shallow-fetch 边界。
- **凭证存储（2026-09-16 起唯一持久化）**：本地文件 `<app_data>/rag/git-credentials.json`（map: repo_hash → {username,password}，原子写 tmp+rename，unix 0600，`creds_lock()` RwLock 串行读改写）。**为何弃用 OS keyring（2026-09-16）**：macOS/Windows 每次 keyring 读取会弹授权确认框，自动更新几分钟一次直接把无人值守流程打死；改本地文件后零交互（文件在应用数据目录内，权限 0600）。接口：`store_credential(app,hash,u,p)`/`load_credential(app,hash)`/`delete_credential(app,hash)`/`store_credential_for(app,url,u,p)`（canonicalize 后取 hash，需 AppHandle）。**数据源注册表（rag/sources.json + RegistryEntry）已删除**——每次导入重新输入地址，不独立维护数据源列表；refresh 无 branch（重 clone 用远端 HEAD）。keyring crate 依赖已从 Cargo.toml 移除。
- **错误约定**：`GIT_AUTH_REQUIRED:<detail>` 前缀结构化错误（`is_auth_error` 按消息关键字分类 401/403 等）；前端表单内联报错 + 自动展开账密区，无独立弹框。
- **canonicalize_url 走 `.no_proxy()`（2026-09-16 修复）**：reqwest 0.13 默认读系统代理（macOS Clash 7890 等）——即使用户已把 `*.haidaifu.net` 加进绕过列表，reqwest 的 from_system 通配符解析仍把请求送进代理，代理对 git smart-protocol 的 /info/refs 返回 404（curl/raw TCP 直接连接看到的是 308）→ http→https 升级失效、探测结果错误。修复：探测客户端显式 `.no_proxy()`（Git 地址是用户显式配置的直连目标）。
- **取消拉取（2026-09-16 修复）**：`cancel_rag_git_pick` → `signal_abort_canonical`——先 canonicalize 再查 `PICK_ABORTS`，找不到再回退原始 URL key（修复：pick 把令牌注册在 canonicalize 后的 URL 上，前端传原始 URL（http→https 升级/尾斜杠场景）查不到 → 取消静默无效）。令牌为 `{notify, cancelled: AtomicBool}`——signal 置 flag + notify_waiters；clone 的 select 循环「注册 waker → 复查 flag → await」，关闭注册与 select 之间丢信号的竞态。
- **命令**：`pick_rag_git_repo(url,branch,username,password,depth)`（clone→scan→账密入本地凭证文件）+ `cancel_rag_git_pick`；`list_rag_git_sources` 已随注册表删除；上传时 git 源路径在 `upload_rag_doc` 内 map+persist。
- **拉取进度（2026-09-16，含实证调优）**：`rag://git-clone-progress` 事件——gix `NestedProgress` 自实现（`CloneProgress`）累计**字节**（unit 按 hash 判别：`progress::bytes()` 合成参照比对——独立 probe 实证 clone 全链路 unit 动态生成但 hash 等价、判别有效；objects/refs 等 unit 不计入）。**字节更新路径**（probe 实证）：pack 读取走 `progress::Read` wrapper 每次网络 read 调 `inc_by` → `ThroughputOnDrop` 透传到 `read pack` 子任务（`add_child` 共享 Arc 计数器）——小仓库每秒数千次 inc_by、连续分布，120ms 采样足够顺滑；`total`（pack size）在流式接收时**始终未知**（`init(None, bytes())`），故 UI 恒为 indeterminate 流动条 + 已接收字节/速度（除非未来切 `write_to_directory_eagerly` 拿到 pack_size）。采样任务 120ms 间隔 + 启动即发首帧；clone 结束后**补发终帧**（真实最终字节数）再退出，防止 UI 冻结在上一采样。仅 pick 流程发射（`clone_to_temp` 加 `Option<&AppHandle>`，refresh 路径 None）。前端 RagPage 监听，进度条渲染在下方扫描占位卡片的 spinner/「正在拉取仓库…」文字之下（`maxWidth:360`），取消/完成即随占位消失。
- **冒烟**：真实公共仓库（octocat/Hello-World，depth=1，branch=master）clone+checkout+HEAD sha 测试通过（临时测试跑完已删）。

#### 3.17.4 更新链路（P4）

- `service.rs` 新增 `refresh_git_repo`（60s TTL 去重缓存 `GIT_REFRESH_CACHE`，`git::refresh_registered` = 本地凭证文件→refresh_persistent，无注册表读取）+ `refresh_git_repo_best_effort` + `collect_git_repo_urls`。
- **三个接入点**：①`check_rag_update`（单条检查，best-effort 刷新后 classify）；②`preview_batch_update`（手动按钮+自动 tick 均刷新，保证 preview 的 md5 判定反映远端）；③`run_batch_update`（doc 循环前逐 repo 刷新，去重缓存使 preview+run 双刷新只 clone 一次）。
- 凭证缺失的认证仓库：自动更新/批量刷新失败 → warn 日志跳过（不弹框打断后台）；单条检查同样降级用现有 clone 分类。
- **引用计数清理**：`delete_doc` 后若被删文档 kind=git 且 `count_git_docs_for_repo`==0 → `remove_persistent` + 删本地凭证。
- `open_file_location` git 分支无需改动——git 文档 original_path 已指向持久 clone 内文件，reveal 天然工作。

#### 3.17.5 前端（P5，已获用户确认的 UI）

- `RagPage.tsx` UploadDialog：数据源分段切换（文件/文件夹/Git）+ Git 表单（地址常显；branch/depth/账密折叠在「更多参数」后，认证失败自动展开 + 内联错误）；`gitSourceMeta` 记录 url/branch 供 confirm 构建 DocSource。**重新点「拉取并扫描」时清空上一轮的扫描结果/勾选/仓库元信息**（防旧仓库文件残留在确认列表里误导导入，2026-09-16）。
- 新组件 `components/ui/RagDocTree.tsx`：forwardRef + useImperativeHandle（expandAll/collapseAll）+ 类型徽章（git/文件夹；内置文件/tool 不显示）+ 目录树折叠。
- 工具栏：平铺/树形切换按钮 + 一键展开/收起（带文案的 hub-btn）。
- **mock 已全部移除**（P4 ⑤）：`MOCK_DATA_SOURCE`/`MOCK_GIT_SOURCES`/`MOCK_SOURCE_DOCS`/`MOCK_GIT_SCAN`/`mkMockDoc` 及合并分支全删，全部走真实后端。
- i18n 四语言 ~35 键（git*/dataSource*/tree* 等）。

#### 3.17.6 边界 / 已知限制

- 仅支持 HTTPS（SSH 不支持，后续增强）；树形视图一次渲染全量（量大后虚拟滚动待办）。
- `rag://git-sync-progress` 进度事件未实现（clone 有 120s timeout + spinner；低优先级）。
- 多音字/拼音等 FTS 语义不变；树形纯 GUI 展示，不影响后端分页/搜索契约。
- 私有仓库真机冒烟（GitHub PAT / GitLab）留给用户手动验证。

#### 3.17.7 五轮全量复核（2026-09-09）

> 对已完成功能做 5 轮复核（后端逻辑 / 更新链路 / 前端链路 / 并发竞态 / 回归边界），发现 9 个问题，全部修复。

- **R1 后端 git.rs 逻辑**：①`validate_url` 只拒「带密码」的内嵌凭证，`https://user@host` 型漏放（注入凭证后会成畸形 URL）→ 收紧为拒绝 authority 中任何 `@`；②注释被终端脱敏工具损坏成 `******`（写码时误存）→ 修复；③`is_auth_error` 裸 `"401"/"403"` 子串匹配会误伤 URL 含数字的错误文本（如 repo 名 `issue-1401`）→ 改为短语匹配（`http status 401/403`，对应 gix-transport reqwest 后端 `Received HTTP status NNN` 文本）+ 补 GitHub/GitLab 私有仓库匿名 404 的 `repository not found`。
- **R2 更新链路**：④`update_doc_from_file` 手动换源更新保留了旧 `source`（git 文档手动更新后仍归 git 节点 + 引用计数误算）→ 改置 `None`，读时从新 `original_path` 父目录重新归类；⑤`classify_source` 的 tool 兜底 label 旧文案「工具创建」→「MCP 工具」（与写侧一致）。
- **R3 前端链路**：⑥Git 导入的「强制 copy 语义」只有 UI 注释、后端未强制（隐藏的方式切换 state 默认 symlink 会透传）→ `upload_rag_doc` 命令层对 `kind=git` 强制 `method="copy"`；⑦关弹框不清 git 密码/表单错误 → 抽 `closeUploadDialog`（凭证已入本地凭证文件，不留前端 state；切 tab 不清避免重输）。
- **R4 并发竞态**：⑧`clone_to_temp` 每次清空**整个临时根**、无锁 → 并发 pick（Enter 双击等）互相摧毁 clone → 加全局 `PICK_LOCK` 串行化（refresh 不受影响，自有锁）；⑨`refresh_persistent` swap 中段失败（`.new→main` rename 失败）会让 main 缺失、`.old` 有旧数据且运行时无自愈 → 失败时回滚 `.old→main` + 清 `.new`；⑩单条检查与批量刷新并发时竞态操作 `{hash}.new`（TTL 缓存只记已完成，挡不住进行中）→ per-repo `GIT_REFRESH_LOCKS` 进行中锁 + 锁内 TTL 复查。
- **R5 回归边界**：i18n 260 个 `pages.rag.*` 键四语言全齐（脚本核对）；后端 serde 字段 ↔ 前端 types 逐一比对全匹配（DocSource/GitSource/RagGitSource/RagFolderScan/RagScanFile）；tauriClient 一处过时注释（"credential dialog" → 内联报错）修正；锁序复核：PICK→SOURCES 无反向、META→runtime 恒定、refresh 锁为叶子。
- **回归验证**：`cargo check` 0 错 0 警；`cargo test --lib` 34 passed；`npm run build` ✓；`npx tsc --noEmit` 24 = 基线。

#### 3.17.8 数据源级同步：新增/删除文件随更新检查同步（2026-09-10）

> 用户需求：文件夹/Git 数据源做更新检查时，除了已变更文档的 md5 重索引，**新添加的文件要自动导入、源文件已删除的文档要自动移除**（尤其是文件夹选择）。

- **核心**：`service.rs` 新增 `plan_source_sync`（纯检测）+ `run_source_sync`（执行）。
  - **检测范围**：①文件夹源——`source.kind=="folder"` 的去重 root（存在才参与，root 整体丢失 → 跳过删除判定走既有 lost 语义）；②Git 源——refresh 后的**持久 clone** 目录（clone 缺失/refresh 失败 → 跳过）。两者都用 `scan_folder_public(root, recursive=true)`。
  - **新增判定**：扫描出的文件不被**任何**文档的 original_path 引用（全局引用集，防跨源重复导入）→ `SourceSyncAdd{path, source, method}`；文件夹新增走 `symlink`（UI 默认，文件原地）、Git 新增走 `copy`（与强制 copy 语义一致），tags 为空。
  - **删除判定（双重条件防误删）**：文档 original_path 在 root 前缀下 **且** 不在新扫描集 **且** 磁盘上确实不存在——扫描有扩展/ignore 过滤器，「仅仅掉出扫描候选集」的文件不删其文档（防过滤器漂移误删）。扫描 `truncated`（撞 cap）的源整体跳过增删检测（截断结果不作删除依据）。
  - **前缀匹配**：`path_under_root` 带 `/` 分隔符比较，防 `/a/b` 误配 `/a/bc`。
- **接入点**：①`preview_batch_update`（refresh 后 plan，被同步删除的文档计 `removed` 而非 `lost`，不再双报）；②`run_batch_update`（refresh → plan → 有变化则发 `phase="sync"` 事件 → 执行 → **重收 docs**（增删改变集合与 total）→ md5 pass）；③自动 tick 门槛扩为 `to_update==0 && added==0 && removed==0` 才空转放行。
- **模型/前端**：`BatchPreview` 加 `added`/`removed`（serde camelCase 自动透传）；确认弹框加两格计数 + 同步提示行；进度弹框加 `phase="sync"` 文案；`BatchProgress` 类型（RagPage 本地别名 + useRagData state）补 `'sync'`；i18n 四语言 4 键（batchScanAdded/batchScanRemoved/batchScanSyncHint/batchSyncingSources）。
- **语义注记**：文件夹同步递归扫描（原导入可能是扁平模式——新增检测按递归算，用户「尤其是文件夹」的诉求优先）；legacy 文档（source=None 但 original_path 在已知 root 下）同样参与删除判定（其原始文件确实没了）；被同步删除的 git 文档走 `delete_doc` → 引用归零自动清 clone/本地凭证。
- **验证**：cargo check 0 错 0 警；cargo test --lib 34 passed；npm build ✓；tsc 24=基线。
- **⚠️ 开关门一致性修复（2026-09-18）**：`sourceSyncAddEnabled`（「自动同步新增文件」）此前**只在 `run_batch_update` 执行阶段生效，`preview_batch_update` 未应用**——两个 bug：①开关关闭时确认弹框仍显示「N 个新文件待导入」（实际不导入，数字误导）；②自动 tick 的空转判定 `added==0` 永不成立（只要源目录有新文件），每个 tick 都唤醒 UI 跑一遍空扫描。修复：preview 阶段应用同一开关（关闭时 `sync_added.clear()`，删除侧不受开关影响照常计数）。cargo check ✓；cargo test --lib 42 passed。

#### 3.17.9 Git 数据源失效提示（账密修改/地址变更，2026-09-11）

> 用户问题：git 仓库可能因外部原因拉取失败（地址变更、账密被改/吊销），更新检查此前静默降级（只记日志，用户无感知地基于旧克隆判定 md5）。如何提示？

- **结构化错误**：`refresh_git_repo` 失败改为返回 `GitSourceError{url, branch, auth, message}`（models/rag.rs）——`auth=true`（GIT_AUTH_REQUIRED 前缀映射，gix-transport reqwest 后端 401 文本）→ 修复方式是重新输入账密；`auth=false`（not found / 网络）→ 地址可能已迁移。message 已剥前缀（不泄露内部错误链）。
- **上报通道（四处）**：
  - **批量预览**：`BatchPreview.gitErrors: Vec<GitSourceError>`（并发刷新收集，按仓库去重），确认弹框红色警示块逐仓库列出 + 修复指引（i18n `batchGitAuthError`/`batchGitOtherError`）。
  - **单文档检查**：`RagUpdateCheck.gitError: Option<GitSourceError>`，UpdateDialog 顶部琥珀色警示（i18n `updateGitAuthError`/`updateGitOtherError`）——明确告知「本次检查基于旧克隆」。
  - **进度弹框状态图标**（2026-09-11 补充，用户需求）：`BatchUpdateDialog` 右上角状态按钮——查询前中性灰（未知），点击调 `get_git_source_errors`（读 `GIT_REFRESH_ERRORS` 缓存，**无网络 I/O**）后：无失败变绿色对勾 + 「所有 Git 数据源状态正常」，有失败变琥珀色 ⚠ + 展开错误面板（逐仓库列 url@branch + auth 分支文案，maxHeight 160 滚动）。`refresh_git_repo` 成功时清该仓库的失败记录（恢复即对勾）。
  - **自动定时 tick**：保持静默（只记日志）——后台任务不弹框打断用户（既有设计原则）；用户通过批量按钮、进度弹框图标或单条检查会看到失败态。
- **修复路径**：导入弹框重新输入地址 + 新账密 → 拉取成功即 `store_credential_for` 覆盖本地凭证文件（后续 refresh 自动用新凭证）。地址变更则输入新 URL 后同样走该流程（新 URL = 新 hash = 新数据源，旧仓库文档按引用计数自然清理）。数据源下拉已删除，无快捷重选。
- **失败时语义**：md5 判定基于旧克隆继续工作（不会因拉取失败中断批量/自动流程），仅提示可能过期。

#### 3.17.10 Git 数据源支持 http://（自建 GitLab 场景，2026-09-11）

> 用户场景：自建 GitLab 常走 http://（内网无 TLS），此前 `validate_url` 仅放行 https。

- `validate_url`：`http://` 与 `https://` 均放行（SSH 仍不支持）；内嵌凭证拒绝规则对两 scheme 一致（authority 含任何 `@` 即拒，path 含 `@` 不误杀）。
- `build_clone_url`：凭证注入时**保留原 scheme**（此前硬编码 `https://` 前缀剥离，http URL 会拼出错误 scheme）。
- 单测：`rag::git::url_tests` 3 个（两 scheme 放行/凭证拒绝/凭证注入 scheme 保留 + percent-encoding），`cargo test --lib` 37 passed。
- **http 明文 basic-auth 放行（2026-09-14 补充）**：gix-transport 默认拒绝在 http:// 上发凭证（`Will not send credentials in clear text over http`，`add_basic_auth_if_present` 的 `http-client-insecure-credentials` 守卫）——自建 GitLab/Gitea 走 http + 账密必失败。Cargo.toml 直接依赖 `gix-transport = { version = "0.48", features = ["http-client-insecure-credentials"] }`（与 gix 同版本 feature unification 生效，`cargo tree` 已验证），放行明文凭证。风险注记：仅用户显式配置 http 数据源时才可能走此路径；https 不受影响。
- **http→https 重定向凭证剥离修复（2026-09-16 补充，用户真机 git.haidaifu.net）**：该服务器对 http 返回 308→https。gix/reqwest 在**跨 scheme 重定向时剥离 Authorization 头**（http 请求带的凭证到 https 跟随请求时被丢），gix 重试仍从原 http URL 发起 → 死循环 → 报 `Didn't find smart protocol header`/`InvalidCredentials`（本地 git CLI 能用是因为 curl 会对最终 URL 重新认证）。修复：新增 `canonicalize_url`（reqwest 探测 `/info/refs` 跟随重定向取最终 URL，**先升级 scheme 再 hash + clone**，10 分钟 per-origin 缓存，探测失败非致命回退原 URL），接入 `clone_to_temp` / `refresh_persistent` / `refresh_registered` 三入口——canonicalize 在 hash 之前，保证 http/https 两种写法落到同一存储身份。单测 `canonicalize_upgrades_http_redirect`（真实网络：https 直连不变、http 升级为 https、ssh 原样）。`cargo test --lib` **42 passed**。

#### 3.17.11 匿名拉取认证仓库无报错修复（2026-09-11）

> 用户反馈：无账密访问需认证的 http 仓库，拉取失败但前端**没有**内联认证提示（只显示普通失败，不展开账密区）。

- **根因**（读 gix-protocol 0.51 handshake 源码确认）：gix 收到 401 后**不直接抛 401**，而是走 credential-helper 重试流程——helper 返回 None（我们没有配 helper，凭证来自表单）→ 抛 `EmptyCredentials`，错误链文本是 **"No credentials were returned at all as if the credential helper isn't functioning unknowingly"**，不含任何 "401"/"unauthorized" 字样 → `is_auth_error` 短语全部不命中 → 误判为普通失败。
- **修复**：`is_auth_error` 补 5 条 gix 凭证流程短语（`no credentials were returned` / `credential helper isn't functioning` / `credentials provided` + `were not accepted by the remote`（InvalidCredentials，凭证错误重试后仍 401）/ `failed to obtain credentials`）。
- **过期警示清理**：`GIT_REFRESH_ERRORS` 记录最近一次刷新失败；`refresh_git_repo` 成功即清该仓库记录；`git_pick_inner` 成功拉取（clone + 账密入本地凭证文件）后调 `service::clear_git_source_error_for(url)` 立即清记录，避免进度弹框图标在下次成功刷新前一直显示过期警示。
- 新增单测 `is_auth_error_matches_gix_credential_flow`（EmptyCredentials/InvalidCredentials/FailedToObtain 三文本命中 + 数字 URL 不误报）；`cargo test --lib` **38 passed**。

#### 3.17.12 验证

- `ORT_SKIP_DOWNLOAD=1 cargo check` 0 错 0 警；`cargo test --lib` 34 passed；`npm run build` ✓；`npx tsc --noEmit` 24 = 基线。
- 版本：`1.0.35001 → 1.0.35003`（tauri.conf.json / Cargo.toml / 根 package.json / frontend package.json + Cargo.lock）；changelog **与 1.0.35003 合并为单文件 `doc/upgrade/1.0.35003.md`**（原 `1.0.35002.md` 已删；该文件后续随 2026-09-24 基线同步再并入 `doc/upgrade/1.0.39001.md`，`1.0.35003.md` 已删）。

#### 3.17.13 Git 数据源支持 SSH 连接（2026-09-13）

> 需求：RAG Git 数据源在 http/https 之外支持 SSH 远程。gix 0.73 原生支持 `Scheme::Ssh`（`gix-transport::blocking_io::ssh`，无需新增 Cargo feature），实现方式是 **spawn 系统 `ssh` 程序**——认证完全走本机 SSH 体系（key/ssh-agent/`~/.ssh/config`/known_hosts），现有账密表单 + 本地凭证文件对 SSH 不适用（key 路径）。

**后端（`src-tauri/src/rag/git.rs`）**

- `validate_url`：放行 `ssh://[user@]host[:port]/path.git` 与 SCP 风格 `[user@]host:path.git`（新增 `is_scp_style` helper：`:` 在 `/` 前且 `@` 在 `:` 前；**显式 user@ 必须**，否则 `host:path`/`C:\...` 本地路径有歧义被拒）。凭证拒绝规则分流：http(s) 任何 userinfo 都拒（凭证表单唯一路径）；SSH 的 `user@host` 是标准登录名**放行**，仅拒 `user:pass@`（ssh URL 不支持密码注入）。
- `build_clone_url`：SSH 分支原样返回（无凭证注入）——即使表单误输账密也不进 URL，认证走系统 ssh key/agent。
- `is_auth_error`：补 SSH 失败短语（`permission denied (publickey/password/keyboard-interactive`、`host key verification failed`、`could not read from remote repository`），SSH 认证失败同样映射 `GIT_AUTH_REQUIRED` 前端可感知。
- **Pick 取消（2026-09-16）**：`PICK_ABORTS` per-canonical-URL notify 令牌表 + `register/signal/unregister_abort`；`clone_to_temp` 用 `tokio::select!` 抢占 clone 等待（阻塞 gix 线程无法中断 → 弃等 + 删临时目录，结果丢弃），PICK_LOCK 排队中也可短路；取消错误哨兵 `PICK_CANCELLED`（前端静默不清账密）。新命令 `cancel_rag_git_pick` + 前端 `cancelRagGitPick`（`/rag/cancel-git-pick` 路由）+ 拉取中「取消」按钮（i18n `gitCancelFetch` ×4）。
- **SSH stderr 过滤 wrapper（关键修复）**：gix 的 `supervise_stderr` 逐行解析 ssh stderr，任何含 `Connection to ` 的行被判为连接错误（`line_to_err` 启发式）——但 `nc -v`（`~/.ssh/config` ProxyCommand 常见写法，如 GitHub 走 443 代理）成功时输出 `Connection to ssh.github.com port 443 [tcp/https] succeeded!`，**每次握手必死**（scratch crate 独立复现，错误链 caused by 就是该行）。修复：`ensure_ssh_stderr_wrapper()` 写 POSIX sh wrapper 到 `$TMPDIR/mcphub-rag-git-ssh/ssh`（fd-swap：`{ ssh "$@" 2>&1 1>&3 | sed -l/-u -e '/Connection to .* succeeded/d' >&2; } 3>&1`，Darwin 用 BSD sed `-l`、其余 GNU `-u` 行缓冲保错误即时分类），经 `gix::open::Options::config_overrides(["core.sshCommand=..."])` 注入 clone——`remote.connect()` 会从 repo config 读 `core.sshCommand`（已读 gix 0.73 `ssh_connect_options` 源码确认链路）。**文件名必须叫 `ssh`**：gix 按 basename 推导 ProgramKind，非 ssh 名降级 Simple kind（不能传端口、丢错误分类）。Windows 跳过（无 POSIX shell；`nc -v` ProxyCommand 非 Windows 模式）。
- 单测 +2：`validate_accepts_ssh_forms`（两写法/端口/SCP 判定/本地路径拒绝）、`validate_rejects_ssh_embedded_password`；`build_clone_url_preserves_scheme` 补 2 断言（SSH 带表单凭证仍原样返回）；`is_auth_error` 补 3 个 SSH 文本断言。
- 真机冒烟（octocat/Hello-World，本机 ssh key + ProxyCommand nc -v 环境）：`git@host:path` 与 `ssh://` 两写法 clone 均成功（临时测试跑完已删）。

**前端（`frontend/src/pages/RagPage.tsx` + locales 四语言）**

- 新增 `isSshGitUrl` helper（与后端同口径）；Git 表单：SSH 地址时**隐藏账密输入行**（显示 `gitSshHint` 提示：认证走本机 key/agent，支持两种写法）、底部提示行同步切换、placeholder 更新为含 ssh 写法。
- `handlePickGit`：`GIT_AUTH_REQUIRED` 且 URL 为 SSH 时**不展开账密区**（无意义），改引导文案 `gitSshAuthFailed`（检查 SSH key / ssh-agent / known_hosts / `ssh -T git@host` 验证）。
- locales：`gitSshHint`/`gitSshAuthFailed` 2 新键 ×4 + `gitRepoUrl` placeholder 更新 ×4（rag keys 289→291）。

**SSH 密码认证支持（2026-09-14 补充，用户真机反馈：git.haidaifu.net 为密码认证，无 TTY 无法输密码）**

- `ensure_ssh_stderr_wrapper` 升级为 `ensure_ssh_wrapper(password)`：每 clone 一个**唯一临时目录**（并发 clone 账密不同不互踩，0700），内含
  1. stderr 过滤 wrapper（原 `nc -v` 修复保留）；
  2. `StrictHostKeyChecking=accept-new`（spawn 的 ssh 无 TTY，交互确认 host key 必挂，改为自动接受新主机键——已在 wrapper 内注释说明）；
  3. 密码非空时另装 **SSH_ASKPASS helper**（`askpass.sh`，**0700 必须可执行**——首版 0600 导致 ssh `exec` 失败输出 "Permission denied" 被 gix 误判 InvalidCredentials，真机踩坑）：**无条件应答所有 prompt**（服务器 kbdint 提示词可能本地化如「密码:」，`*assword*` 文本守卫会静默漏答；本受控调用中交互 prompt 只可能是密码类——host key 已 accept-new、用户名来自 URL），密码经单引号转义嵌入（shell probe 验证 round-trip），`SSH_ASKPASS_REQUIRE=force` + `DISPLAY=dummy:0`（兼容旧 OpenSSH）。clone 结束 Drop guard 删整个目录（密码不落盘残留）。
  4. **诊断日志**：wrapper 把 ssh stderr 全量 tee 到 `${TMPDIR:-/tmp}/mcphub-ssh-debug.log`，askpass 把收到的 prompt 文本（**不含密码**）追加到同一日志——密码认证连不上时看这个文件即可定位（服务器拒绝原因 / askpass 是否被调用）。
  5. **setsid TTY 脱离 + 无密码 BatchMode（2026-09-15 补充，真机反馈：无账密时页面永远「正在拉取中」+ 控制台出现 `git@host's password:` 交互提示）**：两层修复——①wrapper 用 `setsid`（存在时）把 ssh 脱离控制终端，`/dev/tty` 打不开 → ssh 回落 askpass；②**不填密码时无 askpass 可装**，ssh 仍会 open `/dev/tty` 要密码（挂起 + 控制台提示），故无密码时给 ssh 追加 `-o BatchMode=yes` 立即失败（`Permission denied` → GIT_AUTH_REQUIRED → 前端弹账密表单）；有密码时不加 BatchMode（会禁用 askpass 查询），统一 `-o NumberOfPasswordPrompts=1` 错密码快速失败。真机端到端验证（git.haidaifu.net）：无密码 BatchMode 路径 0.7s 快速失败零 TTY 泄露；错密码 askpass 路径 3.3s 快速失败零 TTY 泄露。
- `build_clone_url` SSH 分支：表单用户名是 ssh **登录名权威来源**——URL 无 userinfo 时注入，URL 带惯例 `git@` 时**覆盖**（真机反馈：git.haidaifu.net 密码认证，登录账户是用户自己的账号而非 `git`，连 `git` + 用户密码必被拒，gix 报 `InvalidCredentials: Credentials provided ... were not accepted by the remote`）；密钥登录（GitHub）表单留空用户名即不受影响。ssh:// 带端口 userinfo swap 保留端口；SCP 风格同规则。
- `clone_blocking`/`clone_async` 增加 `ssh_password` 参数（clone_to_temp / refresh_persistent 透传表单密码）；refresh_registered 走本地凭证文件自动适用（SSH 密码仓库的更新检查免重输）。
- Windows：wrapper 整体跳过（无 POSIX shell；旧代码在 Windows 返回了**未写盘的路径**是 bug，已改为返回 None）。Windows 下 SSH 仅支持 key 认证（无 askpass）。
- 前端：SSH 地址**同样显示账密输入**（密码认证服务器直接填）；`handlePickGit` SSH 认证失败也展开账密区；`gitSshHint`/`gitSshAuthFailed` 文案更新为「key 优先 + 密码可用」双语义（×4 语言）。
- 单测：`ssh_wrapper_renders_filter_and_askpass`（无密码无 askpass / 有密码 askpass 0600 + 单引号转义 `p@ss'w` round-trip + prompt 守卫）；GitHub SSH 冒烟回归（key 路径 + wrapper）通过（临时测试已删）。`cargo test --lib` **41 passed**。

**http 路径 TTY 提示修复（2026-09-15 补充，用户反馈：http 导入时控制台出现密码输入提示）**：gix 的 credential cascade 走空（http 401 + 无/错凭证）后调 `gix-prompt` **直接 open `/dev/tty`** 向用户要账密——提示打到 app 控制台并阻塞。修复：clone 的 `config_overrides` 加 `gitoxide.credentials.terminalPrompt=false`（gix 的 `Mode::Disable` 只禁 TTY 交互、askpass 程序不受影响；http 与 SSH 双路径生效）→ cascade 失败即快速返回 → `GIT_AUTH_REQUIRED` → 前端表单提示账密。端到端验证（GitHub 对不存在私有 repo 返回 401）：1.4s 快速失败、分类为 auth error、零 TTY 交互（临时测试已删）。

**验证**：`cargo test --lib` **41 passed**；`cargo check` 0 错 0 警；`npm run build` ✓；`npx tsc --noEmit` 24 = 基线。版本 `1.0.35002 → 1.0.35003`（四源 + Cargo.lock）；changelog `doc/upgrade/1.0.35003.md`（后续并入 `doc/upgrade/1.0.39001.md`，原文件已删）。

#### 3.17.15 文件夹级联勾选（主页树形 + 导入弹框，2026-09-17）

> 用户需求：RAG 主页文件列表与导入的文件列表中，**文件夹可以被勾选，勾选则级联选中其下所有文件（含子文件夹内的文件）**。文件夹本身无独立持久状态——勾选是纯级联语义（子树文件全选/全不选）。

- **主页树形列表**（`frontend/src/components/ui/RagDocTree.tsx`）：
  - props 新增 `selectedIds?: Set<string>`（与平铺列表共享同一勾选状态）+ `onToggleDocs?: (docs: RagDocInfo[]) => void`。
  - 新增模块级 helper `collectDocs(node)`：深度优先收集目录子树内全部文档（与既有 `countDocs` 同遍历口径）；新增 `DirCheckbox` 组件（`hub-checkbox`，`ref` 设 `el.indeterminate` 支持半选态，`onClick` stopPropagation 避免触发整行折叠/展开）。
  - **目录行（DirNode）** 头部最左侧渲染 checkbox：checked = 子树文档全选；indeterminate = 部分选中；onChange → `onToggleDocs(collectDocs(node))`。数据源（kind）节点行同样加 checkbox（`src.docs` 全量）。
  - DirNode props 透传 `selectedIds`/`onToggleDocs`（两处递归调用点）。
- **导入弹框**（`frontend/src/pages/RagPage.tsx`）：
  - 新增 `ScanDirCheckbox` 组件（同 DirCheckbox 语义）。
  - **树形目录头（ScanDirNode）**：最左侧 checkbox → `onToggleGroup(node.files)`（`node.files` 本就是子树全部文件）；**平铺分组头**：checkbox → `onToggleGroup(g.files)`。勾选态：`sel.length === files.length` 全选 / 部分选 → indeterminate。
  - `toggleGroupSelected` 语义天然匹配 checkbox（全选中→清空，否则全选）。
- **RagPage 接线**：新增 `toggleSelectDocs(docs)`（子树全选→全部取消，否则全部选中；updater 内 `new Set(prev)` 复制，遵守 Object.is 突变禁令）；`<RagDocTree>` 调用处传 `selectedIds` + `onToggleDocs`。
- **验证**：`npx tsc --noEmit` 24 = 基线（零新增）；`npm run build` 通过。Rust 无改动。
- **导入弹框配置区对齐优化（9-20）**：「数据源」「导入方式」两行改为 2 列 grid（`max-content 1fr`，label 左列对齐、切换器右列），分段切换器底色统一 `hub-surface`（与卡片内层对比清晰）；网格在导入方式行后闭合，选择入口/Git 表单/开关组仍为卡片纵向子项。
- **导入弹框布局重设计（9-18 定稿）**：全新「导入配置分区」卡片（`hub-bg-2` + 边框圆角），内部固定顺序 = 数据源切换 → 导入方式（file/folder）→ 选择入口 + 递归（file/folder）→ **Git 表单**（git）→ 自动同步开关组（新增[folder/git] + 删除[file/folder/git] 并排一行）；格式说明文案（uploadHint）+ OCR 预检提示放卡片下方，其后扫描占位/扫描树/标签/footer。⚠️ **事故与恢复**：重做过程中用 `join('\n')` 整文件行手术切错块边界，随后误执行 `git checkout -- RagPage.tsx` 把 9 天未提交改动还原到 HEAD（9-09）；**唯一完整恢复源 = Vite 构建 sourcemap 的 `sourcesContent`**（`dist/assets/RagPage-*.js.map` 保存构建时原始 TSX 源码，含 19:13 前全部改动），提取即完整还原（tsc 25 基线 + build ✓）。教训：①工作树有大量未提交改动时严禁 checkout/stash 丢弃；②vite sourcemap 是应急源码恢复源；③大范围 JSX 重组应小步替换 + 每步 tsc 验证。
- **导入弹框底部「全选/清除」按钮移除（同日）**：footer 只保留「已选择 N / M 个文件」计数 + 取消 + 导入；组头/目录头的勾选框已覆盖批量选择语义（文件夹级联），顶部全选按钮冗余。`onSetAll` prop 保留（组头逻辑仍用）。
- **单文件数据源纳入删除同步（同日，用户需求）**：`plan_source_sync` 末尾新增 file 源检测——`source.kind=="file"`（含 legacy 无 source 的文档）且 `original_path` 在磁盘上不存在 → 加入 removed（受「自动同步删除文件」开关控制；关闭时保留 + lostOriginal 徽章）。文件源无扫描/过滤环节，无需 folder/git 的双条件（扫描集 + 磁盘双查），仅「文件不存在」即判定；`!removed.iter().any(id)` 防与 folder root 重叠路径重复入列。tool 源（无 original_path）天然跳过。
- **「自动同步删除文件」开关（同日，用户需求）**：删除同步从「始终运行」改为受控开关（默认开）——①RagSettings 加 `source_sync_remove_enabled`（serde default true，配置键 `sourceSyncRemoveEnabled`，deep-merge 持久化）；②`preview_batch_update` 与 `run_batch_update` 在开关关闭时 `sync_removed.clear()`（不删文档、确认弹框不显示「移除 N」）；③关闭时列表行沿用既有 `lostOriginal` ⚠️「原始丢失」徽章告警（判定独立于删除：original_path 不存在即亮，symlink/copy/git 通用）；④导入弹框设置区新增「自动同步删除文件」Switch（file/folder/git 三种数据源均显示，与「自动同步新增文件」并排一行；新增开关仅 folder/git 显示——file 数据源无目录可扫），inline fallback 文案（跟随既有开关模式）。开关组位置在**数据源分段切换行正下方**（格式提示文案之前）。注意 run/preview 两处 `sync_removed` 需 `mut`（E0596 曾踩）。
- **批量进度弹框 Git 状态图标默认态修正（同日）**：右上角 Git 数据源状态按钮未查询时渲染 AlertTriangle（灰），用户误以为有错误——未查询态改为中性 CircleHelp；点击查询后无失败=绿勾、有失败=琥珀警告（语义不变，见 §3.17.9）。
- **手动更新与数据源同步竞态导致重复文档修复（9-20）**：用户手动上传更新（update_doc_from_file 记录新 original_path）的同一分钟，auto tick 的 plan_source_sync 用**更早的 meta 快照**判定该文件「无文档引用」→ run_source_sync 导入了第二个同路径文档（树里出现两个同名文件，一 v1 一 v2，DB 证实两条 meta 同 original_path）。修复：`run_source_sync` 每次导入前在 **META_LOCK 内**重扫全部 .meta，若已有文档的 original_path == 待导入路径则跳过（日志 "raced with manual update"）——锁序 META → runtime 与既有规则一致；plan 阶段的 referenced 检查保留（快照过滤大集合），执行期重查兜底竞态。
- **自动更新空转时「原始丢失」徽章不出现修复（9-20）**：根因——auto tick 的空转分支（to_update==0 && added==0 && removed==0）静默 `continue` 不发任何事件，而 lostOriginal 徽章在列表加载时计算，源文件被删后列表停留旧数据直到手动刷新。修复：空转但 `preview.lost > 0` 时发轻量事件 `rag://docs-invalidated`（payload=lost 数），useRagData 监听后仅 `fetchDocs()`（不弹进度框/不动按钮状态，保持空转不打扰 UI 原则）；有实际工作的 pass 走既有 done 事件重拉，不受影响。
- **批量进度弹框 Git 状态按钮改为「仅失败时显示」（9-20）**：此前未查询时渲染中性 ？（CircleHelp），用户误认为帮助按钮。改为弹框打开即自动查询（读后端 GIT_REFRESH_ERRORS 缓存，无网络 I/O），**仅在确有 Git 刷新失败时渲染琥珀 ⚠ 按钮**，无失败/无 Git 数据源完全不显示图标。查询逻辑仍在点击展开时复用（已加载则跳过）。
- **导入弹框卡片化布局落地（9-20，设计稿 v9 定稿）**：按确认的方案 B 重写 UploadDialog 配置区——①**数据源三选大卡片**（3 列 grid，emoji 图标 📄/📁/🌿 22px + 名称 + 描述居中，选中蓝边+浅蓝底，点卡片即选）；②**「自动同步」卡片**（hub-sect 小标题 +「更新检查时执行」副注）：「新增文件」/「删除文件」两张**可整卡点击的子卡片**（flex:1 等分同行，内含 compact Switch + 加粗标签 + 一行灰色说明，开启蓝边高亮；新增子卡仅 folder/git 渲染）；③选择入口行（选择按钮改 `hub-btn primary` 主色 + 递归开关）；④Git 表单不变；⑤**「导入方式（软链接/拷贝）」切换器 UI 移除**——`handleDataSourceChange` 按数据源自动设 method（git=copy、file/folder=symlink），后端强制 Git copy 语义不变；⑥uploadHint 文案优化（"支持 Markdown、代码、Office 文档、PDF 与图片，导入时自动提取内容并建立检索索引。"）。新 i18n 键 ×4 语言：sourceSyncTitle/Sub/AddShort/AddDesc/RemoveShort/RemoveDesc + uploadHint 更新；数据源卡片描述 dataSourceFile/Folder/GitDesc ×4（对齐设计稿，首版遗漏后补）。⚠️ 该轮曾因整块字符串替换切错边界致 JSX 断裂（tsc 424），通过会话备份（files/RagPage.tsx.bak-20260920）恢复 git 表单块 + 清除旧开关组残留修复。
- **RAG 文件类型图标（9-20）**：新增 `frontend/src/utils/fileIcon.tsx` 统一映射器——按扩展名/精确文件名（Dockerfile/Makefile/README/LICENSE）返回 lucide 图标：md→BookOpen、pdf→BookOpen(红调)、图片→FileImage、表格→FileSpreadsheet、演示→Presentation、压缩→FileArchive、shell→FileTerminal、json/配置→FileJson、代码→FileCode、未知→FileType。接入点：平铺/树形叶子行（renderDocRow）、删除确认弹框、文档详情弹框；树形数据源节点图标（kind）不变。样式统一 `var(--hub-ink-3)`。
- **RAG 标题计数补齐（同日）**：RAG 页标题下方新增「N 篇文档」副标题（`hub-num` + `hub-sub`），与其他页面（提示词/资源/Skill）的标题计数样式一致；计数取 `docPagination.total`；i18n 新键 `pages.rag.docCount` ×4 语言。
- **搜索栏计数移除（同日）**：Servers / Prompts / Resources / Skills / RAG 五个页面搜索栏右侧的「N/M」总数统计移除（信息价值低且挤占工具栏宽度）；RAG 移除的是工具栏计数 span（批量条计数此前已删）。
- **样式修复（同日）**：批量勾选条/树形「展开收起」按钮出现时工具栏宽度不足——容器无 `flex-wrap` 且子项默认收缩，按钮被压缩致「批量添加标签/平铺/树形」等文字换行错乱。修复：工具栏容器加 `flex-wrap`；批量条、计数+视图切换组、展开收起按钮加 `flex-shrink-0`；全局 `.hub-btn` 加 `white-space: nowrap` + `flex-shrink: 0`（按钮文字永不换行）。**后续补充**：`flex-wrap` 导致平铺勾选时批量条整体换行——去掉换行需求不现实（窗口小必换），改为优先收缩可收缩项：文件名搜索卡片加 `min-w-[160px]`、TagSearchSelect 根加 `min-w-[150px] shrink`——宽度不足时先缩搜索区，仅在极窄窗口才换行；批量条不再单独换行。

#### 3.17.16 RAG 树形视图增强：数据源/目录级更新与排除、源别名（2026-09-24）

> 树形视图（RagDocTree）的操作能力扩展。涉及 `rag/service.rs`、`commands/rag.rs`、`RagDocTree.tsx`、`RagPage.tsx`、`tauriClient.ts`、`ragService.ts`、locales ×4。

- **数据源级手动更新**：`SourceUpdateTarget { kind: "git"|"folder"|"file", url, root }`——git=强制拉取远端+范围同步/重索引（TTL 绕过）；folder=扫描源根目录（支持子目录 root：路径前缀圈定范围）；file=逻辑「文件」分组（单文件导入+无 source 存量文档），仅重索引有变更文件（无同步）。命令 `refresh_rag_source` / `preview_rag_source_update`（复用 BATCH_UPDATE_RUNNING CAS + `rag://batch-update-progress` 事件）。前端树形：数据源行（git/folder/file）+ **目录行**（folder=绝对路径 root；git=root=clone 内相对子目录）都有刷新按钮，统一弹源范围确认框（复用 BatchUpdateConfirmDialog）。⚠️ tauriClient 路由 `/rag/source-update` 分支必须加 `segs.length === 2`，否则会吞掉 `/preview` 导致确认框拿不到数字。
- **数据源/目录级排除**：排除注册表（`config_json.rag.excludedPaths`）加**目录条目**（前缀覆盖其下全部文件）——folder=源根/子目录绝对路径；git=持久 clone 目录（新命令 `get_rag_git_clone_dir`）；file 逻辑分组=逐文档按 original_path 切换。目录行/数据源行排除按钮状态按「有 original_path 的文档全部 excluded」判定（doc.excluded 后端前缀匹配计算，含祖先覆盖），子级状态随 refreshDocs 自动同步；取消排除时移除覆盖它的祖先条目。按钮顺序统一【刷新、排除】（数据源行另加【别名】按钮）。
- **数据源别名**：注册表 `config_json.rag.sourceAliases`（identity=folder 根路径 / git canonical URL → 别名），`set_rag_source_alias` 命令（空别名=清除）；读路径在 `doc_info_from_meta` 覆盖 classify_source 的默认标签（3 个列表/分页调用点共享一次注册表读取）。前端 `SourceAliasDialog`（铅笔按钮，预填当前展示名，空值提交=恢复默认名），refreshDocs 后树形数据源节点即时显示别名。
- **验证**：`cargo check` 通过；`cargo test --lib rag::` 22 passed；`tsc` 24=基线；`npm run build` 通过。i18n 新键：refreshFolder/refreshFile/refreshFileGroupScope/excludeFileGroupAdd/Remove/renameSource* ×4 语言。

#### 3.17.15 分片策略修正 + Git 图标 + skill 多选样式 + 刷新按钮（2026-09-22）

> 用户五项需求的落地与一次**方案纠偏**（3.17.15 内分片部分曾按「固定长度窗口 + 超长降级 text」实现，用户明确否定，已按本节最终方案重写）。

**① RAG 导入 Git 图标（用户需求1）**：`components/icons/GitIcon.tsx` 新增——simple-icons 官方 Git logo path（viewBox 24，默认色 `#f05032` 官方橙，`style.color` 可覆盖、其余 style merge）。替换点：RagPage 导入弹框数据源卡片/地址输入行/拉取按钮/扫描统计条/文档列表 Git 徽章（5 处 `GitBranch`→`GitIcon`）+ RagDocTree 树形数据源节点与类型徽章（2 处）；RagPage 移除 `GitBranch` import。

**② skill 页多选样式统一（用户需求2）**：InstallDialog 的「独立搜索框 + checkbox 平铺列表」改为与 RAG TagSearchSelect 同视觉语言的 `AgentMultiSelect`（此前 ExportDialog 已接入）：触发按钮 + portal 下拉（搜索输入/已选 chips 带清除/方框勾选项行）。`AgentMultiSelect` 扩展三个可选 prop 支撑 InstallDialog 特性：`sections`（分节渲染「已安装/未安装」，sticky 节头，查询在节内过滤）、`renderRowMeta`（行内徽章：当前安装方式 + switching 提示）、`renderRowExtra`（行尾控件：symlink/copy 切换器 + 卸载按钮，stopPropagation 不触发行选择）。列表 maxHeight 200→240。删除 InstallDialog 的 `search` state / `filtered` / 旧 `renderRow` / `toggle`。

**③ 五页刷新按钮（用户需求3）**：核查确认 Groups/Prompts/Resources/Skills/RAG 五页头部均已有「刷新」按钮（`hub-btn` + `RefreshCw` + animate-spin，同 Servers 页模式，前轮已落地）；本轮无改动。

**④ 内容提取产物分片路由（用户需求4）**：`extract/mod.rs` 新增 `produces_markdown(filename)`（PDF/Office → true 产出 Markdown；image → false 产出纯 OCR 文本）。`chunker.rs::semantic_strategy` 最前分支：`produces_markdown` → `MarkdownChunkStrategy`；其余照旧（`.md` → markdown、代码见⑤、其他 → text）。即「看产出内容」：md 产物走 mdsplit、其他（OCR 文本）走 text。新增测试 `extracted_products_route_to_markdown_splitter`（report.pdf 的 md 内容按标题切 2 块、report.txt 同内容合 1 块作对照）。

**⑤ 代码分片路由（用户需求5，两次纠偏后的最终方案）**：
- **第一次纠偏**：9-20 曾实现「>256KB 的语义文件按行对齐切 256KB 窗口、窗口内重跑 splitter」；用户明确要求**不使用固定长度、不用超长降级**。已移除 `SEMANTIC_WINDOW_BYTES`/`split_line_windows` 及其测试。
- **第二次纠偏（linthis 整体撤回）**：曾引入 `linthis = "0.28"` 做「支持语言先格式化再走 code-spliter、不支持走 text-spliter」。核实后发现 **linthis 只是聚合器**——每个语言的 formatter 都 shell-out 到外部二进制（rustfmt/prettier/ruff/gofmt…），等于要求用户安装外部工具，违背「内置工具、不外部安装」预期。用户决定**暂时不处理格式化**，已完整撤回：删除 `src/rag/format.rs`、Cargo.toml 的 linthis 依赖、`rag/mod.rs` 的 `pub mod format`、`chunk_document` 内的格式化步骤与 `semantic_strategy` 的 linthis 门控。
- **最终路由**（与项目最早逻辑一致 + 防挂死守护）：`chunk_document` 依次判定——提取产物（`produces_markdown`，见④）→ `.md` → `MarkdownChunkStrategy`；代码扩展（tree-sitter 语法支持，js/ts/py/rs/go/java/c/cpp…）→ `CodeSplitter`；其余 → `TextSplitter`。整文件单一策略一次完成，无窗口、无格式化、无降级。
- **保留的防挂死机制**（非窗口、非降级）：`TokenChunkSizer::size` 快速否决——`len > capacity * MAX_TOKEN_BYTES(64)` 时直接返回超容量哨兵，不经 tokenizer。**该机制正是「压缩 JS 跑一整晚」的根因修复**：临时探针实测（见下），未加否决时 1.8MB 单巨型 AST 节点的 minified JS 分片 **>19 分钟仍不结束**（text-splitter 二分探测反复 tokenize 永远装不下多 MB 候选），真实模型 tokenizer 下即「过夜」量级；加否决后同一文件 1.7s 完成（整文件作为一个超容 chunk 返回）。另一形态（海量小语句，3.2MB/14k AST 节点）两种状态均 ~70s：每 chunk 恒定 ~0.1s 的库内 AST range 扫描开销，耗时与 chunk 数线性相关（chunk_size 128 → 2593 chunks → 259s），tree-sitter 解析本身 <1s 非瓶颈。`minified_js_terminates_quickly`（30k 行 <120s）与 `giant_single_line_js_terminates_quickly` 两个回归测试守护该机制。
- **同类问题全面复查（2026-09-22 第二轮，用户要求「再次检查是否还有类似问题」）**：发现并修复 3 处——①**超长 chunk 安全阀**（`chunker.rs::split_oversized_chunks`）：text-splitter 的「至少保留一个 section」保证会让不可再分的巨型节点（单巨型 AST 节点/无边界行）输出一个远超容量的整块，embed 端 forward 有 max_context 截断不会挂，但该块会**原样存库**，检索命中时把 MB 级文本推进 UI；现于 `chunk_document` 出口把超过 `capacity×64` 字节的 chunk 按字符边界硬切窗口（内容零丢失，正常分片输出不受影响），新增测试 `oversized_unsplittable_chunks_are_hard_split`（含多字节 UTF-8 边界断言）。②**单条 `embed()` 无截断**（`gguf.rs`）：`embed_batch` 走 `forward_sub_batch` 按 max_context 截断，但单条 `embed()`（查询路径）直接构造 `[1, seq]` 张量——超长查询会分配超额 attention 内存（Metal buffer overflow/OOM 风险）；已对齐截断。③**测试桩 tokenizer 退化行为修复**（`chunker.rs` 测试模块 `WhitespaceTokenizer`）：原实现 `split_whitespace().find()` 对无空白文本把任意长候选坍缩成 1 个 token，在快速否决边界制造 fits/非 fits 悬崖，令 text-splitter 二分在悬崖处字符级碎裂——50KB "xxx…" 实测 428s/17,233 个单字节 chunk（真实 tokenizer 每字符出 token、单调平滑无此问题，**生产不受影响**，纯测试桩伪影）；桩改为顺序 8 字符分组 token（对候选长度单调，镜像真实 tokenizer 行为），相关测试判别尺寸同步重调。
- **第三轮横向排查（同日第四轮）**：发现并加固**活动日志载荷无界写入**——`log_service::write_activity`（5 个调用点：Tauri tools 命令 ×2、http_server ×2、mcp_tasks）把工具调用的 `input`/`output`（serde_json 序列化）与 `error_message` 原样写入 `activity_log`，无长度限制——MCP 工具传大文件内容（如 rag_file_create 的 docContent）或返回大结果时整块入库（ActivityPage 详情弹窗也会整块渲染）。收口截断：input/output/error_message 各 64KB（字节 + 字符边界对齐，超出附 `…(truncated)` 标记）。其余排查项：Rust 侧剩余 `.repeat` 全为 gguf 张量广播/测试；extract 层 markdown 构建由库线性完成；前端无 indexOf-in-loop/repeat 平方模式，ViewDialog/分片弹框已有分页，chunk_text 经安全阀 ≤32KB。**回归**：`cargo test --lib` 46 passed / 0 failed。
- **第二轮横向排查（同日第三轮）**：以「无界输入 × 重复扫描 / 无界写入」为模式横扫 rag/、skill_service、fts_service、http_server、app_logger——发现并加固 2 处日志写入无界点：①`app_logger::log_to_db`（**117 个调用点的唯一收口**，前端 `log_event` 也经此）原来对 message 无长度限制，超长消息直接进 `app_log` 表并被 FTS tokenize（写放大）；现收口处截断 4000 字符（字符边界对齐）。②`stdio_transport` stderr drain 逐行 `log_to_db`/`log::info!` 无截断——输出多 KB 行（如 minified JSON dump）的 MCP server 会刷爆控制台与日志表；现单行 2000 字符截断（`stderr_tail` 32KB 滚动缓存语义不变）。其余排查项均线性或有界：正则零使用；http_server 有 `parse_body_limit`；skill frontmatter 解析单遍线性；自动更新定时器有代数守卫；SSE/http/openapi transport 的 log_to_db 均为有界 format! 状态消息；`decode_text` SIMD 线性；提取层受 64MiB 上传硬顶约束。**回归**：`cargo test --lib` 46 passed / 0 failed。
- **审计覆盖面**：三个 splitter 共用同一 `TokenChunkSizer`（快速否决全域生效）；上传读取有 `MAX_UPLOAD_BYTES` 64MiB 硬顶；搜索 snippet 即 chunk_text（阀后 ≤32KB）；`to_lowercase`/find/decode 等 O(n) 操作无平方级路径；`reindex_doc`/批量/自动更新均为逐文档线性。**结论：无其余同类无界开销路径**。
- **cargo 配置**：机器全局 `~/.cargo/config.toml` 把 crates.io 指到 GitHub GIT index，github 不稳时 `cargo` 卡「Updating crates.io index」超时；`src-tauri/.cargo/config.toml` 追加 `[source.crates-io] registry = "sparse+https://index.crates.io/"`（注意**末尾斜杠必需**，否则 "sparse registry url must end in a slash"）本地覆写强制 sparse 协议。

#### 3.17.16 深层多行 AST 生产挂死根治：CodeSplitter AST 预分区（2026-09-22 第三轮，release 实测确认）

> 用户实测反馈：`mermaid.min.js`（3.3MB）RAG 导入在 **release 生产包**依旧挂死，前两轮修复（快速否决 / 安全阀）无效。本轮以真实文件 + 真实模型 tokenizer 复现出真正的根因。

**根因：测试桩 tokenizer 掩盖了 1000 倍的真实成本**

- 前两轮的所有回归测试（`minified_js_terminates_quickly` / `giant_single_line_js_terminates_quickly`）用 `WhitespaceTokenizer` 测试桩——每次 `size()` 探测 ~**µs** 级；生产用 granite 97m GGUF BPE tokenizer——每次 32KB 候选探测 ~**7ms**（快 1000 倍成本）。测试全绿对生产行为**毫无证明力**。
- 真实复现 bench（`cargo test --test bench_tmp --release`，真实模型 + 真实 mermaid.min.js）：全量 tokenize 3.3MB 仅 **1.0s**（tokenizer 本身不慢）；但 unpartitioned `CodeSplitter` 跑了 **2 分 48 秒 + 98.5% CPU 仍未完成**（debug），release 下同样挂死。
- mermaid.min.js 的真实形态是关键：**3587 行、184 行 >1KB、最长单行 324KB**——不是此前测试假设的「单行巨型文件」，而是「**深层多行 AST + 多条超长行**」。text-splitter 生成每个 chunk 时都要做二分探测（`MemoizedChunkSizer` 缓存在**每个 chunk 生成后就清空**，见 `next_chunk` 里 `clear_cache()`），7717 个 chunk × 每 chunk ~20 次探测 × 每次 ms 级真实 tokenize ≈ 数小时。快速否决只挡住了「>32KB 不可 fits 的候选」，挡不住**决定 chunk 边界时必须精确定位的 ≤32KB 候选探测**。

**修复：AST 预分区（非窗口、非降级——分区边界就是语句边界）**

- `chunker.rs::CodeChunkStrategy` 新增 `partition_offsets`：文件 >`CODE_PARTITION_BYTES`(256KB) 时，先用 tree-sitter 把文件解析成语法树，按「最浅可分节点」递归切出原子区间（`push_atoms`，子节点之间的空隙也作为 gap 原子，**区间精确铺满全文、零字节丢失**），贪心装箱成 ≤256KB 的分区；每个分区独立跑 `CodeSplitter`。
- **语义不变**：分区边界 = AST 节点边界（语句/块结束处），与任何 chunk 边界同类；每个 chunk 依然在 AST 节点处切分。这是「分层」而非「定长窗口」（用户已明确否定定长窗口方案）——window 大小由语法树结构决定，不由字节位置决定。
- **代价**：`chunk_overlap` 只在分区内生效（跨分区边界无重叠）——分区边界是语句结束，与普通 chunk 边界同类，语义可接受。
- 解析失败（语法不支持/半途解析错误）回退历史单遍路径（warn 日志），导入永不因此报错。
- **release 实测**（真实模型 + 真实 mermaid.min.js）：分区 256KB → **47.6s** / 7717 chunks；分区 64KB → **37.3s** / 7722 chunks（overlap=0 时 16.5s）；max chunk 2021 bytes（=512 token 正确）；内容零丢失（3,493,058 字符精确对齐）。从「数小时挂死」到 37s。**收益递减**：256→64KB（分区数 4 倍）仅快 1.3 倍——剩余成本主要是 splitter 每分区的固定遍历开销而非探测前缀长度，不再调小（更小分区把语句子树切碎成噪声）。debug 构建同文件 396s（release/debug 约 10 倍差）。

**配套修复：分片阶段 UI 进度反馈**

- `service.rs::reindex_doc` 的 0% 进度 tick（`emit_upload_progress(0, total_chars)`）从 chunking 之后**提前到 chunking 之前**——大文件分片阶段（现在也可能几十秒）进度条不再停留在「Preparing…」无响应，而是立即显示真实总字数。

**「分片中…」5 分钟根因：git 刷新风暴 + TTL 竞速（2026-09-23，日志实证）**

- **现象**：单文档更新时「分片中…」卡 ~5 分钟（总 296s 中嵌入仅 211s）。日志实证：更新窗口内 **4-6 次 `git: refreshed repo`（每次 ~63s 网络重克隆）与分片/嵌入并发**，抢光网络/CPU。
- **根因是定时器风暴**：用户把自动更新间隔设为最小 60s，而每次 git 刷新本身 ~63s（重新克隆 hdf-book 仓库）+ TTL 缓存只有 60s——刷新完 TTL 刚好过期，下一个 tick 又刷新，**无限重克隆循环**（日志 269 次/天，空闲时也在每分钟克隆）。
- **修复①**：`GIT_REFRESH_TTL_SECS` 60 → **600**（10 分钟）。刷新是全量网络重克隆，TTL ≤ 克隆时长必然循环；10 分钟既封顶克隆频率又足够新鲜（md5 对比最多滞后 10 分钟）。
- **修复②**：`updateDoc`（单文档更新）补上 **import-session 括号**（与批量导入相同）——会话期间后端 defer git 刷新 + 自动更新 tick，更新不再被后台克隆拖慢。
- **大文件提示通用化**（用户需求「提示是通用的，不分入口」）：无 fileSize 的入口（单文档更新/模型重载）退化为用 `charsTotal × 1.5` 近似字节判断（只用于提示不用于逻辑），≥1MB 同样显示。
- 回归：`cargo check` ✓；`cargo test --lib` **48 passed**；tsc 24 = 基线；build ✓。

**嵌入阶段 padding 浪费根治：embed_batch 长度分桶（2026-09-23，用户「耗时还是太长」）**

- **生产日志揭示真瓶颈**：mermaid.min.js 实测 `totalMs=522253`，其中 **embedMs=436611（84%）**——分片（前轮已优化到 ~37s）不是大头，**嵌入才是**。6199 chunks ÷ 32 批 = 194 次 forward，每次 ~2.2s。
- **根因**：`forward_sub_batch` 的 forward 成本 = `[batch × 批内最长序列]`（右 padding），一条 2048-token 长尾 chunk 混进 31 条 300-token 短 chunk，全批按 2048 算——语句边界分片的长短方差极大，实测 padding 浪费 ~6 倍 GEMM。
- **修复**（`gguf.rs::embed_batch`）：**长度分桶**——tokenize 后按长度排序，长度相近（新行超过桶首行 12.5%+64 token 即封桶）的行一起 forward，最后按原下标 scatter 回调用方顺序。**零语义变化**（每行 embedding 逐位相同，只有计算顺序不同），padding 浪费 ~6x → ~1.1x，预期嵌入阶段 437s → ~90-120s（Metal/CPU 同样受益）。
- 逐行 embedding 的正确性不依赖批内组成（attention mask 隔离 padding），排序不改变任何输出值。
- 回归：`cargo check` ✓；`cargo test --lib` **48 passed** / 0 failed。

**导入浮层耗时显示 + 大文件提示（2026-09-23，用户需求）**

- **耗时显示**（总耗时 + 单文件耗时）：`useRagData.tsx` 新增 `uploadTiming` state（`startedAt` 批次起点 / `fileStartedAt` 当前文件起点 / `fileSize` 当前文件字节 / `tick` 每秒 +1 驱动重渲染；interval 仅在 overlay 存在时挂载）。`upload()`（批次起点 + 每文件切换时更新 fileStartedAt/fileSize）、`updateDoc()`、`reindexAll()` 三处接入，finally 清空。`RagPage.tsx` 导入浮层在文件计数下新增耗时行：`总耗时 m分s秒 | 单文件 m分s秒`（`formatElapsed` helper，mm:ss 中文习惯 m分s秒）。
- **大文件提示**：`RagPickedFile` 加可选 `size`（`selectedPickedFiles` 从扫描结果 `RagScanFile.size` 透传；单文件 pick 无 size=0 不提示）。浮层单文档进度条下方，`fileSize >= 1MB`（`LARGE_FILE_HINT_BYTES`，与后端 chunker >1MB 源码警告同量级）时显示 spinner + 「大文件导入耗时较长（分片与向量化），请耐心等待 · N MB」——**分片阶段（charsTotal=0）同样显示**，这正是最久的一段。文件级提示（不分大小）在 overlay 中始终可见的耗时行承担。
- **批量更新/自动同步弹框同款耗时**（2026-09-23 追加，用户需求）：`BatchUpdateDialog` 也显示「总耗时 | 单文件」。`useRagData` 新增 `batchTiming`（startedAt/fileStartedAt/name/tick）——**hook 层持有，弹框可关闭重开而计时不清零**（用户明确要求：关闭后再打开不能重置时间）。起点取**首个 `rag://batch-update-progress` 事件到达时刻**（事件驱动——定时自动同步不是前端发起的，事件是唯一可靠起点；手动按钮启动时先行初始化填补确认到首事件的间隙）；文档名变化重置单文档时钟；done/error 终态清空（终态不显示时间）。i18n 复用同两键。
- **分片进度（第三条进度条，2026-09-23 追加，用户需求）**：`rag://upload-progress` 事件加 `chunksDone`/`chunksTotal` 字段（serde default 向后兼容）——`reindex_doc` 嵌入循环逐批推进 chunk 计数（`emit_upload_progress_chunks`）；分片阶段 total=0（数量未知）前端隐藏该条，分片完成嵌入开始后显示「分片进度 N / M · P%」。导入浮层与批量更新弹框都加（共用 charProgress，其类型扩为 5 字段，4 个构造点同步）。**分片阶段显示「分片中…」占位而非隐藏**（初版隐藏导致分片期间整条消失——恰是最长最无反馈的阶段，用户实测反馈）。i18n 新键 `pages.rag.chunkProgress` / `chunkingInProgress` ×4。
- i18n 新键 ×4 语言：`pages.rag.totalElapsed` / `fileElapsed` / `largeFileHint`（inline fallback 已带）。
- 验证：`npx tsc --noEmit` 24 = 基线（零新增）；`npm run build` ✓（1.22s）；四语言 JSON 解析 + 键存在性校验通过。


- `service.rs::reindex_doc` 的 0% 进度 tick（`emit_upload_progress(0, total_chars)`）从 chunking 之后**提前到 chunking 之前**——大文件分片阶段（现在也可能几十秒）进度条不再停留在「Preparing…」无响应，而是立即显示真实总字数。

**字符进度条分母修复（2026-09-23，用户实测反馈：字符条已 100% 而分片条未满）**

- **根因**：`rag://upload-progress` 的 `charsTotal` 用**文档字符数**作分母，而 `charsDone` 是嵌入循环**逐 chunk 累加**——相邻 chunk 共享 `chunk_overlap` token 的尾部文本被重复计数，累加值提前到达文档总数后被 `min()` 钳制，字符条先满、chunk 条还在走（重叠占比 ≈ overlap/(chunk_size+overlap) ≈ 10%，几千 chunk 的大文件提前量非常明显）。
- **修复**（`service.rs::reindex_doc`）：分片完成后计算 `progress_total_chars` = **所有 chunk 字符数之和**，嵌入循环 tick 与空 chunk 兜底分支都用它作分母——恰为循环累加口径，两进度条严格同步、最后一片嵌入完成时恰好 100%。分片前的 0% 预 tick 仍用文档字符数（此时 chunk 未知，占比 0% 不受影响）。`RagUploadProgress` 结构体注释同步更新。
- 验证：`cargo check --lib` ✓；`cargo test --lib` **48 passed**。

**分片阶段进度可视化（2026-09-23，用户需求：1m30s 的分片期也要有进度）**

- **后端**：`RagUploadProgress` 加 `chunkingDone`/`chunkingTotal`（serde default，0/0=不适用）；`ChunkStrategy::chunks` trait 加 `on_progress: Option<&dyn Fn(u64, u64)>` 参数（Text/Markdown/Empty 忽略，Code 策略用）；`chunk_document` 拆为 `chunk_document_inner` + 公开的 `chunk_document_with_progress`（旧签名 wrapper 保留，测试不动）。**进度来源**：大文件（>64KB）代码分片本就逐 AST 分区处理（`CODE_PARTITION_BYTES`），每完成一个分区回调 `(累计已扫描字符, 文档总字符)`——分区铺满输入无间隙，最后一次 tick 恰好 100%。单分区小文件/解析失败回退路径在结束时补发一帧 100%。`reindex_doc` 用闭包接 `emit_chunking_progress` 发同一 `rag://upload-progress` 事件（charsDone 保持 0、charsTotal=文档总字符，嵌入条不受扰动）。⚠️ 仅代码策略有粒度钩子（text-splitter 无 per-chunk 回调），纯文本/Markdown 大文件分片期仍无细分进度（相对快）。
- **前端**：`charProgress` 类型（useRagData state + BatchUpdateDialog prop + 2 处种子）扩 `chunkingDone`/`chunkingTotal` 字段；「分片进度」行在分片期显示「分片中… N%」，并新增贯穿两阶段的 6px 填充条（分片期按 chunkingPct、嵌入期按 chunkPct 填充）。导入浮层与批量更新弹框同款。
- 验证：`cargo check --lib` ✓；`cargo test --lib` **48 passed**；`npx tsc --noEmit` 24 = 基线；`npm run build` ✓。

**新回归测试**

- `huge_multiline_bundle_partitions_and_tiles`：12 行 × 2000 语句的仿 mermaid 形态（>256KB 深层多行 AST），断言 <60s 完成 + 内容零丢失（空白剥离后拼接 == 输入）+ chunk 全部 ≤ 容量阀。**注意**：该测试仍用桩 tokenizer，只能守护「分区逻辑 + 内容不丢失」；性能守护依赖 `bench_tmp` 人工 bench（见下）。
- **教训（MUST FOLLOW）**：涉及性能的回归测试，**测试桩与生产实现的成本特征必须同量级**——桩 tokenizer µs 级 vs 真实 BPE ms 级，1000 倍差距下「测试绿」对生产毫无证明力。真实模型 bench（临时 `src-tauri/tests/bench_tmp.rs`，跑完已删）：`#[ignore]` 测试 + `GgufEmbedder::load(runtimes/rag/model/granite/97m/model.gguf)` + 真实文件路径，release 下需 `CARGO_PROFILE_RELEASE_PANIC=unwind` 覆盖 tauri 的 `panic="abort"` 配置（否则 test harness 与 abort 依赖树冲突，712 个链接错误）。需复现时按此重建。

#### 3.17.17 导入会话守卫：导入中不刷新 Git 仓库 + 关窗取消剩余导入（2026-09-23）

> 用户反馈两个问题：①导入进行中日志出现 `[RAG] git: refreshed repo ... -> commit ...`——还没导入完就开始刷新仓库，不对；②导入文件还没解析完时关闭程序，下次进来文件全在列表里了——只有导入完成过的文件才应该出现在列表中。

**根因**

1. **导入中刷新**：自动定时 tick（默认 5 分钟）→ `preview_batch_update` → `collect_git_repo_urls(docs)` 收集已有 git 文档引用的仓库并 `refresh_persistent`（重克隆 + **rename swap** 持久目录）。导入循环（每文件 `upload_rag_doc`，读的正是该持久目录）一旦跑过 5 分钟，tick 就在导入中途 swap 目录——既浪费（克隆至多几分钟旧）又有 `ensure_persisted`/文件读取与 swap 的竞态，且日志误导。
2. **关窗后文件全在列表**：`lib.rs` 的窗口关闭 = **隐藏到托盘（`window.hide()`），进程与 webview 继续运行**——前端上传循环在隐藏窗口里继续跑完全部文件并落盘。用户以为「关闭程序」取消了导入，实际上导入在后台全部完成。真正的进程退出（托盘退出/Cmd+Q）反而是对的：`write_doc_and_index` 的顺序是 写 content → reindex（分片+嵌入）→ **写 meta** → SQL upsert，meta 是列表事实源（`list_docs` 扫 `.meta`），中途杀进程只有已完成文件入列——符合「只有导入进来的才会出现」。

**修复一：导入会话（IMPORT_SESSION）+ 全链路守卫**

- `rag/service.rs` 新增 `IMPORT_SESSION: AtomicBool` + `begin_import_session()`/`end_import_session()`/`import_session_active()`。store 语义（非计数器）：导入弹框是模态的、同时最多一个循环，且 `end` 无条件清零可自愈泄漏的 begin。
- 新命令 `commands::rag::begin_rag_import_session`/`end_rag_import_session`（lib.rs 注册；tauriClient 路由 `/rag/import-session/begin|end`；ragService `beginRagImportSession`/`endRagImportSession`）。
- `useRagData.upload()`：循环前注册 `rag://import-cancel-requested` 监听（**先注册后 begin**，杜绝事件落入无监听的间隙）→ best-effort `begin` → 循环每迭代检查取消标志 → finally 中 `unlisten` + **await `end` 放最后**（新 upload 的 begin 不会越过旧 end）。
- 守卫生效点（全部在 `rag/service.rs`）：
  - `refresh_git_repo`：会话激活时直接 `Ok(false)` 跳过 + 日志 `git: refresh deferred (import in progress)`，**不写 TTL 缓存**（导入结束后的下一次 tick/manual batch 立即补刷）；
  - 自动 tick（`restart_auto_update_timer` 循环）：`is_enabled()` 检查后新增 `import_session_active()` → `continue` 跳过整轮（refresh + md5 + source sync 全不跑），日志 `auto-update: import in progress, skipping this tick`；
  - `preview_batch_update_inner` 与 `run_batch_update`：source-sync 的 `sync_added` 在会话激活时清空（防 sync-add 与导入循环互抢同批文件造成重复文档），removal 同步不受影响照常。
- `check_rag_update`（单条检查）经 `refresh_git_repo` 的守卫自动获得同样语义（对既有克隆分类，不报 gitError）。

**修复二：关窗 = 取消剩余导入**

- `lib.rs` `CloseRequested`：`api.prevent_close()` 后检查 `rag::service::import_session_active()`，为真则 `rag::service::request_import_cancel(window.app_handle())`——写日志 `import: cancel requested (window closed mid-import)` + emit `rag://import-cancel-requested`，然后照常 `window.hide()`。
- 前端循环在**下一个文件边界**停止：正在解析中的当前文件照常完成并入列（属于「导入进来的」），其余文件不再导入。重开窗口后列表只含已完成文件。
- 兜底：真退出（托盘退出/Cmd+Q/杀进程）时循环随进程消亡，per-file 原子性已保证列表只含完成文件，无需额外处理。

**边界**

- 会话由前端 bracket，前端若崩溃不调用 end → 静态标志随进程消亡（重启自然复位）；单进程内 begin/end 的 store 语义自愈任何泄漏。
- pick/scan 阶段（未点导入）无会话：首次导入前无 git 文档 → tick 无 URL 可刷；重复导入同一仓库时 pick 期间 tick 可刷新**既有**仓库——无害（pick 用 temp 克隆，refresh 只动持久目录）。
- 「自动同步新增文件」开关若开启，**设计上**会在后续更新检查中把源目录/仓库里未导入的文件自动导入——被中断导入遗留的未选文件同样会被同步进来（与「新文件」语义无法区分）；不希望如此时关闭该开关（默认关闭）。

**验证**：`ORT_SKIP_DOWNLOAD=1 cargo check --lib` 通过（11.08s）；`npm run build` 通过（1.24s）；`npx tsc --noEmit` 24 错误 = 基线 24（零新增）。

#### 3.17.18 列表⟺向量一致性：诊断澄清 + lancedb 清空自愈（2026-09-23 第二轮）

> 用户二次澄清：卡在 6/xxx（大 JS 分片慢）时杀掉程序，重开后列表全在但「向量没跑完」；明确需求——**导入多少列表就多少，不要排除机制，未导入的下轮更新自然同步（已接受），但列表里绝不能出现向量搜不到的文档（起码的数据一致）**。据此回退了首轮起草的「排除清单」方案（未落地，仅 models/rag.rs/service.rs 两处草稿已撤）。

**真实数据诊断（临时审计测试，跑完已删）**

对用户机器实际数据跑 `meta.chunk_count>0 ⟹ lancedb.read_chunks_by_doc 非空` 全量核对：**121 个 meta 全部有向量，0 缺失、0 数量不符**。所报现象的真相：卡在 6/xxx 期间，**60 秒自动 tick 的 source-sync 在后台把剩余 87 个文件全部带向量导入**（日志 `10:40:14 batch_update: source sync applied (added 87, removed 0)`，逐条 `indexed ... chunks=N`）——列表全在且可搜是 tick 干的活，进度条 6/xxx 之所以不动是前端循环卡在大 JS；「向量没跑完」是进度条误导下的推断。**§3.17.17 的会话守卫正是根治**：导入期间 tick 全跳过 → 进度真实（6/xxx 就是 6）、杀掉后列表只有 6、下轮 tick 自然补齐（用户接受的语义）。

**数据一致性防线（真正的洞 + 修复）**

写路径顺序（`write_doc_and_index`：向量 add_chunks → meta 原子写 → SQL 镜像）已保证「列表 ⊆ 向量」。唯一真实漏洞：**lancedb 目录被清空/损坏/误删时，`VectorDb::open` 静默重建空表且 `needs_reindex=false`**（「全新创建无旧数据」语义），而 meta 仍记 chunk_count>0 → 列表显示「已索引」但向量搜索永远搜不到——与用户投诉的形态完全一致。修复：

- `vectordb.rs` 新增 `count_rows()`（lancedb `Table::count_rows(None)`）。
- `service.rs::start()`：`!needs_reindex` 分支新增一致性检查——`count_indexed_metas(app)`（新 helper，扫 meta 数 chunk_count>0 的文档数）> 0 且 `db.count_rows()==0` → 判定向量丢失，warn 日志 + `zero_all_chunk_counts` + 置 `needs_reindex=true`，与模型换维路径同构（前端按既有 needs_reindex 流程提示重建）。
- 刻意只做「空表 vs 有索引 meta」判定：lancedb 可能存在孤儿向量（杀进程在 add_chunks 后、meta 写前），严格按行数对账会误报。

**验证**：`cargo check --lib` 0 错 0 警；`cargo test --lib` 48 passed / 0 failed（含 §3.17.17 的 import_session 测试）。前端无改动。

#### 3.17.19 树形视图「文件比目录还靠前」渲染顺序修复（2026-09-23）

> 用户截图：导入弹框扫描树中，「技术选型」目录展开后，其直属 `README.md`（443B）被渲染在子目录「存储选型」的展开内容（README.md 231B + 数据存储.md）**之后**，视觉上文件脱离父级缩进、混在子目录文件中间——「文件比目录还靠前」。

**根因**（纯前端渲染顺序，两处同病）：

- 导入弹框扫描树 `RagPage.tsx::ScanDirNode`：`!isCollapsed` 分支**先渲染 `node.children`（子目录）再渲染 `node.rootFiles`（本目录直属文件）**——直属文件被推到子目录的展开内容之后，缩进层级（`depth`）不变导致视觉错位。
- 主页文档树 `RagDocTree.tsx::DirNode`：同样先 `children.filter(!c.doc)`（子目录）后 `children.filter(c.doc)`（直属文档）。

**修复**（文件管理器惯例：直属文件在前、子目录在后）：

- `ScanDirNode`：渲染顺序改为 `node.rootFiles`（直属文件）→ `node.children`（子目录递归），附注释说明。
- `DirNode`：直属文档 → 子目录，同注释。

数据层无需改动：`scan_folder` 的 groups 排序（root 优先 + 路径序）与 `RagDocTree` 的 `sortRec`（目录/文件各自排序后拼接）本就正确，问题只在组件渲染拼接顺序。

**验证**：`npx tsc --noEmit` 24 = 基线（零新增）；`npm run build` 通过（981ms）。Rust 无改动。

#### 3.17.20 平铺扫描视图文件行「顶到头」缩进修复（2026-09-23）

> 用户截图（平铺视图）：`scripts/honkit-plugin-image-zoom 2/2` 分组头下的 `index.js`/`package.json` 文件行内容起点比分组头名字**更靠左**——分组头 = 10px padding + checkbox/chevron/folder 三图标（名字起点 ~73px），文件行仅 `padding-left: 14px`（起点 ~27px），视觉上文件「顶到头」、与分组头平级。

**修复**（两轮，最终对齐 folder 图标列）：

- **平铺**（`FlatScanFileRow`）：`padding-left` 14px → **56px**（= 10 + checkbox13 + gap8 + chevron17 + gap8，文件图标正落在分组头 folder 图标列之下）；收起提示行同步 32px → 34px。
- **树形**（`ScanDirNode` rootFiles 行）：`paddingLeft` 24 + depth×14 → **52 + depth×14**（树形头 gap-1.5：10 + 13 + 6 + 17 + 6 = 52），同一对齐语义。
- 首轮 34px 修正后用户反馈「没有变化」——排查确认 vite dev server 已 serve 新代码（curl 验证），是 **webview 未热重载**（HMR socket 失效）；第二轮顺手把对齐目标从「名字起点下」改为更标准的「folder 图标列下」。用户需重开导入弹框 / Cmd+R 刷新 webview 才能看到。

**验证**：`npx tsc --noEmit` 24 = 基线；`npm run build` 通过（1.01s）；curl 确认 vite serve 新代码。Rust 无改动。

#### 3.17.14 导入弹框勾选冻结 / 树形空 / 模态点击穿透修复（2026-09-16）

> 用户反馈三个交互问题：①文件行点击取消选中一次后，后续点击完全无响应；②文件夹扫描的树形视图空白（只有平铺有数据）；③RAG 关闭时导入弹框整体不可点击。

**修复一：勾选状态 updater 重放丢失（根因修复）**
- React 19 dev 模式（StrictMode）会把 `setState` 的 **updater 函数以不同 base 重放执行**。toggle 类 updater（`has ? delete : add`）以 `{b}` 重放 → `{}`，再以 `{}` 重放 → `{b}`——两次重放**相互抵消，更新静默丢失**，表现为「取消选中一次后，再点击毫无效果」。
- **修复**（`RagPage.tsx`）：引入 `selectedRef`（与 state 同步的 ref），所有勾选变更先从 ref 读「最新已提交值」计算出**绝对新集合**，经统一入口 `applySelected(next)` 一次性写入（ref + setState）。绝对写入幂等，不受重放次数影响。全部勾选操作（单行/整组/清除组/全选/重置/合并扫描/handlePickFolder/handlePickGit/resetPickState）都改走该入口。
- **同时禁用 StrictMode**（`main.tsx`）：桌面端开发不需要其副作用双重检查，注释说明原因。两道防线叠加。

**修复二：文件夹扫描树形视图空白**
- 根因：`scanTreeNodes` useMemo 只返回 `root.children`；扁平文件夹（文件 relPath='' 挂在 `root.rootFiles`）的文件从未被返回 → 树形视图空。
- **修复**：根层级文件存在时前置一个虚拟「(根目录)」节点（key=`'__root__'`），并加入默认收起 effect 的依赖键。

**修复四：取消勾选后行消失 → 后续点击「无效果」（2026-09-17，用户复测反馈）**
- 根因：此前把「未勾选即隐藏」做进了列表渲染——树形 `prune` 把无已勾选文件的目录分支整个删除、虚拟根节点仅在有已勾选文件时渲染；平铺 `sel.length===0 return null` 隐藏整个分组。**取消勾选（尤其组内最后一个选中文件）后行/分支立即从 DOM 消失，用户无法再点击重新勾选**，感知为「第一次取消选中有效，后续点击无任何效果」。这不是 React 调度问题。
- **修复**：删除 prune 与分组隐藏逻辑——扫描到的所有文件/分组**常显**（未勾选 = 弱化态灰字 + 空心图标，样式已有），虚拟根节点在 `root.rootFiles.length > 0` 时始终渲染。
- **同修：平铺「全部收起」失效**：平铺分组折叠 key 用 `g.relPath`（根分组为 `''`），而 collapsed 集合存的是 `'__root__'`（树形节点 key）——key 不匹配导致根分组永远收不起来。修复：平铺分组折叠 key 归一化 `relPath || '__root__'`（渲染判断 + 点击 toggle 两处）；`toggleCollapse` 改为基于最新 state 的绝对值写入（不再用 updater 形式，与 applySelected 同源防重放抵消）。

**修复五：勾选更新的真正根因——原地突变 Set + Object.is 吞更新（2026-09-17 第二轮深挖，生产构建无头复现钉死）**
- 此前归因于「StrictMode updater 重放」不完整——生产构建（无 StrictMode、无 HMR、react 19.1.1/19.2.8 双版本实测）同样复现「第一次点击生效、后续全部无效」。
- **完整排查链**（生产构建 + headless Chrome + React DevTools hook 注入）：handler 内状态每次都正确（ref/state 日志）→ render 计数器停在第一次更新 → 注入 `onCommitFiberRoot` 确认 commit 存在但不含变化 → 交替点击实验发现**更新总是「慢一拍」显现**（下一次任意无关 setState 时状态追上）→ fiber 检查发现 hook 的 `lastRenderedState` 已是新值但未触发渲染。
- **根因**：`resolveSelected(prev)` 在 prev 非空时**返回原 Set 实例**，toggle 类操作（`delete`/`add`）**原地突变**该实例——而它同时是 React `useState` queue 里的 `lastRenderedState`。下次 dispatch 时 React 的 eager 优化比较 `Object.is(eagerState, currentState)`——**action 与 lastRenderedState 是同一个对象**，恒相等 → 更新被判定「无变化」**直接跳过，连调度都不发生**。第一次点击有效是因为全选哨兵物化时创建了新实例。
- **修复**：所有变更路径先 `new Set(...)` **复制再增删**（`toggleFileSelected`/`toggleGroupSelected`/`clearGroupSelected`；`setAllSelected`/`mergeIntoScan`/`resetSelectedPaths` 本就新建/置 null 无需改），并加注释说明缘由。`toggleCollapse` 同步改为绝对值写入。
- **教训**：**放进 setState 的对象绝不能与 React 内部持有的对象是同一实例且被原地突变**——`Object.is` 相等时 React 会合法地吞掉更新。可变对象（Set/Map/数组）作 state 必须不可变更新。
- **验证**：生产构建 + headless 8 连击（3 文件 × 选中/取消交替）全部正确；平铺 Collapse all 生效；`tsc` 24=基线；build ✓。排查用临时代码（bridge/MiniTest/flushSync/dbg 日志/react 降级）全部还原。

**修复三：RAG 禁用时模态点击穿透**
- 根因：页面根容器 `disabled` 时带 `pointer-events-none`，模态（fixed 定位在其内部）被连坐——`elementFromPoint` 命中的是底层页面，所有点击静默丢失。
- **修复**：`UploadDialog`（及批量更新等模态）根节点显式 `pointerEvents: 'auto'`。

**注**：排查中还确认了一个无头测试环境伪影（502 请求风暴 + dev 双调用下 React 同步渲染偶发半途而废，无错误、无提交），不影响真实 Tauri 运行环境，未做处理。

**验证**：`npx tsc --noEmit` 24 = 基线（零新增）；`npm run build` ✓；Rust 无改动。

---

### 3.18 RAG 文件排除（导入时勾选排除，更新检查忽略，桌面端独有，2026-09-23）

> 需求：文件导入时支持「排除」——后续自动更新 / 手动批量更新 / 数据源同步时忽略该文件。排除注册表存 `config_json.rag.excludedPaths`（JSON 字符串数组，绝对路径），**目录条目排除其下全部文件**（复用 `path_under_root` 前缀语义）。无 DB 迁移。

#### 3.18.1 后端（`src-tauri/`）

- **模型**（`models/rag.rs`）：`RagScanFile.match_path`（扫描文件对应的注册表匹配路径；文件夹扫描 = 文件绝对路径，Git 扫描由 `git_pick_inner` 在 temp→persistent 映射后**重打**——与文档将要记录的 `original_path` 一致，排除 Git 文件才能挡住后续 sync 再导入）；`RagDocInfo.excluded` / `RagDoc.excluded` / `RagUpdateCheck.excluded`（`#[serde(default, skip_serializing_if = "Not::not")]`，false 不上线）；`BatchPreview.excluded: u32`（`skip_serializing_if = "u32_is_zero"`，0 不上线）。
- **注册表服务**（`rag/service.rs`）：`list_excluded_paths()`（config 读失败 fail-open 返回空集）/ `set_excluded_paths(paths)`（trim + 去空 + 保序去重后整体替换）/ `is_path_excluded(path, &reg)`（async，目录前缀匹配）/ `path_excluded(path)`（单路径便捷）/ `is_excluded_sync(path, &reg)`（同步版，分类循环用）。
- **管线跳过点**（全部生效）：
  - `plan_source_sync`：排除路径的扫描文件不算「新增」（folder/git 两分支）；排除文档的 original_path 不参与「删除」判定（folder/git/file 三分支）。
  - `preview_batch_update_inner`：排除文档计入 `excluded` 计数并跳过 lost/skipped/to_update 分类。
  - `run_batch_update`：排除文档跳过重索引。
  - `check_rag_update`：结果带 `excluded: true`（UpdateDialog 提示 + 取消排除入口）。
  - `doc_info_from_meta`：列表/分页行带 `excluded` 徽章（**注册表由调用方一次读入传入**——list_docs / search_docs_paged 两条路径各读一次，非每文档一次 DB 读）。
  - `get_doc_inner`：RagDoc 带 `excluded`（单文档一次 `path_excluded`）。
- **命令**（`commands/rag.rs` + `lib.rs` 注册）：`list_rag_excluded_paths` / `set_rag_excluded_paths`；`git_pick_inner` 扫描后对每个文件 `map_temp_to_persistent` 重打 `match_path`。
- **单测**（`service.rs::exclusion_tests`）：`path_under_root` 目录语义（兄弟前缀不误配）、`is_excluded_sync` 文件+目录匹配、`normalize_excluded_paths` trim/去空/去重。`cargo test --lib` **51 passed**。

#### 3.18.2 前端（`frontend/`）

- **API**：`ragService.listRagExcludedPaths()` / `setRagExcludedPaths(paths)`；`tauriClient.ts` 路由 `GET /rag/excluded-paths` + `POST /rag/excluded-paths/set`；`types/index.ts` 对应字段（RagScanFile.matchPath / RagDoc.excluded / RagDocInfo.excluded / RagUpdateCheck.excluded / BatchPreview.excluded）。
- **导入弹框**（RagPage UploadDialog）：文件夹/Git 数据源（file 数据源不参与数据源级同步，不提供排除）每文件行尾 **Ban/Eye 切换按钮**（平铺 FlatScanFileRow + 树形 ScanDirNode 文件行），目录/分组头**整组切换按钮**；排除行 55% 透明 + 「已排除」徽章。状态 = `excludedBase`（打开弹框时注册表快照，弹框内可移除）∪ `excludedLocal`（弹框内新增）；**确认导入时 diff 持久化**（读当前注册表 → 删 removed 加 added → 整体写回），取消/关闭丢弃本地态。
- **排除 ⇄ 勾选互斥（2026-09-23 补充）**：已排除文件**强制未选中且不可选中**——①null「全选」哨兵物化（`resolveSelected`）、组切换（`toggleGroupSelected`）、全选（`setAllSelected`）、合并扫描（`mergeIntoScan`）全部跳过排除路径；②文件行点击对排除文件无效果（cursor: not-allowed）；③目录/分组头勾选框与计数（`selCount`/`allChecked`/indeterminate/折叠提示/`groupCount`）只统计未排除文件。排除 = 不导入，与勾选状态天然互斥，无「勾选了但不导入」的歧义态。
- **文档列表**：行内「已排除」徽章 + 操作列 Ban/Eye 切换按钮（直接读写注册表 + refreshDocs）；批量勾选条新增「排除更新」按钮（选中集全部排除 / 全部取消，toggle 语义）。
- **UpdateDialog**：`check.excluded` 时显示提示块（批量/自动更新与数据源同步都会忽略，手动上传仍可用）+「取消排除」按钮（移除注册表条目后重查）。
- **BatchUpdateConfirmDialog**：新增「已排除（忽略）」统计卡（第 7 格）。
- **i18n**：`pages.rag` 新增 14 键 ×4 语言（excludedBadge/Tip、excludeAdd/Remove、excludeAddDir/RemoveDir、excludeNoPath、excludeToggleFailed、excludeRemovedToast、batchExclude/Hint、batchScanExcluded、updateExcludedHint、updateReinclude）。

#### 3.18.3 边界

- 排除**不阻断手动上传更新**（update_doc_from_file / from-original 不查注册表）——用户显式操作优先。
- 排除路径匹配的是 `original_path`（导入时记录的绝对路径）；file 数据源文档同样可从列表排除（其源文件消失时不再被同步删除）。
- Git 重打 match_path 失败（意外路径）时保留 temp 路径——永不匹配注册表，仅退化为「无法在弹框内排除该文件」，无副作用。
- 注册表写失败（config_service Err）时导入弹框确认流程不回滚已成功的导入（best-effort，console.warn）。
- 验证：`cargo check` 0 错 0 警；`cargo test --lib` 51 passed；`npx tsc --noEmit` 24 = 基线；`npm run build` ✓。

#### 3.18.4 数据源级手动更新（树形视图数据源节点刷新按钮，2026-09-23）

> 需求：RAG 列表树形结构时，在数据源节点上也放一个「更新」按钮，做数据源级别的手动更新。

- **后端**（`rag/service.rs::refresh_source_update` + `commands/rag.rs::refresh_rag_source`，`lib.rs` 注册）：
  - 入参 `SourceUpdateTarget { kind: "git"|"folder", url, root }`（camelCase）。
  - **数据源范围预扫描**（`preview_source_update` + `preview_rag_source_update` 命令，2026-09-23 补充）：与批量预览同构的 `BatchPreview` 计数（total/toUpdate/skipped/lost/added/removed/excluded/gitErrors），但只统计该数据源的文档（显式 source 身份 git url / folder root；legacy 文档按 original_path 前缀）；git 目标先强制拉取远端（TTL 绕过），失败进 `gitErrors`（确认框已有渲染）；范围同步计划走共享的 `plan_source_sync_scoped`（与执行阶段同一函数，确认框数字与实际执行严格一致）。预扫描只读、无 CAS 守卫。
  - **git**：先删 `GIT_REFRESH_CACHE` 的 TTL 条目（**强制重拉**，用户显式点击），再 `refresh_git_repo`（失败返回结构化错误——auth 前缀「认证失败：」/其他「拉取失败：」+ detail，前端 toast）。
  - **folder**：校验 root 存在。
  - **数据源范围同步**：`plan_source_sync_scoped`（全量计划后按目标 root/url 前缀过滤 + legacy folder root 补扫），add/remove 两闸门（「自动同步新增/删除文件」开关）与批量流程一致；有变化时发 `phase="sync"` 进度事件并 `run_source_sync`。
  - **数据源范围重索引**：重收集 metas 后按 `source.git.url`（git）/ `source.root`（folder；legacy 无 source 文档按 original_path 前缀）过滤本数据源文档，`classify_original` + `update_doc_from_original` 逐条 md5 变更重索引（排除/丢失/无路径跳过）。
  - **守卫**：与批量/自动更新共用 `BATCH_UPDATE_RUNNING` CAS（互斥不重叠）+ import-session 检查；**内联执行**（命令 await 返回 `(added, removed, updated)` 计数），进度走同一 `rag://batch-update-progress` 事件——前端 batchUpdateRunning 状态与进度弹框对两种更新通用。
  - 变更后按需 `rebuild_rag_sql_index`。
- **前端**：
  - `RagDocTree.tsx`：新 props `onRefreshSource` / `refreshingSourceKey`；sources 分组携带 `gitUrl`/`sourceRoot`（取组内首个文档）；git/folder 数据源节点**行尾**渲染 RefreshCw/Loader2 按钮——按钮放在与文件行**等宽的 170px 容器内右对齐**（`justify-content: flex-end`），落点与文件行按钮列完全重合（首版按钮放容器左侧曾错位，用户截图反馈后修正）；stopPropagation 不触发折叠，进行中转圈禁用，tooltip 说明三步动作；file/tool 数据源无按钮（无远端可拉）。
  - `RagPage.tsx`：**与批量更新同流程（2026-09-23 补充）**——点击刷新按钮先弹**数据源范围**的确认框（`preview_rag_source_update` 预扫描：git 先强制拉取远端，统计只算该数据源的文档/新增/移除/排除），用户确认后 `startSourceUpdate` 才执行 `refresh_rag_source`（进度弹框复用批量的）。`BatchUpdateConfirmDialog` 扩展可选 `title`/`subtitle` props（数据源级传「数据源更新」标题 + url/root 副标题）；已有更新在跑时只重开进度弹框（与批量按钮同语义）；完成 toast（导入/移除/更新计数）+ refreshDocs。
  - `ragService.refreshRagSource(kind, {url, root})` / `previewRagSourceUpdate(kind, {url, root})`；`tauriClient.ts` 路由 `POST /rag/source-update` + `POST /rag/source-update/preview`。
  - i18n：`pages.rag.refreshSource` / `refreshSourceDone` / `refreshSourceTitle` ×4 语言。
- **legacy 文件夹数据源（无 source 元数据、按 original_path 父目录 classify）**：`plan_source_sync` 只扫显式 folder root，这类 root 永不出现在计划里——`refresh_source_update` 对「root 下无任何显式 folder source 文档」的 folder 目标**直接补扫该 root**（`scan_folder_public` 递归，truncated 跳过，排除注册表 + 引用集判定照常），使 legacy 文件夹数据源的刷新按钮同样能导入新增文件；移除/重索引路径经 `scope_hit(original_path)` 天然覆盖。
- **验证**：`cargo check` 0 错 0 警；`cargo test --lib` 51 passed；`npx tsc --noEmit` 24 = 基线；`npm run build` ✓。

---

### 3.19 OpenAPI 兼容端点（/api/*，镜像 origin openApiController，2026-09-24；同日复核轮补齐 query 参数 + 辅助端点 + E2E 实测）

> 基线同步（§4.4，`8ed6478`）把 ServerCard/GroupCard 的 `/api/` 端点 chip 带进前端，但桌面 Rust `http_server.rs` 此前无对应路由（访问 404）。本节在 Rust 端补齐实现，使 chip 语义完整。前端 chip 复制的是 API base（`http://host:port/api/{name}`），客户端在其后拼接 `/openapi.json` 获取 spec。

**文件**：`src-tauri/src/services/http_server.rs`（「OpenAPI-compatible endpoints」区段 + `openapi_tests` 测试模块）

**路由（全部走与 /rest/*、/mcp/* 相同的 bearer-key 鉴权 + `get_allowed_servers` 服务器白名单）**：

| 路由 | 说明 |
| --- | --- |
| `GET /api/openapi.json`（`.yaml` 同响应 JSON） | 全局 spec：所有可达服务器（connected / 睡眠 on-demand / RAG builtin）的工具 |
| `GET /api/openapi/servers` | 已连接服务器名列表（origin getOpenAPIServers parity：`{success, data: [...]}`） |
| `GET /api/openapi/stats` | 工具统计（origin getToolStats parity：`{totalServers, totalTools, serverBreakdown:[{name, toolCount, status}]}`；status ∈ connected/connecting/sleeping/disconnected） |
| `GET /api/{name}/openapi.json` | 作用域 spec：`name` 先按分组解析（分组 servers + 工具 allow-list），否则按单服务器（含 RAG builtin）；无可达工具返回 404（origin 同文案） |
| `GET\|POST /api/tools/{server}/{tool}` | 全局作用域执行工具 |
| `GET\|POST /api/{name}/tools/{server}/{tool}` | 分组/单服务器作用域执行工具（校验 server 在 scope 内，否则 404） |

**spec 端点 query 参数（origin 全量 parity）**：`?title=&description=&version=&serverUrl=&includeDisabled=true|false&group=<分组名>&servers=a,b`——`serverUrl` 覆盖 `servers[0].url`（自动追加 `/api` 后缀）；`includeDisabled=true` 在 spec 中包含已禁用工具（仍仅限已连接服务器，与 origin 一致）；`?group=` 传不存在的分组返回**空 spec**（此参数不做单服务器回退——回退语义仅存在于 `/api/{name}/openapi.json` 路径形态，与 origin 语义一致）。

**spec 生成（镜像 origin openApiGeneratorService 语义）**：
- 工具收集 `collect_openapi_tools(scope, include_disabled)`：复用 `mcp_scope_server_filters` + `tools_for_server` + `apply_tool_filters`（禁用工具跳过、描述覆盖生效、分组 allow-list 按 bare name 匹配）；运行时前缀名（`{server}{nameSeparator}{tool}`）剥为 bare name。
- 操作形状 `tool_schema_shape`：纯基础类型参数（无 object/array/string）且 ≤10 个 → **GET query parameters**（schema 透传 type/enum/default/format，origin parity）；否则 → **POST JSON requestBody**（origin 同判定，string 也算复杂类型）。
- path 名 `/tools/{enc(server)}/{enc(bareTool)}`（encodeURIComponent 等价的最小 percent-encoding）；`operationId` = bare name，**跨服务器同名工具冲突时改用 `{server}_{tool}` 消歧**（origin 用全前缀名天然唯一，桌面用 bare 名 + 冲突去重，两者均满足 OpenAPI 唯一性要求）；`tags` = 服务器名。
- 文档结构：`openapi: 3.0.3` + `servers: [{url: base_url + "/api"}]` + `components.schemas`（ToolResponse/ErrorResponse）+ `security: bearerAuth`。`base_url` 优先取请求 `Host` 头（反代友好），回退 `localhost:{运行端口}`；恒为 `http://`（桌面 HTTP 监听无 TLS，TLS 反代场景应显式传 `serverUrl=`）。
- `.yaml` 路径不做 YAML 序列化（无依赖），与 `.json` 同返回 JSON——所有主流 OpenAPI 客户端均接受。

**工具执行 `execute_openapi_impl`**：
- 工具名解析：先裸名、再 `{server}{nameSeparator}{tool}` 前缀名（origin 语义）。
- 禁用工具 gate（与 /rest、/mcp 同源，#1178 安全语义；动态读 DB，改禁用状态即时生效无需重启）。
- GET 的 query 参数按工具 inputSchema 做类型纠偏（`coerce_query_args`：number/integer/boolean 字符串 → 对应 JSON 类型，解析失败回退原字符串，origin convertParametersToTypes 同义）；POST body 直接作为 arguments。
- 调用走共享 `pool::call_tool`（与 /rest 相同——无下游 session，不走 per-session 隔离）。
- 写 activity_log（含 64KB 载荷截断，§3.17.15）+ `[OpenAPI]` 前缀日志。
- 响应 `{content, isError}`（origin 同构）。

**使用方式（OpenWebUI 等客户端）**：把 `http://localhost:{httpPort}/api/{server 或 group}/openapi.json` 作为 OpenAPI spec 地址导入，客户端即按 spec 中每个工具的 `POST/GET /tools/...` 路径直接调用；bearer auth 开启时需带 `Authorization: Bearer <token>` 请求头（token 为 Bearer Keys 页签创建的 key）。
**仪表盘入口（2026-09-24）**：Dashboard 端点区两处改动——①「MCP 接入端点」卡片原「GROUP ×2 + SERVER 兜底」多行合并为单行 `SERVER/GROUP`，URL 占位 `<服务名或分组名>`（i18n 键 `pages.dashboard.namePlaceholder`；原 groupNamePlaceholder/serverNamePlaceholder 两键已删）；②「MCP 接入端点」卡片下方新增「OpenAPI 接入端点」卡片——两张 `EndpointCopy` 行（全局 spec `/api/openapi.json` + 命名 spec `/api/{服务名或分组名}/openapi.json`，均可一键复制），下方一行用法说明（占位符替换为服务器/分组名、简单参数走 GET、复杂参数走 POST JSON、Bearer Key 鉴权提示）。i18n 键 `pages.dashboard.openapi*` 4 键 × 4 语言（openapiTitle/openapiHint/openapiNamePlaceholder/openapiUsage，插在 endpointsHint 之后）。Dashboard 的 `useGroupData` 引用随之移除（无其他使用点）。

**与 origin 的差异（有意为之，记录在案）**：
- **鉴权更严格**：origin 的 /api/* 完全公开（仅限流）；桌面端在 `routing.enableBearerAuth` 开启时对全部 /api 路由强制 bearer 鉴权（含 spec 端点），关闭时与 origin 同为开放。安全增强，保持。
- **.yaml 返回 JSON**（origin 用 js-yaml 输出真 YAML）：避免引入序列化依赖，主流客户端不受影响。
- **UI-only 工具过滤缺失**：origin `filterModelVisibleTools` 过滤 `_meta.ui.visibility` 不含 `model` 的工具；桌面端无该概念（全局缺失，非 /api 特有），此类工具会出现在 spec 中。
- **限流缺失**：origin 对 /api 有 rate limiter；桌面端 HTTP 服务器整体无限流（既有状态，非 /api 特有）。
- **matchit 静态优先边界**：服务器恰好命名为 `openapi` 时其命名 spec（`/api/openapi/openapi.json`）会被 `/api/openapi/*` 静态路由挡住（matchit 无回溯；origin express 按注册序可匹配）。病态命名边界，不处理。
- origin 执行路径传 `x-session-id`（固定 `openapi-session`）+ 全部请求头透传；桌面无此透传链路（与 /rest 一致）。

**E2E 实测矩阵（2026-09-24 复核轮，针对运行中的 dev 实例真机验证）**：
- spec：全局（102 路径 / operationId 唯一 / method-body 一致性断言全过）、单服务器（playwright 25 工具）、CJK 服务器名（本机公网ip查询，percent-encoding 链路）、分组（Test → 4 已连接服务器、禁用服务器正确排除）、builtin（mcphub-desktop → 6 RAG 工具）、不存在名 → 404（origin 同文案）、`.yaml` → JSON、CORS `*`。
- query 参数：`servers=playwright` 过滤 ✓、`group=Test` ✓、`group=<不存在>` → 空 spec ✓、`serverUrl/title/version` 覆盖 ✓、`includeDisabled=true`（临时禁用 codegraph_files 后 7→8 工具，含隐藏/可见双向断言）✓。
- 辅助端点：`/api/openapi/servers`（4 名单）、`/api/openapi/stats`（96 工具 + breakdown）✓。
- 执行：GET 全局（getPublicIp）、GET 分组作用域、POST 空 body（codegraph_status）、POST 带 required 参数（codegraph_search query+limit）、POST 分组作用域——响应均 `{content, isError}` ✓；activity_log 行（server/tool/duration/status）全部落库 ✓。
- 错误路径：未知服务器 404 / 未知工具 404 / scope 外服务器 404 / PUT 405 / 禁用工具 403「is disabled」/ 鉴权开启后无 key、错 key 401 + 对 key 200 + spec 端点同受控 / `allowed_servers` 受限 key：spec 过滤到白名单服务器 + 执行越权 403——全部 ✓，测试后 DB 均已还原并复核还原生效。
- 对照实验：Idea-mcp-server 上游 404 经 `/mcp` JSON-RPC 复现同错——确认为上游失联（IDE 端点漂移），非 /api 路径缺陷。

**验证**：`ORT_SKIP_DOWNLOAD=1 cargo check --lib` 0 错 0 警；`cargo test --lib` **57 passed**（`openapi_tests` 模块 6 个：路由构建防 matchit panic、GET/POST 形态分流、operationId 唯一性、urlencode、coerce_query_args 类型纠偏、spec options 解析）。

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
| `frontend/src/components/ServerCard.tsx`         | 移除 sponsor/wechat/discord、样式调整；下载进度条 / 更新角标+「更新到」菜单项 / 名字后版本号（见 3.6）；传 `startOnDemand` prop 给 StatusDot（见 3.8） |
| `frontend/src/components/ServerForm.tsx`         | hub-* 样式、隐藏 visibility（删除 Advanced 分区内的选择器块）、visibility 默认 public、保留 OAuth2、`getInitialServerType` 跳过 builtin；表单 3 分区布局随上游 #1034 同步（见 3.2.1）；stdio 专属「按需启动」checkbox + idle timeout 输入框（见 3.8） |
| `frontend/src/components/ui/StatusDot.tsx`       | `startOnDemand` prop + 💤 Sleeping 渲染（见 3.8）            |
| `frontend/src/components/LogViewer.tsx`          | source 类型改为 string[]、source filter UI 移除、滚动方向 |
| `frontend/src/components/layout/Header.tsx`      | GitHub 链接、移除文档按钮                                 |
| `frontend/src/components/layout/Sidebar.tsx`     | Logo 使用应用图标                                         |
| `frontend/src/components/ui/UserProfileMenu.tsx` | 移除 sponsor/wechat/discord 按钮；更新检查上移到根级 provider，改为消费 `useUpdateCheck()`（红点 + openAbout），不再自带检查/对话框（见 3.4.7） |
| `frontend/src/components/ui/AboutDialog.tsx`     | MCPHub Desktop 标识、canAutoUpdate 逻辑、Markdown 渲染 notes、flex 滚动布局、Loader2 安装图标、移除「忽略此版本」（见 3.2.4 / 3.4.7） |
| `frontend/src/components/ui/Markdown.tsx`        | 桌面端新增（见 3.4.7）：react-markdown+remark-gfm 渲染 release notes，`inline` 模式供 highlights |
| `frontend/src/contexts/AuthContext.tsx`          | skipAuth/guest 模式                                       |
| `frontend/src/contexts/SettingsContext.tsx`      | httpPort/exposeHttp 字段                                  |
| `frontend/src/contexts/UpdateCheckContext.tsx`   | 桌面端新增（见 3.4.7）：根级启动更新检查 + 全局 AboutDialog，自动弹框/红点，无「忽略」 |
| `frontend/src/contexts/ServerInstallProgressContext.tsx` | 桌面端新增（见 3.6）：监听 install-progress / update-available 事件 |
| `frontend/src/App.tsx`                           | 包入 ServerInstallProgressProvider（见 3.6）+ UpdateCheckProvider（见 3.4.7） |
| `frontend/src/types/index.ts`                    | `Server.version` 字段（见 3.6）；`AuthState.skipAuth` 字段（见 3.5.12）；`ServerConfig.startOnDemand`/`idleTimeoutMs` + `ServerFormData` 同名字段（见 3.8） |
| `frontend/src/services/configService.ts`         | getPublicConfig 使用 apiGet                               |
| `frontend/src/services/changelogService.ts`      | Tauri 中 changelog API 禁用；新增 `buildChangelogFromTauriUpdate`、移除「忽略此版本」（见 3.4.7） |
| `frontend/src/utils/version.ts`                 | 本地修改（见 3.4.7）：`checkForAppUpdate(source)` + `logUpdateEvent` 全流程写 `[update]` 日志 |
| `frontend/src/pages/SettingsPage.tsx`            | 隐藏未实现模块、RuntimeVersionManager、HTTP 端口；免登录隐藏「修改密码」、导出 JSON 格式化修复、「下载 JSON」走原生保存对话框（见 3.5.12） |
| `frontend/src/pages/LoginPage.tsx`               | admin 默认填充、密码提示、Logo 图标                       |
| `frontend/src/pages/Dashboard.tsx`               | 隐藏 SMART/Docs                                           |
| `frontend/src/pages/ActivityPage.tsx`            | 隐藏用户列、createdAt UTC 转换、字段名统一为 createdAt     |
| `frontend/src/utils/tauriClient.ts`              | 桌面端新增；`toFrontendServer` 映射 `serverVersion`（见 3.6） |
| `frontend/src/utils/serverFormPayload.ts`        | config 按 serverType 分支构建、无 visibility；`perSessionClient` 加在 return 前；`startOnDemand`/`idleTimeoutMs` 携带（见 3.8） |
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
# 9. 在本轮 §4.4 条目末尾写「影响功能点与结果」总结（MUST）：
#    - 影响功能点：本轮同步实际影响哪些桌面端功能/模块（前端页面、Rust 命令/服务、i18n、DB 迁移等），逐条列出；无影响则明确写「无」
#    - 结果：同步后桌面端行为与上一版本的差异（新增能力/修复的问题/行为不变），以及是否需要用户操作（如重启、数据迁移）
```

### 4.3 最近同步基线

每次基线同步后，必须同步原项目的版本号，即：/Users/jphoebe/opt/code/IdeaProjects/github/mcphub-desktop/mcphub-origin/locales/zh.json文件中{{version}}
桌面的版本号规则为：{{version}}xxx, xxx代表当前桌面端的版本号，从001开始递增
| 项                             | 值                      |
| ------------------------------ | ----------------------- |
| **当前已同步到 origin commit** | `8ed6478` (origin/main，v1.0.39 tag 之后 9 个未发布提交) |
| **对应 origin tag**            | `v1.0.39`（最新 tag） |
| **桌面端版本号**               | `1.0.39001` |
| **同步执行日期**               | 2026-09-24            |

> 下次同步时，使用 `8ed6478` 作为新的基线 SHA 起点（命令：`cd mcphub-origin && git --no-pager log --oneline 8ed6478..HEAD`）。
>
> 注：本节「对应 origin tag」指 origin 仓库的最新 tag（与子模块指针所在 commit 未必相同——指针停在 tag 之后的未发布提交上）。本轮基线跨 origin 四个 release（`v1.0.36`~`v1.0.39`，`6b1fdb7 → 8ed6478`，43 个 commit），前端改动集中在 #1175（分组可见性）、#1129（per-user credentials）、#1186（包版本展示）、#1190/#1191/#1193（服务器复制 + 客户端配置预设复制）、#1199（Smart Routing 索引面板）。版本号 `1.0.35003 → 1.0.39001`：基线跟随 origin 最新 tag v1.0.39（35 → 39），序号从 001 重新开始；changelog `doc/upgrade/1.0.39001.md` 单文件。

### 4.4 最近同步记录

#### 2026-09-24：同步 `6b1fdb7` -> `8ed6478`（43 个 commit，跨 v1.0.36 ~ v1.0.39 四个 release）

origin 基线前进 43 个 commit（`6b1fdb7..8ed6478`；v1.0.36/37/38/39 四个 tag 均在本次范围内）。`git diff --stat 6b1fdb7..8ed6478 -- frontend/ locales/`：31 个文件 +3148/-197；`-- src/`：59 个文件 +4850/-272。

**已同步到 desktop（前端 / locales）**

| 来源 commit | 说明 | desktop 应用方式 |
| ----------- | ---- | ---------------- |
| `b3d0dcb` #1143 | remove deprecated baseUrl from tsconfig | 桌面 tsconfig 与 origin 同步（新文件直接采用 origin 版本，`serverName.ts` 补齐依赖）。 |
| `451102d` #1142 | use group names in copied MCP configuration | GroupCard 直接采用 origin 版本区段（GroupCard 为桌面自定义文件，手动合并）。 |
| `ae5962c` #1153 | log-stream reconnect backoff reset on open | `logService.ts` 手动合并：`onopen` 中重置 `openAttempts`（桌面端保留 Tauri 跳过 EventSource 的差异）。 |
| `fb60844` #1160 | slogan 措辞更新（open-source / control plane） | 4 语言 `auth.slogan` 同步为 origin 值。 |
| `319e191` #1129 | per-user credentials（CredentialsPage 等） | **部分采用**：新增文件 `credentials` locales、`credentialTemplate`/`CredentialSlot` 类型、`ServerConfig.resources` 字段、ServerForm 的 credentials slots 编辑器（Advanced 分区）；**跳过** CredentialsPage 路由/Header/Sidebar 入口（桌面端免登录单用户，无 per-user 语义）；`serverFormPayload` 不带 `credentialTemplate`（Rust 模型无此字段）。 |
| `1772c7a` #1175 | group visibility controls | **仅类型与 locales**：`Group.visibility/owner/sharedWithUsers` 类型 + `groups.legacyVisibility` 等 4 键；跳过 AddGroupForm/EditGroupForm/GroupVisibilityFields UI 与 GroupCard 的 `canManage`/visibility 行（桌面端无用户体系）。 |
| `629ff61` #1180 | OpenAPI endpoint URLs 展示 | ServerCard/GroupCard 手动合并：`/api/` 端点 chip + 复制（baseUrl 沿用桌面 httpPort 逻辑）。 |
| `260de33` #1164 | keep on-demand servers alive during calls | **Rust 镜像**：`on_demand.rs::run_call` 调用开始前 bump `last_used` + 重 arm idle timer，长调用不再被 idle 计时器中途回收。 |
| `afcc72f` #1178 | **CRITICAL**: disabled tools remain executable via tools/call | **Rust 镜像**：`http_server.rs` 的 REST `/rest/:server/call` 与 `/rest/group/:group/call` 补 disabled-tool gate（MCP JSON-RPC 路径与 Tauri 命令路径已有 gate）。 |
| `895f448` #1182 | scope npx reinstall cache clear | **Rust 镜像**：`servers.rs` 新增 `clear_npx_cache_for_specs`（读 `_npx` 各 entry 的 package.json 元数据匹配包 spec，仅删本服务器的 entry），替代整目录 `remove_dir_all`。 |
| `ee124ff` #1186 | show resolved package version + update availability | 前端展示层类型（`Server.packageVersion/latestVersion/updateAvailable`）+ `server.packageVersion/updateAvailable` locales；**运行时**桌面端已有自研 §3.6 实现（记录版本池 + `server://update-available`），不做 origin 的 registry 查询链路。 |
| `a793e38`/`47cd1fc` #1190/#1193 | duplicate server into pre-filled add form | 完整镜像：新增 `serverDuplicate.ts`（含 `SERVER_NAME_MAX_LENGTH` 依赖 `serverName.ts`）；AddServerForm/EditServerForm 采用 origin 版本（AddServerForm 去掉 `shareCandidatesFrom`，桌面无 share-candidates API）；ServerForm 加 `mode` prop + OAuth 子字段回传；ServersPage 加 B1 竞态守卫（edit/duplicate request-id）+ duplicate 接线；ServerCard 加 `onDuplicate`/`isDuplicating`。 |
| `93fdb6f` #1191 | per-client MCP configuration presets | 完整镜像：新增 `clipboard.ts`/`mcpClientSnippets.ts`/`CopyClientConfigDialog.tsx`；ServerCard/GroupCard 的复制动作改为打开预设对话框（桌面各自研 `copyText` 收敛为共享 helper）。 |
| `5ac617f` #1184 | reap idle dynamically-registered OAuth clients | 前端完整镜像（SettingsContext `clientTtl` 类型/默认/读取 + SettingsPage temp state/保存/UI 输入）；Rust 后端无 OAuth server，无镜像面。 |
| `aa46861` #1199 | Smart Routing performance & reindex panel | 新增 `SmartRoutingIndexPanel.tsx`/`smartRoutingService.ts` + SettingsPage 挂载（桌面 Tauri 下 Smart Routing 区块隐藏，按既有策略同步代码保留）+ 30 个 locales 键。 |
| `8ed6478` #1208 | docs only | 跳过（origin 文档）。 |

**已镜像到 desktop（Rust 后端）**：#1178（REST disabled-tool gate）、#1164（on-demand 长调用保活）、#1182（npx 重装 scoped 缓存清理）。其余后端 commit 逐项评估为无需镜像：

| 来源 commit | 说明 | 处理决策 | 原因分析 |
| ----------- | ---- | -------- | -------- |
| `f0d8428` #1157 | overlapping inits strand 'connecting' | **无需镜像** | 桌面端 `connect_server` 有 `is_starting` 重入守卫 + disconnect 清理（§3.6.1），无 stranded 路径。 |
| `2d4ad64` #1156 | isolate transport creation failures | **无需镜像** | 桌面端 `start_all` 每服务独立 `tokio::spawn`，`connect_server` 返回错误 status，单服务失败天然隔离。 |
| `306e636` #1149 | preserve single-route tool calls after session rebuild | **无需镜像** | 桌面端 HTTP server 不校验 session id 有效性（`strategy_for_session` 未命中即 fallback，请求继续），单服务器 scope 的 raw/prefixed 工具名双查已实现，rebuild 语义天然成立。 |
| `a55a650` #1162 | bound concurrent startup connects | **暂不镜像** | 桌面端已有 staggered startup（每服务间隔 2s），并发压力受限；服务量级小。后续需要时再引入 env-config 并发上限。 |
| `67ca123` #1135 | rate limit: count only failed auth | **无需镜像** | 桌面端内置 HTTP server 无 rate limiter/登录端点，无镜像面。 |
| `215a770`/`5ac617f`/`48981a3`/`9c5dc93` | OAuth server / BetterAuth / OAuth provider | **无需镜像** | 桌面端无 OAuth Server、无 Better Auth（§7 待办），无对应链路。 |
| `319e191` 后端 / `abf15c5` #1203 / `8aaac68` #1204 | per-user credentials 路由 + 上游错误脱敏 | **无需镜像** | 桌面端未实现 per-user credentials（无凭证注入层），无此错误泄漏面。 |
| `b6887ef` #1183 / `1db0aa7` #1194 / deps bumps | CI/deps | **不同步** | Node 依赖与 origin CI；桌面端有自己的 release.yml。 |
| `b23f1db`/`5861e75`/`17e04ea`/`e9faf82`/`180b251` 等纯文档 | docs | **跳过** | docs/ 按策略不同步。 |

**同步操作**：子模块指针 `6b1fdb7 -> 8ed6478`；版本号 `1.0.35003 -> 1.0.39001`（基线跟随 origin 最新 tag v1.0.39，35 → 39，四源 + Cargo.lock）；changelog `doc/upgrade/1.0.39001.md` 单文件。

**同步后验证**：`cd frontend && npm run build` 通过；`npx tsc --noEmit` 24 错误 = 基线 24（经 `git stash` 对照 HEAD 逐条确认，零新增）；locales JSON 四文件解析校验通过（deep-merge 保留桌面自定义键，en 81/fr 82/tr 82/zh 81 个 origin 新键合入）；`ORT_SKIP_DOWNLOAD=1 cargo check --lib` 0 错 0 警；`cargo test --lib` 51 passed。

**影响功能点与结果**：

- **影响功能点**：①ServersPage/ServerCard/ServerForm/AddServerForm/EditServerForm（复制、竞态守卫、OAuth 子字段回传、`/api/` 端点 chip）；②GroupCard（OpenAPI 端点 + 客户端配置对话框；可见性/管理权 UI 未启用）；③SettingsPage/SettingsContext（OAuth clientTtl + Smart Routing 面板，Tauri 下均隐藏）；④logService（SSE 退避，桌面端 Tauri 不启用 EventSource）；⑤Rust `http_server.rs`（REST disabled-tool 安全 gate）、`on_demand.rs`（长调用保活）、`commands/servers.rs`（npx scoped 缓存清理）；⑥locales 四语言 +82 键（credentials/clientConfig/smartRoutingIndex/groups.legacyVisibility 等）。
- **结果**：相比上一版本新增服务器复制、按客户端预设复制配置、OpenAPI 端点展示；修复禁用工具可经 REST 调用的安全漏洞、按需服务长调用被误关、npx 重装波及其他服务器缓存、日志流退避失效、编辑服务器竞态。**用户需重启应用**生效（Rust 侧三处修复随二进制更新）。
- **发布**：版本 `1.0.39001`，changelog `doc/upgrade/1.0.39001.md`。

#### 2026-09-08：同步 `980ab4a` -> `6b1fdb7`（8 个 commit，跨 v1.0.35 release + 3 个未发布提交）

origin 基线前进 8 个 commit（`6043a1f` #1133、`8f73e98` #1137、`dba3366` #1138、`4f40b1c` #1140、`6b1fdb7` #1141、`8030868`~`980ab4a` 之间无新增前端改动；v1.0.35 tag 指向本次范围内），本轮为跨越 v1.0.34/v1.0.35 两个 origin release 的基线同步。

`cd mcphub-origin && git --no-pager log --oneline 980ab4a..6b1fdb7` 共 8 个 commit；`git diff --stat 980ab4a..6b1fdb7 -- frontend/ locales/` 涉及 `SettingsContext.tsx`/`SettingsPage.tsx`/`configService.ts`/`index.css` + 4 个 locales。

**已同步到 desktop（前端 / locales）**

| 来源 commit | 说明 | desktop 应用方式 |
| ----------- | ---- | ---------------- |
| `6043a1f` #1133 | refactor: Smart Routing 配置字段 provider-neutral 更名 | 前端完整镜像：`SmartRoutingConfig`/temp state/表单 UI 中 `openaiApiBaseUrl`→`llmProviderBaseUrl`、`openaiApiKey`→`llmProviderApiKey`、`openaiApiEmbeddingModel`→`embeddingModel`（SettingsContext / SettingsPage / configService 三个桌面自定义文件手动合并，改动区域与桌面差异无重叠）；4 个 locales 键值对同步更名（zh 另带 `llmProviderApiKeyDescription`，en/fr/tr origin 无该键按 origin 为准）。Smart Routing 区块桌面 Tauri 下隐藏但代码保留对齐（既有策略）。 |
| `6b1fdb7` #1141 | fix: `.btn-primary` 边框改透明（亮/暗两处） | `index.css` 直接 patch 应用（该区域与桌面 `.hub-icon-btn:disabled` 等差异无重叠）。 |
| `980ab4a` #1131 确认 | MRL 透传开关（上一轮已同步） | 本轮 patch 重放时产生重复字段（桌面端已预合入 `embeddingDimensionsApiPassthrough`），去重后 tsc 与基线逐条一致——确认上一轮同步完整。 |

**已镜像到 desktop（Rust 后端）**：无。逐项评估：

| 来源 commit | 说明 | 处理决策 | 原因分析 |
| ----------- | ---- | -------- | -------- |
| `8f73e98` #1137 | fix: reject multipart content type line breaks | **无需镜像** | 修复对象 origin multipart 请求头解析；桌面端 Rust 无 multipart 处理链路（`grep multipart` 无命中）。 |
| `dba3366` #1138 | fix: serialize OpenAPI query arrays correctly | **无需镜像** | 修复对象 origin 自有 OpenAPI 请求构建（query array 序列化）；桌面端 OpenAPI 传输由 `rmcp-openapi` 库内部处理，无自有序列化落点（同 2026-09-06 #1130 判定）。 |
| `4f40b1c` #1140 | refactor: release-notes skill 更名 polish-release | **不同步** | 改动在 origin 仓库 `.claude`/skill 元数据，桌面端有自己的 skill 体系。 |
| `6043a1f` #1133 后端 | `smartRouting.ts` 配置键校验更名 | **无需镜像** | 桌面端 Smart Routing 未实现；`config_service::update` JSON 深合并对任意键名透传，前端发什么存什么，无需 Rust 改动。 |

其余依赖类 commit 按同步策略不同步（Node 依赖）。

**同步操作**：子模块指针 `980ab4a -> 6b1fdb7`；版本号 `1.0.34003 -> 1.0.35001`（基线跟随 origin 最新 tag v1.0.35，34 → 35，四源 + Cargo.lock）；changelog `doc/upgrade/1.0.35001.md` 单文件。

**同步后验证**：`cd frontend && npm run build` 通过；`npx tsc --noEmit` 24 错误 = 基线 24（经 `git stash` 对照 HEAD 逐条 diff，零新增；合并中产生的 3 处 `embeddingDimensionsApiPassthrough` 重复字段已去重）；locales JSON 四文件 `json.load` 校验通过，桌面端自定义键（runtime*/rag 全选/技能全选）完整；本轮无 Rust 源码改动，跳过 `cargo check`。

**影响功能点与结果**：

- **影响功能点**：①Smart Routing 表单配置键更名（桌面端 Tauri 运行时该区块隐藏，仅 web dev 模式可见，行为无变化）；②web 模式 `.btn-primary` 边框视觉微调；③locales settings 段 6 键更名 + zh 新增 1 键描述。
- **结果**：桌面端运行行为不变（Smart Routing 未实现、隐藏区块代码保持与 origin 对齐）；存量配置中的旧键名经 JSON 深合并 round-trip 保留不丢。用户无需任何操作。
- **发布**：版本 `1.0.35001`，changelog `doc/upgrade/1.0.35001.md`。

#### 2026-09-07（第二轮）：同步 `a67165e` -> `980ab4a`（1 个 commit，embedding dimensions MRL 修复）

`cd mcphub-origin && git --no-pager log --oneline a67165e..980ab4a` 共 1 个 commit：`980ab4a` #1131/#1132（fix: only forward embedding dimensions to MRL-capable models）。`git diff --stat a67165e..980ab4a -- frontend/ locales/` 涉及 `SettingsContext.tsx`/`SettingsPage.tsx`/`configService.ts` + 4 个 locales。

**已同步到 desktop（前端 / locales）**

| 来源 commit | 说明 | desktop 应用方式 |
| ----------- | ---- | ---------------- |
| `980ab4a` #1131/#1132 | fix: 嵌入维度参数仅对支持 MRL（Matryoshka，可配置输出维度）的模型转发 | 前端部分完整镜像：①`SettingsContext.tsx` `SmartRoutingConfig` 加 `embeddingDimensionsApiPassthrough: boolean`（类型 + 默认 `false` + 读取映射 `?? false`）；②`configService.ts` `SystemConfig.smartRouting` 加 `embeddingDimensionsApiPassthrough?: boolean`；③`SettingsPage.tsx` temp state 类型/默认值/初始化各加一字段 + embeddingDimensions 输入框下方新增「向 API 转发 dimensions 参数（MRL 透传）」Switch（`disabled={loading \|\| !smartRoutingConfig.enabled}`，`updateSmartRoutingConfig` 保存，与 origin 逐行一致）；④4 个 locales 各加 2 键（`embeddingDimensionsApiPassthrough` + `...Description`），与 origin 值逐语言核对一致。三个前端文件均为桌面端自定义文件，手动合并保留既有差异（本次改动区域与桌面差异无重叠，纯增量）。 |

**已镜像到 desktop（Rust 后端）**：无。逐项评估：

| 来源 commit | 后端部分 | 处理决策 | 原因分析 |
| ----------- | ---- | -------- | -------- |
| `980ab4a` #1131/#1132 | `vectorSearchService.ts` 的 `supportsDimensionsParameter`/`shouldForwardDimensionsToApi`（白名单 text-embedding-3/gemini-embedding + 强制透传开关）+ `smartRouting.ts` 配置键校验 + `serverController.ts` 入参校验 | **无需镜像** | 后端改动全部在 Smart Routing 向量检索链路——桌面端 Smart Routing 未实现（§7 待办），Rust 侧无向量检索/维度转发落点（与 2026-09-01 #1113 轮同判）。新配置键 `embeddingDimensionsApiPassthrough` 经 `config_service::update` JSON 深合并自动 round-trip 持久化，无需 Rust 改动。待将来实现 Smart Routing 时一并落地 MRL 白名单转发逻辑。 |

**同步操作**：子模块指针 `a67165e -> 980ab4a`；版本号 `1.0.34001 -> 1.0.34002`（tauri.conf.json / Cargo.toml / 根 package.json / frontend package.json + Cargo.lock）；changelog **最终合并为 `doc/upgrade/1.0.34002.md` 单文件**（`1.0.34001.md` 的 FTS 功能内容并入，`1.0.34001.md` 已删——覆盖 2026-09-06 轮「合并为 1.0.34001.md」的记录）。

**同步后验证**：`cd frontend && npm run build` 通过；`npx tsc --noEmit` 24 错误 = 基线 24（零新增）；locales JSON 四文件 `json.load` 校验通过，2 个新键与 origin 逐语言一致；本轮无 Rust 源码改动，跳过 `cargo check`。

**影响功能点与结果**：

- **影响功能点**：①设置页 Smart Routing 区块（web dev 模式可见，桌面端 Tauri 运行时隐藏）：embeddingDimensions 输入框下新增 MRL 透传开关；②`SettingsContext`/`configService` 新增 `embeddingDimensionsApiPassthrough` 配置字段（类型 + 默认 false + 读取/保存链路）；③四语言 locales 各 +2 键；Rust/DB/迁移零改动。
- **结果**：与上一版本（1.0.34002 前身 1.0.34001）相比，桌面端运行行为完全一致（Smart Routing 未实现，开关仅存在于 web dev 模式 UI）；新配置键可透传存储，为将来实现 Smart Routing 预留。用户无需任何操作。
- **发布**：版本 `1.0.34002`，changelog `doc/upgrade/1.0.34002.md`（与 1.0.34001 的 FTS 功能内容合并为单文件，34001.md 已删）。

#### 2026-09-07：同步 `8030868` -> `a67165e`（1 个 commit，纯文档）

`cd mcphub-origin && git --no-pager log --oneline 8030868..a67165e` 共 1 个 commit：`a67165e` #1136（docs: prefer gh CLI for GitHub operations in agent guides，改动仅 `AGENTS.md` + `docs/agents/git-and-contribution.md`）。`git diff --stat 8030868..a67165e -- frontend/ locales/ src/` **为空**——本轮 origin 无任何前端/locales/Node 后端代码改动。

**已同步到 desktop（前端 / locales）**：无。**已镜像到 desktop（Rust 后端）**：无。docs/ 按同步策略不同步；桌面端有自己的 AGENTS.md 与 agent guide 体系，无落点。

**同步操作**：仅子模块指针 `8030868 -> a67165e`。版本号维持 `1.0.34001` 不变（本轮零代码改动，无发布内容；origin 最新 tag 仍为 v1.0.34，下一轮有代码改动时基线仍是 34、序号从 002 递增）。

**同步后验证**：本轮无任何源码改动，跳过 build/cargo check。

**影响功能点与结果**：无。本轮 origin 仅 1 个纯文档 commit（agent guides 建议 gh CLI），桌面端无任何前端/Rust/locales/DB 落点，所有功能行为与上一版本（1.0.34001）完全一致，用户无需任何操作。

#### 2026-09-06：同步 `40e7c74` -> `8030868`（3 个 commit，全为 Node 后端修复）+ 补登未记录段

> ⚠️ **本轮补登**：上轮基线（2026-09-01，`0f59780`）之后，`38d5691 feat: 支持扫描子文件夹` 已把子模块指针无记录推进到 `40e7c74`（该段含 5 个 commit：`5fe212c` typeorm bump = v1.0.34 tag 点、`f03eb10` #1124、`40e7c74` #1125 等）。已提交部分均无 frontend/locales/Rust 落点：`f03eb10` #1124（commit message 提及 mcpService.ts，**实际 diff 仅 .agents/.claude 元数据文件，无代码改动**）、`40e7c74` #1125（纯文档）。本次基线顺带补齐。

本轮 `cd mcphub-origin && git --no-pager log --oneline 40e7c74..HEAD` 共 3 个 commit（`ffcc5cd` #1095、`b941d4d` #1128、`8030868` #1130）；`git diff --stat 40e7c74..8030868 -- frontend/ locales/` **为空——本轮 origin 无任何 frontend/locales 改动**，桌面端前端零改动。

**已镜像到 desktop（Rust 后端）**：无。逐项评估：

| 来源 commit | 说明 | 处理决策 | 原因分析 |
| ----------- | ---- | -------- | -------- |
| `ffcc5cd` #1095 | fix: persist startOnDemand/idleTimeoutMs when running in database mode | **无需镜像** | origin 的 DB-backed `ServerDaoDbImpl` 无两字段专属列，保存时白名单列集静默丢弃 `startOnDemand`/`idleTimeoutMs`，修复靠塞进 schema-less `options` JSON blob 兜底。桌面端 `servers` 表已有专属列（migrate_v17），`server_service::create/update` INSERT/UPDATE 均绑定 `start_on_demand`/`idle_timeout_ms`，`map_row` 回读——不存在该 bug。 |
| `b941d4d` #1128 | fix: reconnect upstream OAuth servers after reauthorization | **无需镜像** | 修复对象是 origin `oauthCallbackController.ts`（OAuth 重授权回调后重连上游 server）。桌面端无 OAuth 回调控制器（`grep oauthCallback` 无命中），OAuth2 仅静态配置透传存储，无重授权回调解链路。 |
| `8030868` #1130 | fix: decode multipart binary file arrays | **无需镜像** | 修复对象是 origin 自有 `openApiRequestBody.ts` 的 `buildMultipartParts`（`type: 'array'` 且 items 为 binary 时 `isBinaryField` 误判）。桌面端 OpenAPI 传输由 `rmcp-openapi` 库内部处理 HTTP（无自有 multipart 构建器，`grep multipart` 无命中），无落点。 |

其余依赖类 commit（`5fe212c` typeorm 0.3.31→1.1.0、`1d1b800`/`ea1c6e8`/`c93a40f`/`94fcb00` dev deps）按同步策略不同步（Node 依赖）。

**同步操作**：子模块指针 `40e7c74 -> 8030868`；版本号 `1.0.33102 -> 1.0.34001`（基线跟随 origin 最新 tag v1.0.34，四源 + Cargo.lock）；changelog **合并为 `doc/upgrade/1.0.34001.md` 单文件**（并入 `1.0.33102.md` 的 FTS 功能内容，原 `1.0.33102.md` 已删）。

**同步后验证**：`cd frontend && npm run build` 通过；locales JSON 四文件 `json.load` 校验通过（本轮无改动）；`cargo check` 通过（仅 Cargo.toml/Cargo.lock 版本号变更，无 Rust 源码改动）。

#### 2026-09-01（第二轮）：同步 `7ed1637` -> `0f59780`（7 个 commit，含上游安全修复）

origin 基线前进 7 个 commit（`9400d32` #1115、`e657df9`/`e677000`/`20db67e` 三个 "Merge commit from fork" 安全修复、`67738d4` #1116、`180b251` #1117、`0f59780` #1118），均在 `v1.0.33` tag（`41d34be`）之后未发布。与第一轮合并为一次发布，最终版本号 `1.0.33002`（中间版本 1.0.33001 未发布）。

`cd mcphub-origin && git --no-pager log --oneline 7ed1637..0f59780` 共 7 个 commit；`git diff --stat 7ed1637..0f59780 -- frontend/ locales/` 仅涉及 4 个 locales（各 2 行，全部来自 #1115；其余 commit 无 frontend/locales 改动）。

**已同步到 desktop（前端 / locales）**

| 来源 commit | 说明 | desktop 应用方式 |
| ----------- | ---- | ---------------- |
| `9400d32` #1115 | feat: MCPHub 定位措辞更新（"self-hosted MCP gateway"） | `locales/{en,zh,fr,tr}.json` 的 `auth.slogan` + `auth.subtitle` 两键更新为 origin 新文案（登录页标语/描述），与 origin 值逐语言核对一致。桌面端 LoginPage 自定义（admin 默认填充、密码提示、Logo）不受影响。 |

**已镜像到 desktop（Rust 后端）**

无。见下方「未同步」中对安全修复的逐项评估。

**未同步（经评估无需 / 无法同步）**

| 来源 commit | 说明 | 处理决策 | 原因分析 |
| ----------- | ---- | -------- | -------- |
| `e657df9` Merge from fork | 安全修复①：activity/log 控制器端点加 `requireAdmin`；② OAuth client 注册/discovery 出站 fetch 改走 `createRedirectValidatingFetch`（SSRF 校验） | **无需镜像** | ①桌面端内置 HTTP 服务器（`http_server.rs`）只暴露 `/health`、`/.well-known/oauth-protected-resource`、`/servers`、`/rest/*`、`/mcp*` 路由，**没有** origin 的 `/logs`、`/activities` 管理 REST 路由——桌面端日志/活动经 Tauri IPC 命令（`get_logs` 等）访问，天然不出内置 HTTP 面；`/servers` 列表本就要求 Bearer Key。②桌面端 Rust 无 OAuth client 动态注册/discovery 出站链路（`grep well-known/discovery/register` 无 OAuth 出站点；OAuth2 仅作静态配置存储透传），无镜像落点。 |
| `67738d4` #1116 | fix: SSRF redirect 校验传播 DNS lookup + 校验**初始** URL（此前只校验 redirect 目标） | **无需镜像** | 修复对象是 origin 的 `cimdClientService`（CIMD client-id 文档拉取）与 `createRedirectValidatingFetch` 初始 URL 未校验漏洞。桌面端无 CIMD 集成；桌面端 reqwest 出站点（registry/cloud/runtime/progress/rag/sse/http/openapi transport）均直连用户显式配置或固定官方 URL，无「拉取远端文档再跟 redirect」的可注入链路。注意：桌面端 reqwest 默认 `Policy::default()`（跟 10 跳），若未来引入「从不可信源 URL 拉取并跟随 redirect」的功能，需同步评估 SSRF 防护。 |
| `e677000` / `20db67e` Merge from fork | 树内容为空（与前一提交 tree 一致的 GH 安全通告空合并） | **跳过** | 无代码改动。 |
| `180b251` #1117 / `0f59780` #1118 | 纯文档（CLAUDE.md 链接 AGENTS.md、docs/agents/ 体系重组） | **跳过** | docs/ 按同步策略不同步；桌面端有自己的 AGENTS.md。 |

**同步后验证**：`cd frontend && npm run build` 通过；locales JSON 四文件 `json.load` 校验通过，`auth.slogan`/`auth.subtitle` 与 origin 逐语言一致。本轮无前端 TS 改动（仅 locales），无 Rust 源码改动。本轮与第一轮合并为一次发布：版本号四源直接落到 `1.0.33002`（`tauri.conf.json`/`Cargo.toml`/根 `package.json`/`frontend/package.json` + `Cargo.lock`，中间版本 1.0.33001 未发布），changelog 与第一轮合并为 `doc/upgrade/1.0.33002.md` 单文件。子模块指针 `7ed1637 -> 0f59780`（main 分支）。

#### 2026-09-01（第一轮）：同步 `41d34be` -> `7ed1637`（2 个 commit，v1.0.33 之后未发布提交）

origin 基线前进 2 个 commit（`8f5539a` #1112 + `7ed1637` #1113），均为 `v1.0.33` tag（`41d34be`，即上一轮基线终点）之后的未发布提交。与同日第二轮合并为一次发布，本条目记录的改动最终随 `1.0.33002` 发布。

`cd mcphub-origin && git --no-pager log --oneline 41d34be..7ed1637` 共 2 个 commit；`git diff --stat 41d34be..7ed1637 -- frontend/ locales/` 涉及 `SettingsContext.tsx`/`SettingsPage.tsx`/`configService.ts` + 4 个 locales（全部来自 #1113；#1112 无 frontend/locales 改动）。

**已同步到 desktop（前端 / locales）**

| 来源 commit | 说明 | desktop 应用方式 |
| ----------- | ---- | ---------------- |
| `7ed1637` #1113 | feat(embeddings): provider presets + configurable dimensions | 前端部分完整镜像：①`SettingsContext.tsx` `SmartRoutingConfig` 加 `embeddingDimensions?: number`（类型 + 默认值 + 读取映射）；②`configService.ts` `SystemConfig.smartRouting` 加 `embeddingDimensions?: number`；③`SettingsPage.tsx` 新增 `EMBEDDING_PROVIDER_PRESETS`（openai/openrouter/siliconflow/gemini/custom 五预设）+ `getEmbeddingProviderPresetId()`（按 baseUrl+model 反推预设）+ `parseEmbeddingDimensionsForUpdate()`（空串→null 清除 / 正整数→number / 非法或未变→undefined）+ `embeddingProviderPreset`/`embeddingDimensions` temp state 字段 + preset 下拉（改 baseUrl/model 手动输入时自动回落 custom）+ dimensions 数字输入框（provider 分支后、basePacingDelay 前渲染，openai/azure 共用）+ 两处保存路径（`handleSmartRoutingConfigChange` enable 批量保存 + `handleSaveSmartRoutingConfig`）都解析 `embeddingDimensions` 进 updates；④4 个 locales 各加 10 键（`embeddingProviderPreset*` 7 键 + `embeddingDimensions*` 3 键），与 origin 值逐语言核对一致。注：Smart Routing 区块在桌面端 Tauri 下隐藏（`!isTauri()`）但代码保留（web dev 模式可用），历史上一直跟进该区块的表单逻辑（桌面 vs origin@41d34be 该区块 diff 仅剩桌面自定义项），故继续镜像保持对齐。 |

**已镜像到 desktop（Rust 后端）**

无。#1113 后端部分（`vectorSearchService.ts` 的 `normalizeEmbedding`/`getDimensionsForModel(model, configuredDimensions)`/payload 带 `dimensions`、`serverController.ts` 的 `embeddingDimensions` 校验入库、配置变更触发向量索引重建）属 Smart Routing 功能链路——桌面端 Smart Routing 未实现（§7 待办），无对应 Rust 落点。且桌面端 `config_service::update` 是 JSON deep-merge 透传，前端发的 `smartRouting.embeddingDimensions` 原样 round-trip 持久化，无需 Rust 改动。

**未同步（经评估无需 / 无法同步）**

| 来源 commit | 说明 | 处理决策 | 原因分析 |
| ----------- | ---- | -------- | -------- |
| `8f5539a` #1112 | fix: align smart routing env docs with code | **不同步** | 改动全部在 `docs/`（文档）+ `src/utils/smartRouting.ts`（环境变量优先级注释与别名保留：`SMART_ROUTING_ENABLED` 优先、`ENABLE_SMART_ROUTING` 兼容、`OPENAI_API_EMBEDDING_MODEL` 加入 fallback 链）+ 测试。桌面端无 Smart Routing，Rust 侧无环境变量读取链路，无镜像落点；docs/ 按策略不同步。 |
| `7ed1637` #1113 后端部分 | `vectorSearchService.ts`/`serverController.ts`/`smartRouting.ts` + 测试 | **不同步** | 见上「已镜像」说明——Smart Routing 未实现，后端无落点；待将来实现 Smart Routing 时一并评估（含向量归一化 `normalizeEmbedding` 对自定义维度跨提供商一致性的处理）。 |

**同步后验证**：`cd frontend && npm run build` 通过（1.52s）；`npx tsc --noEmit` 错误数 24 = 基线 24（`git worktree` 对照 HEAD 验证，全部为存量错误，本轮零新增，Settings 相关 11 个错误与基线逐条一致）；本轮无 Rust 源码改动，跳过 `cargo check`。locales JSON 四文件 `json.load` 校验通过，19 个 embedding/preset 相关键与 origin 逐语言 diff 一致。本条目与同日第二轮合并为一次发布 `1.0.33002`，changelog 合并为 `doc/upgrade/1.0.33002.md` 单文件（不再有独立的 1.0.33001 changelog）。子模块指针 `41d34be -> 7ed1637`（随后被第二轮推进到 `0f59780`，main 分支）。

> 历史同步记录（2026-07-24 ~ 2026-08-25，共 11 轮）已精简，仅保留最近基线同步条目（2026-09-01 同日两轮各保留一条，发布合并为 1.0.33002）。如需查阅历史同步的逐 commit 评估（已同步/已镜像/未同步的决策与原因），见 `git log` 对应提交的 AGENTS.md 版本。


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
- [X]  启动更新检查（根级 `UpdateCheckProvider`：应用启动即检查、不依赖登录态；检测到新版本自动弹「关于」+ 侧边栏红点；移除「忽略此版本」；详见 3.4.7）
- [X]  更新检查日志（`log_event` Tauri command 写入 `app_log`，前端 `[update]` 全流程日志：检查/新版本/已最新/失败/安装；日志页按来源 `update` 可过滤；详见 3.4.7）
- [X]  release notes Markdown 渲染（`Markdown` 组件 react-markdown+remark-gfm；notes 即 `doc/upgrade/{version}.md` 全文；详见 3.4.7）
- [X]  安装进度可视化（下载百分比进度条 + 已下载/总字节 + 实时下载速度 EMA；安装阶段 indeterminate spinner；按钮文案随阶段切换；详见 3.4.8）
- [X]  版本号四源同步（`tauri.conf.json` / `Cargo.toml` / 根 `package.json` / `frontend/package.json`；当前 1.0.33002；详见 3.4.7）
- [X]  stdio 服务器按需启动（startOnDemand：跳过启动连接、首次工具调用懒建进程、空闲超时自动关闭、缓存工具保留；详见 3.8）
- [X]  stdio 连接错误包含上游 stderr（`stderr_tail` 滚动缓存拼接进 handshake 失败 error；详见 3.9）
- [X]  编辑服务器避免无谓重连 + proxy 持久化（镜像上游 #1055：`update_server` 比对连接相关字段，仅访问/元数据变更时保留实时连接；`ServerConfig.proxy` + DB v20 持久化使前端 round-trip 生效；详见 3.11）
- [X]  SQLite 全文索引（FTS5 + 中英/拼音分词，charabia 全语言）：DB v24 七张 FTS5 表 + 迁移前 `mcphub.db.bak` 备份（VACUUM INTO）+ `fts_service` 模块（分词/查询路由/同事务同步/对账回填/app_log 存量回填）+ servers/groups/rag_docs/skills/prompts/resources 写路径全同步 + 5 个搜索函数升级（rank 序 + LIKE 降级）；详见 3.15
- [X]  活动日志筛选优化：移除「用户」筛选；server/tool/group/keyName 四条件改可搜索分页下拉（`SearchableSelect` 组件 + `get_activity_filter_options` 命令 + tauriClient 透传 groupName/keyName）；详见 3.15.5
- [X]  应用日志全文检索：`get_logs` 支持可选 `search`（FTS5 匹配 message + LIKE 降级）；日志清理与 fts_app_log 同事务联动；详见 3.15.4
- [X]  删除 `[http-server-watch]` 周期性 heartbeat 调试日志（此前每 30s 刷一条淹没正常日志，保留一次性事件日志）

### 待办

- [ ]  Smart Routing（智能路由）
- [ ]  OAuth Server
- [ ]  Better Auth 集成
- [ ]  Tool Result Compression
- [ ]  MCPB/DXT 文件安装
- [ ]  Templates（配置模板）
- [ ]  CI/CD 打包配置

### 已知问题
（暂无）
