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

四个版本源须保持一致（当前 `1.0.30001`）：

- `src-tauri/tauri.conf.json`（应用版本，也是 `import.meta.env.PACKAGE_VERSION` 的来源——`vite.config.ts` 从此注入）
- `src-tauri/Cargo.toml`
- `package.json`（根）
- `frontend/package.json`

`doc/upgrade/{version}.md` 存在对应版本时，CI 会将其全文作为 `latest.json` 的 `notes` 发布（见 3.4.3/3.4.4）。

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
```

### 4.3 最近同步基线

每次基线同步后，必须同步原项目的版本号，即：/Users/jphoebe/opt/code/IdeaProjects/github/mcphub-desktop/mcphub-origin/locales/zh.json文件中{{version}}
桌面的版本号规则为：{{version}}xxx, xxx代表当前桌面端的版本号，从001开始递增
| 项                             | 值                      |
| ------------------------------ | ----------------------- |
| **当前已同步到 origin commit** | `bfd153c` (origin/main，= `v1.0.30` tag) |
| **对应 origin tag**            | `v1.0.30`（最新 tag，指向 `bfd153c`） |
| **桌面端版本号**               | `1.0.30001` |
| **同步执行日期**               | 2026-08-20              |

> 下次同步时，使用 `bfd153c` 作为新的基线 SHA 起点（命令：`cd mcphub-origin && git --no-pager log --oneline bfd153c..HEAD`）。
>
> ⚠️ **文档补齐说明**：上一次同步（2026-07-27，desktop commit `f417a12 feat: 基线同步`）已把子模块指针前进到 `a99c382`（= `v1.0.25` tag）、桌面端版本号提到 `1.0.25001`，但当时未更新本节「最近同步基线」与 §4.4「最近同步记录」。本次同步（2026-07-30）顺带补齐：把基线文档从陈旧的 `cb44e22`/`1.0.24003` 修正为实际状态 `a99c382`→`29c0704`/`1.0.26001`，并在 §4.4 补登 `a99c382 → 29c0704` 的同步条目（`a99c382..29c0704` 之间 origin 无 frontend/locales 改动，详见该条目）。

### 4.4 最近同步记录

#### 2026-08-20：同步 `0249c73` -> `bfd153c`（2 个 commit，跨入 v1.0.30）

origin 从 `v1.0.29` 跨入 `v1.0.30`（`bfd153c` 精确指向 `v1.0.30` tag）；桌面端版本 `1.0.29001` -> `1.0.30001`。

`cd mcphub-origin && git --no-pager log --oneline 0249c73..bfd153c` 共 2 个 commit（`244847e` #1054 + `bfd153c` #1055）；`git diff --stat 0249c73..bfd153c -- frontend/ locales/` 涉及 `ServerForm.tsx`（#1054+#1055）、`Button.tsx`/`EmbeddingSyncContext.tsx`/`serverFormPayload.ts`/`types/index.ts`/`serverVisibility.ts`/`tsconfig.json`（#1055）+ 4 个 locales（#1054），以及 Node 后端 `serverController.ts`/`mcpService.ts`/`dataService.ts`/`ServerDao*`/`ServerRepository.ts`/`smartRoutingService.ts`/`serverConfigPersistence.ts`/`migration.ts`/`routes/index.ts` + `db/entities/Server.ts`（#1054/#1055，评估镜像）。

**已同步到 desktop（前端 / locales）**

| 来源 commit | 说明 | desktop 应用方式 |
| ----------- | ---- | ---------------- |
| `244847e` | feat: Implement group visibility and shared user functionality for servers (#1054) | 前端类型对齐（`types/index.ts`：`ServerConfig`/`ServerFormData`/`Server` 加 `sharedWithUsers?: string[]`，visibility 注释从「group 预留」改为「group = 指定用户共享」）。locales en/zh/fr/tr 各补齐 `visibility*` + `share*` 共 14 键（zh/en 更新 8 个旧键 + 新增 6 键；fr/tr 新增全 14 键），`group` 语义从「预留/分组」改为「共享给指定用户」。**但 `ServerForm` 的 visibility 选择器块、`sharedWithUsers` 候选 UI、`apiGet('/servers/:name/share-candidates')` 调用均不镜像**——桌面端隐藏可见性（所有 server 默认公开）、单用户免登录、Rust 无 share-candidates 端点，故前端只对齐类型/翻译（dormant），UI 不引入。 |
| `bfd153c` | fix: avoid unnecessary runtime reloads when editing a server (#1055) | 前端 3 个自定义文件 + 3 个基线一致文件手动合并：① `ServerForm.tsx`（⚠️ 自定义文件手动合并，保留 3 处桌面差异：`getInitialServerType` 跳过 `builtin`、visibility 默认 `public`、删除 visibility 选择器块；同步上游 #1055 的 Basic Info 3 列网格（name 1 格 / description 2 格）、OAuth2↔OpenID Connect 配置块顺序调整（OpenID Connect 在前）、`+` 按钮统一改 `hub-btn primary`、passthrough headers + OAuth 配置从 openapi/sse 分支移入 Advanced 分区按 `serverType` 路由（openapi 透传 `openapi.passthroughHeaders`，其余用 `formData.passthroughHeaders`）；**不镜像 cookieSession UI**——桌面 `rmcp-openapi` 传输无 cookie 持久化，toggle 为 no-op，注释隐藏）；② `serverFormPayload.ts`（⚠️ 自定义文件手动合并：`buildOptions` timeout 改为始终回显 `typeof options.timeout === 'number'` 而非 `&& !== 60000`（避免触发后端连接变更误判）；`config.proxy = formData.proxy` round-trip；保留桌面差异——不发送 `visibility`/`sharedWithUsers`，perSessionClient/startOnDemand 按 serverType 分支）；③ `types/index.ts`（⚠️ 自定义文件：`ServerConfig` 加 `startOnDemand`/`idleTimeoutMs` 注释更新 + `sharedWithUsers`；`ServerFormData` 加 `sharedWithUsers` + `proxy?: ProxychainsConfig`）；④ `Button.tsx`（与基线一致，直接复制新版本：`onClick` 类型放宽为 `void | Promise<void>`，handleClick 检测 Promise 自动 setInternalLoading）；⑤ `EmbeddingSyncContext.tsx`（与基线一致，直接复制新版本：`const progress = data.progress` 收窄闭包持有，避免 TS narrowing 在 setTimeout 内丢失）；⑥ `serverVisibility.ts`（与基线一致，直接复制：group fallback `Group` -> `Shared`、`getServerVisibilityOptions` defaultValue `Group` -> `Shared`）。 |

**已镜像到 desktop（Rust 后端）**

| 来源 commit | 说明 | desktop 镜像方式 |
| ----------- | ---- | ---------------- |
| `bfd153c` | fix: avoid unnecessary runtime reloads when editing a server (#1055) | `commands/servers.rs::update_server` 重写为镜像上游 `hasConnectionRelevantChange`：先 `server_service::get_by_name(&name)` 取既有配置 -> 比对连接相关子集（序列化整个 `ServerConfig` 为 JSON 后剔除 `id`/`name`/`description` 等访问/元数据字段 + 默认超时 60000 视为未设置 + `strip_nulls` 归一）-> 无连接相关变更时仅持久化 + 保留实时连接（`pool::connect_server` 不触发），有变更或 enabled 改变才 `disconnect_server` + 后台 spawn 重连。`starting` 状态按 `connection_relevant_changed` 派生。rename 仍由开头 `disconnect_server(&name)`（旧名）天然覆盖（上游 #1055 另需 `closeServer(name)` 是因其 `addOrUpdateServer` 按新名 close 才漏，桌面无此问题）。 |
| `bfd153c`（proxy round-trip 部分） | `ServerConfig` 新增 `proxy: Option<ProxychainsConfig>` 字段（`models/server.rs` 新增 `ProxychainsConfig` 结构体，camelCase 序列化，`type` rename 为 `proxy_type`）；`db/migration.rs` `migrate_v20`（`add_column_if_missing servers proxy TEXT`，幂等）+ `TARGET_VERSION` 19 -> 20 + `apply_migration` 加 `20 => migrate_v20`；`services/server_service.rs` 3 个 SELECT 列清单 + INSERT/UPDATE bind + `map_row` 读取 `proxy`；`services/settings_import.rs` + `rag/service.rs` builtin 的 `ServerConfig` 字面量补 `proxy: None`。使前端 `config.proxy` round-trip 真正生效（此前 serde 默默丢弃该字段，编辑任何字段都会丢 proxy 触发重连）。 |

**未同步（经评估无需 / 无法同步）**

| 来源 commit | 说明 | 处理决策 | 原因分析 |
| ----------- | ---- | -------- | -------- |
| `244847e`（后端部分） | `serverController.ts`（`getServerShareCandidates` 端点 + `isAccessOnlyServerUpdate`）/`dataService.ts`（group 可见性过滤）/`ServerDao*`/`ServerRepository.ts`（`sharedWithUsers` 列与查询）/`mcpService.ts`（`getVisibleServerInfos` 按用户过滤）/`smartRoutingService.ts`/`db/entities/Server.ts`（`sharedWithUsers` 列）/`migration.ts`/`routes/index.ts` | **不同步** | 桌面端 Rust 无 `visibility`/`owner`/`sharedWithUsers` 概念（`server_service.rs` 无这些列）、单用户免登录架构（`SessionState` 始终 admin/None）、前端已隐藏可见性 UI 且不发送 visibility/sharedWithUsers。整套按用户过滤可见性 + share-candidates 端点属多用户特性，桌面端无对应架构。前端类型字段（`sharedWithUsers`）与 14 个 locale 键已对齐（dormant），待将来实现多用户时落地。 |
| `bfd153c`（后端测试部分） | `tests/controllers/serverController.test.ts`（+298）/`tests/frontend/serverFormPayload.test.ts`/`serverLocales.test.ts`/`serverVisibility.test.ts` | **不同步** | 纯 Node/TS 测试；桌面端 Rust 无对应单测基建，#1055 的 Rust 镜像已通过 `cargo check` 验证编译。 |
| `bfd153c`（`tsconfig.json`） | 移除 `ignoreDeprecations: "6.0"`、空白调整 | **不同步** | 桌面端 `tsconfig.json` 在 2026-08-04 同步（§记忆 dep-upgrade）时已主动加 `baseUrl`/移除 `ignoreDeprecations`，与上游目标一致但已本地化，无改动需要。 |
| cookieSession（#1047 体系，`ServerForm.tsx` 上游块） | OpenAPI Set-Cookie 跨调用按下游 session 持久化 | **不同步** | 桌面端 `mcp/openapi_transport.rs` 经 `rmcp-openapi` crate，无 cookie 持久化注入点；2026-08-17 同步 #1047 时已决策「架构分歧无法镜像」。本次 `ServerForm` 合并保留 `cookieSession` 注释隐藏（toggle 无后端落点，为 no-op），不引入 UI。 |

**同步后验证**：`cd frontend && npm run build` 通过（1.61s）；`cd src-tauri && ORT_SKIP_DOWNLOAD=1 cargo check` 通过（asdf cargo 1.96.0，11m45s，无 error/warning）。`npx tsc --noEmit` 错误数 27 -> 21（新 `Button.tsx` async onClick + `EmbeddingSyncContext.tsx` narrowing 修复消解 6 个既有错误，无新增；`ServersPage.tsx:36` 的 `handleServerVisibilityChange` 死引用为同步前既有，ServerCard visibility UI 在桌面端不渲染故运行时无影响）。桌面端自定义文件经 `diff -w` 核对：`ServerForm.tsx` 与 origin 仅余 3 处桌面差异（getInitialServerType builtin 跳过 / visibility 默认 public / visibility 选择器块删除）+ 故意保留的 cookieSession/visibility UI 删除；`serverFormPayload.ts`/`types/index.ts` 保留桌面端 perSessionClient/startOnDemand 分支与不发送 visibility 的差异。版本号已同步：`package.json` / `frontend/package.json` / `src-tauri/tauri.conf.json` / `src-tauri/Cargo.toml` / `Cargo.lock` 均已 `1.0.29001` -> `1.0.30001`；changelog `doc/upgrade/1.0.29001.md` -> `1.0.30001.md`。如需对 Cargo.lock 做正式复核，用 asdf/rustup cargo 1.96.0 + `ORT_SKIP_DOWNLOAD=1` + 本地代理 127.0.0.1:7890（cargo 不在 PATH）。

---

#### 2026-08-18：同步 `9fb73dd` -> `0249c73`（2 个 commit）

origin 仍为 `v1.0.29`（`0249c73` = v1.0.29 tag 之后 2 个未发布提交，无新 tag）；桌面端版本号不变 `1.0.29001`（与 2026-08-17 同步共用同一待发布版本，上次同步的改动当时未提交，本次一并提供）。

`cd mcphub-origin && git --no-pager log --oneline 9fb73dd..0249c73` 共 2 个 commit（`eabde16` #1052 + `0249c73` #1053）；`git diff --stat 9fb73dd..0249c73 -- frontend/ locales/` 涉及 `SettingsContext.tsx`/`SettingsPage.tsx`/`configService.ts`（#1053）+ 4 个 locales。

**已同步到 desktop（前端 / locales）**

| 来源 commit | 说明 | desktop 应用方式 |
| ----------- | ---- | ---------------- |
| `0249c73` | feat(auth): make Better Auth base URL configurable in settings (#1053) | 前端 3 个自定义文件手动合并：`SettingsContext.tsx`（`BetterAuthConfig` 接口 + `getDefaultBetterAuthConfig` + `normalizeBetterAuthConfig` + `mergeBetterAuthConfig` 均加 `baseUrl`，默认 `''`）；`SettingsPage.tsx`（`tempBetterAuthConfig` state/初始值/useEffect 播种/`handleBetterAuthTextChange` key 联合类型/`handleSaveBetterAuthConfig` 差异检测与 `updates.baseUrl` 均加 `baseUrl`；UI 在「Enable Better Auth」开关卡与「Auth base path」卡片之间插入 Base URL 输入框，与上游位置一致）；`configService.ts`（顶层 `BetterAuthConfig` 接口加 `baseUrl?: string`，与上游改动点一致；`SystemConfig.auth.betterAuth` 嵌套内联接口上游未改，桌面端保持一致不改）。locales：en/zh/fr/tr 各加 `settings.betterAuthBaseUrl` + `betterAuthBaseUrlDescription` 2 键（用上游翻译，插在 `enableBetterAuthDescription` 与 `betterAuthBasePath` 之间，与上游顺序一致）。桌面端 Better Auth 区块在 Tauri 模式整段隐藏（`!isTauri()` 守卫保留），字段与翻译先行对齐，web 模式直接生效。 |

**已镜像到 desktop（Rust 后端）**

无。#1053 的后端改动（`src/betterAuth.ts`/`betterAuthConfig.ts`/`serverController.ts`）属 Node Better-Auth 社交登录体系，桌面端 Better Auth 集成在 §7 待办（未实现），Rust 侧无对应落点；`update_settings` 对未知 config 字段的透传由 `config_service::update` 深合并 JSON 兜底，前端发出的 `auth.betterAuth.baseUrl` 会被原样持久化、不丢失。

**未同步（经评估无需 / 无法同步）**

| 来源 commit | 说明 | 处理决策 | 原因分析 |
| ----------- | ---- | -------- | -------- |
| `0249c73`（后端部分） | `src/betterAuth.ts`/`betterAuthConfig.ts`/`serverController.ts` + 3 个 Node 测试文件（#1053 后端） | **不同步** | Node Better-Auth 运行时配置解析（`BETTER_AUTH_URL` 环境变量优先、base URL 拼 redirect URI）；桌面端无 Better Auth 服务（§7 待办），无对应架构；前端 `baseUrl` 字段经 `config_service::update` 深合并持久化，不丢字段。 |
| `eabde16` | chore: align license metadata with Apache-2.0 (#1052) | **不同步** | 仅改 origin `package.json` license 字段 + `docs/api-reference/openapi.json`；桌面端独立 npm 依赖图（4.1 策略 4），license 元数据非本仓库同步范围。 |

**同步后验证**：`cd frontend && npm run build` 通过；locales 4 文件均通过 `JSON.parse` 校验，runtime* 键未受影响；无 Rust 源码改动（`cargo check` 无需重跑，版本号维持 1.0.29001 与上次同步一致）。

---

#### 2026-08-17：同步 `a8ace62` -> `9fb73dd`（4 个 commit）

origin 从 `v1.0.28` 系列跨入 `v1.0.29`（`9fb73dd` 精确指向 `v1.0.29` tag）；桌面端版本 `1.0.28002` -> `1.0.29001`。

`cd mcphub-origin && git --no-pager log --oneline a8ace62..9fb73dd` 共 4 个 commit（`3723946` #1046 + `a1bb974` #1047 + `8bb8334` #1048 + `9fb73dd` #1049 docs）；`git diff --stat a8ace62..9fb73dd -- frontend/ locales/` 非空（#1047 触及 origin `frontend/` 与各 `locales/`）。

**已同步到 desktop（前端 / locales）**

无。桌面 `frontend/` 为自研独立前端，不复用 origin `frontend/`；origin #1047 的前端改动属上游 TS ServerForm/locales，桌面无对应文件，不直接搬运。

**已镜像到 desktop（Rust 后端）**

无。本次 4 个 origin commit 均未镜像到 Rust：#1046/#1047/#1048 为 TS MCP-SDK / axios 特定实现，桌面 OpenAPI 走 `rmcp-openapi` crate、MCP 传输层无 OAuth provider，架构分歧无可镜像落点（详见「未同步」）；#1049 为纯 README 文档。

**未同步（经评估无需 / 无法同步）**

| 来源 commit | 说明 | 处理决策 | 原因分析 |
| ----------- | ---- | -------- | -------- |
| `3723946` | fix(oauth): restore 401 auto-discovery for URL-only Streamable HTTP servers (#1046) | **不同步** | 上游为 TS MCP SDK 的 `createOAuthProvider`（`authProvider` 挂载 → SDK 拦截 401 挑战做 OAuth 发现）。桌面端 MCP 传输层（`mcp/http_transport.rs`、`sse_transport.rs`）无 OAuth provider 实现，`src-tauri/src` 全局无 oauth/cookie 相关代码，无可镜像落点。 |
| `a1bb974` | feat(openapi): persist Set-Cookie across calls per downstream session (#1047) | **不同步** | API 调用路径走 `rmcp-openapi` crate，cookie 持久化由该 crate 决定，桌面无法在外层注入按 sessionId 隔离的 CookieJar；spec 拉取路径用裸 `reqwest::Client::new()`，无登录态流程，不涉及 Set-Cookie 持久化。架构分歧，无法直接镜像。 |
| `8bb8334` | fix(openapi): authenticate spec document download with configured credentials (#1048) | **不同步** | 上游把 spec 下载改走带 `setupSecurity` 凭据 + `maxRedirects:0` + SSRF 校验的 `httpClient`，属 TS axios + 上游自有 SSRF/安全方案体系。桌面 `openapi_transport.rs::fetch_spec` 用裸 `reqwest::Client::new()` 只发 `config.headers`，确有同源隐患（spec URL 需鉴权时拉不到），但完整移植需引入 cookie/OAuth2/SSRF 一套（与上游 `utils/ssrf.ts`、`setupSecurity` 对应的桌面实现尚无），属 feature 级工作而非 fix 镜像。本轮不做，留作独立任务评估；如需缓解，最小补丁是让 `fetch_spec` 在拉 spec 时也注入 `OpenApiSecurity` 凭据（ApiKey cookie / OAuth2 token）。 |
| `9fb73dd` | docs: fix broken star history chart (#1049) | **不同步** | 纯 README 文档，对桌面构建无影响。 |

**同步后验证**：本次仅版本号 + 子模块指针 + 文档同步，无 Rust 源码改动。`src-tauri/Cargo.toml`/`Cargo.lock` 仅 `mcphub` 包 `version` 行 `1.0.28002`->`1.0.29001`（元数据，不影响编译）。版本号已同步：`package.json` / `frontend/package.json` / `src-tauri/tauri.conf.json` / `src-tauri/Cargo.toml` / `Cargo.lock` 均已 `1.0.28002` -> `1.0.29001`；changelog `doc/upgrade/1.0.28002.md` -> `1.0.29001.md`（内容回退为桌面端自有功能/修复，撤去先前误加的 #1046/#1047/#1048「镜像上游」条目——三者未实际镜像）。如需对 Cargo.lock 做正式 `cargo check` 复核，用 asdf/rustup cargo 1.96.0 + `ORT_SKIP_DOWNLOAD=1` 离线执行（详见本地构建记忆：本地代理 127.0.0.1:7890、cargo 不在 PATH）。

---

#### 2026-08-14：同步 `0e8fed0` -> `a8ace62`（2 个 commit）

origin 仍为 `v1.0.28`（`a8ace62` = `v1.0.28` tag 之后 4 个未发布提交，无新 tag）；桌面端版本 `1.0.28001` -> `1.0.28002`。

`cd mcphub-origin && git --no-pager log --oneline 0e8fed0..a8ace62` 共 2 个 commit（`de60851` #1042 + `a8ace62` merge）；`git diff --stat 0e8fed0..a8ace62 -- frontend/ locales/` 为空（无前端/locales 改动）。

**已同步到 desktop（前端 / locales）**

无。本次两 commit 均不触及 `frontend/` 或 `locales/`。

**已镜像到 desktop（Rust 后端）**

| 来源 commit | 说明 | desktop 镜像方式 |
| ----------- | ---- | ---------------- |
| `de60851` | fix: bound graceful shutdown for long-lived connections (#1042) | `services/http_server.rs::start()` 的 `axum::serve(...).with_graceful_shutdown(...)` 用 `tokio::time::timeout(SHUTDOWN_GRACE=10s, serve)` 包裹：宽限期内正常优雅关闭；超时则丢弃 serve future 强制中止残留连接（含永不结束的 SSE/Streamable HTTP 长连接），并打 warn 日志。对应 origin `closeHttpServer` 的 10s 宽限期 + `socket.destroy()` 强销毁语义。修复前长连接会令优雅关闭永久挂起（重启服务时旧 task 泄漏、「HTTP server stopped」日志不打印）。 |

**未同步（经评估无需 / 无法同步）**

| 来源 commit | 说明 | 处理决策 | 原因分析 |
| ----------- | ---- | -------- | -------- |
| `de60851`（测试部分） | `src/utils/serverShutdown.test.ts`（55 行 Node 单测） | **不同步** | 纯 Node 测试；桌面端 Rust 无对应单测基建，#1042 的 Rust 镜像已通过 `cargo check` 验证编译。 |
| `a8ace62` | Merge commit from fork | **不同步** | 纯合并提交，无内容改动。 |

**同步后验证**：`cd src-tauri && cargo check` 通过（asdf cargo 1.96.0，17.75s）。本次无 frontend/locales 改动，`frontend && npm run build` 状态与同步前一致，无需重跑；桌面端自定义文件未触及。

---

#### 2026-08-13：同步 `45e2bd3` -> `0e8fed0`（5 个 commit）

origin 版本 `v1.0.27` -> `v1.0.28`；桌面端版本 `1.0.27001` -> `1.0.28001`。

`cd mcphub-origin && git --no-pager log --oneline 45e2bd3..0e8fed0` 共 5 个 commit；`git diff --stat 45e2bd3..0e8fed0 -- frontend/ locales/` 涉及 `ServerForm.tsx`（#1034，2174 行重构）+ 4 个 locales（#1032，每文件 6 键），以及 Node 后端 `mcpService.ts`/`serverController.ts`/`mcpOAuthProvider.ts`/`serverConfigPersistence.ts`/测试（#1032/#1033/#1041，不同步）。

**已同步到 desktop（前端 / locales）**

| 来源 commit | 说明 | desktop 应用方式 |
| ----------- | ---- | ---------------- |
| `cf1adc9` | fix: wake startOnDemand servers so they can serve tool calls (#1032) | locales：en/zh/fr/tr 各加 6 键（`server.startOnDemand`/`startOnDemandDescription`/`idleTimeoutMs`/`idleTimeoutMsDescription` + `status.sleeping`/`sleepingDescription`），用上游翻译。桌面端 `StatusDot.tsx`/`ServerForm.tsx` 此前用内联 fallback（`t('status.sleeping', 'Sleeping')` 等），现补齐真键使其走翻译。前端 tsx 无改动（#1032 前端只动 locales）。 |
| `98d51ce` | refactor: restructure server edit form into 3 sections (#1034，= `v1.0.28` tag) | 前端：`ServerForm.tsx` 手动合并 3 分区重构（Section 1 Basic Info / Section 2 Connection / Section 3 Advanced Options 可折叠 + `isAdvancedExpanded` state）。**合并策略**：以 origin 新版（98d51ce）为基线（已含桌面端 openapi oauth2 字段 + perSessionClient + startOnDemand），再定点回加 3 处桌面端差异：① `getInitialServerType` 显式返回类型 `: 'stdio'\|'sse'\|'streamable-http'\|'openapi'` + `!== 'builtin'` 跳过（ServerForm 仅用于自定义 server）；② visibility 默认 `'public'`（origin 为 `'private'`）；③ 删除 Advanced 分区内的 visibility 选择器（桌面端隐藏可见性，所有 server 默认公开），留 `{/* Visibility section hidden in desktop client - all servers are public by default */}` 注释。origin 在 #1034 中移除的 OAuth 旧字段（authorizationEndpoint/tokenEndpoint/scopes/resource/accessToken/refreshToken）在桌面端本就被注释为死代码，随基线一并清理。perSessionClient 注释采纳 origin 更准确的「except openapi」（与 `serverType !== 'openapi'` 条件一致）。`diff -w origin新 vs 合并后` 仅余上述 3 处桌面差异，确认无误。无 locales 改动（origin 用内联 fallback）。 |

**未同步（经评估无需 / 无法同步）**

| 来源 commit | 说明 | 处理决策 | 原因分析 |
| ----------- | ---- | -------- | -------- |
| `cf1adc9`（后端部分） | fix: wake startOnDemand（Node `mcpService.ts` 216 行：重写 `ensureServerReady` 直接 spawn+connect+缓存 tools/prompts/resources、`getServerByTool` 跳过 disabled、`primeOnDemandServers` 启动时填充工具缓存、startOnDemand 限 stdio） | **不同步** | origin 的关键 bug 是 `ensureServerReady` 走 `reconnectServer`→`initializeClientsFromSettings`，后者对 on-demand 跳过连接，导致**永远唤不醒睡眠 server**。桌面端 `mcp/on_demand.rs::call_tool_on_demand` 本就是直接 `build_client`+`connect`+`list_tools`+缓存（§3.8），不存在该 bug。逐项核对 origin 4 处修复：① 直接唤醒——桌面已具备；② disabled 守卫——桌面 disable 时 `disconnect_server` 移除 pool 占位 + `shutdown_on_demand_lifecycle`，disabled server 无 pool 条目，`call_tool` 落到「not connected」而非唤醒（比 origin 的 `enabled===false` 检查更彻底，覆盖所有 server 类型）；③ `primeOnDemandServers`（启动时唤醒每个 on-demand server 填充工具缓存再睡）——桌面端**有意不镜像**：on-demand 为低频 server 省内存，启动时全量唤醒会抵消收益；桌面端睡眠 server 工具在首次唤醒后缓存（§3.8.7），此前行为一致，属设计取舍（代价：外部 MCP 客户端 `tools/list` 在首次唤醒前看不到该 server 工具）；④ startOnDemand 限 stdio——桌面 `pool::connect_server` 已 `start_on_demand = cfg.start_on_demand.unwrap_or(false) && cfg.server_type == ServerType::Stdio`（§3.8.3），已具备。综上无需 Rust 镜像。 |
| `63c84ff` | fix: apply resource description/enabled overrides in dashboard list (#1033) | **不同步** | origin 在 `getServersInfo`（dashboard 列表）给上游 server 的 resources 补上 per-URI 的 description/enabled 覆盖（`serverConfig?.resources?.[resource.uri]`）。桌面端 `list_servers`/`get_server` 返回 `resources: Vec::new()`（`commands/servers.rs` 多处），**dashboard 根本不暴露上游 server 的 resources**，没有可应用覆盖的 resource 列表。桌面端 `BuiltinResource` 是 hub 自有 builtin 资源（`builtin_resources` 表，`ResourcesPage` 管理），与上游 resource 覆盖是不同特性。架构不对应，N/A。 |
| `0e8fed0` | Remove vector embeddings under old name when renaming a server (#1041) | **不同步** | origin 在 rename server 时调 `removeServerToolEmbeddings(oldName)` 清掉旧名下的 server-tool 向量嵌入，避免 `search_tools` 广告幻影工具。该向量嵌入属于 Smart Routing / `vectorSearchService`（按语义搜索 server 工具）。桌面端 Smart Routing 未实现（§7 待办），无 `vectorSearchService`/server-tool 嵌入；`http_server.rs` 的 `$smart` 仅是「调用所有已连接 server」的 fan-out 路由，非向量搜索。架构不对应，N/A。 |
| `52e1f49` | chore(deps): bump js-yaml 4.3.0->4.3.1 (#1038) | **不同步** | 仅改 `package.json`+`pnpm-lock.yaml`；js-yaml 为 Node 后端 YAML 依赖，桌面端 Rust 用 serde_yaml，不共享 origin pnpm 依赖图（4.1 策略 4）；无 frontend/locales 改动。 |

**同步后验证**：`cd frontend && npm run build` 通过（7.16s，ServerForm 3 分区重构编译通过）；`cd src-tauri && ORT_SKIP_DOWNLOAD=1 CARGO_NET_OFFLINE=true cargo check` 通过（asdf cargo 1.96.0，22.95s）。桌面端自定义文件 `ServerForm.tsx` 经 `diff -w` 核对仅保留 3 处桌面差异未被覆盖；locales 4 文件均通过 `JSON.parse` 校验，runtime* 键未受影响。

---

#### 2026-08-06：同步 `5894e44` -> `45e2bd3`（5 个 commit）

origin 仍为 `v1.0.27`（`45e2bd3` = `v1.0.27` tag 之后 5 个未发布提交，无新 tag）；桌面端版本号不变 `1.0.27001`。

`cd mcphub-origin && git --no-pager log --oneline 5894e44..45e2bd3` 共 5 个 commit；`git diff --stat 5894e44..45e2bd3 -- frontend/ locales/` 涉及 `JSONImportForm.tsx`/`ServerCard.tsx`/`types/index.ts`/`jsonImport.ts`（#1014）+ 4 个 locales，以及 Node 后端 `mcpService.ts`/`mcpOAuthProvider.ts`/`jsonSchemaValidator.ts`（#1028，不同步）。

**已同步到 desktop（前端 / locales）**

| 来源 commit | 说明 | desktop 应用方式 |
| ----------- | ---- | ---------------- |
| `45e2bd3` | fix(ui): clearer server-import validation and OAuth clientId/redirect-uri guidance (#1014) | 前端：`utils/jsonImport.ts`——`normalizeImportedServers` 改返回 `{servers, issues}`，新增 `KNOWN_KEYS`/`NormalizedServer`/`ImportIssue`/`NormalizeResult`，检测未知顶层字段 / remote 缺 `url` / stdio 缺 `command` 收集为 issue，remote 类型补传 `oauth`。**合并而非覆盖**：保留桌面端 `parseServerType`/`autoDetectType` 宽松类型检测（commit `74e3f17`），仅叠加 #1014 的 issue 上报与 `oauth` 透传，不回退宽松匹配（上游「unsupported type」严格分支因桌面端宽松检测总能在已知类型内消解而省略）。`components/JSONImportForm.tsx`——`handlePreview` 解构 `{servers, issues}`，有 issue 时展示 `jsonImport.validationErrors` + 详情，`servers` 为空则中止。`components/ServerCard.tsx`——`handleOAuth` 开窗后若 `server.oauth.clientIdConfigured` 追加 `status.oauthClientIdHint` 警告 toast。`types/index.ts`——`Server.oauth` 加 `clientIdConfigured?: boolean`。locales：en/zh/fr/tr 各加 `status.oauthClientIdHint` + `jsonImport.validationErrors` 2 键（用上游翻译）。 |
| `aa1903d` | feat: add ResilientJsonSchemaValidator (#1028) | 前端无改动（上游改 Node 后端）。 |

**未同步（经评估无需 / 无法同步）**

| 来源 commit | 说明 | 处理决策 | 原因分析 |
| ----------- | ---- | -------- | -------- |
| `aa1903d` | feat: add ResilientJsonSchemaValidator (#1028) | **不同步** | 上游 bug 源于 Node MCP SDK 的 `AjvJsonSchemaValidator` 在 `tools/list` 预编译 outputSchema 时对不可解析 `$ref` 抛异常。桌面端 Rust MCP 客户端（`stdio_transport.rs`/`http_transport.rs`/`sse_transport.rs`）仅把 `outputSchema` 原样存为 `serde_json::Value`（`models/server.rs:192`），不编译也不校验输出 schema，`Cargo.toml` 无 ajv/jsonschema/schemars/validator 依赖，该 bug 在桌面端不存在。 |
| `45e2bd3`（后端部分） | fix(ui): OAuth `clientIdConfigured` 后端透传（`mcpOAuthProvider.ts`/`mcpService.ts`/`src/types/index.ts`） | **不同步** | 桌面端无**上游** OAuth provider：Rust `ServerInfo` 不暴露 `authorizationUrl`/`clientIdConfigured`（grep `authorization_url`/`clientIdConfigured` 在 `src-tauri/` 无命中），`ServerCard.handleOAuth` 的 `server.oauth?.authorizationUrl` 分支当前为 dormant。前端 `clientIdConfigured` 字段 + 警告 toast 仍已镜像（dormant），待上游 OAuth 实现后自动生效。`http_server.rs` 的 OAuth 是 hub 自身 REST 鉴权（下游），非上游 MCP 连接。 |
| `d2a02fc` | chore(deps): bump hono 4.12.27->4.12.34 (#1027) | **不同步** | hono 为 Node 后端框架，桌面端 Rust 用 axum，`frontend/package.json` 无 hono 依赖。 |
| `8d7f761` | chore(deps-dev): bump postcss 8.5.18->8.5.23 (#1026) | **不同步** | 桌面端 `frontend/package.json` 的 `postcss: "^8.5.6"` caret 已覆盖 8.5.23，无需改动。 |
| `6dee783` | chore(deps): bump undici 7.28.0->7.29.0 (#1025) | **不同步** | undici 为 Node 后端 HTTP 客户端，桌面端 Rust 用 reqwest，`frontend/package.json` 无 undici 依赖。 |

**同步后验证**：`cd frontend && npm run build` 通过（2.53s）；`npx tsc --noEmit` 在移植文件中无新增错误（`jsonImport.ts:130` 的 `parseServerType` 返回 `string` 赋给联合类型、`ServerCard.tsx:350-354` 的 `server.type`/`server.openapi` 均为 HEAD 既有，非本次引入；项目用 vite build 不做严格类型检查）。桌面端 `parseServerType`/`autoDetectType` 宽松检测保留未被覆盖；locales JSON 4 文件均通过 `JSON.parse` 校验；RAG/markdown WIP 同步前已 stash，未受影响。

---

#### 2026-08-04：同步 `29c0704` -> `5894e44`（11 个 commit）

origin 版本 `v1.0.26` -> `v1.0.27`；桌面端版本 `1.0.26001` -> `1.0.27001`。

`cd mcphub-origin && git --no-pager log --oneline 29c0704..5894e44` 共 11 个 commit；`git diff --stat 29c0704..5894e44 -- frontend/ locales/` 仅 4 个前端文件改动（全部来自 `976b4ac` #1012）。

**已同步到 desktop（前端 / locales）**

| 来源 commit | 说明 | desktop 应用方式 |
| ----------- | ---- | ---------------- |
| `976b4ac` | feat: on-demand stdio server spawning to reduce memory usage (#1012) | 前端：`types/index.ts`（`ServerConfig` + `ServerFormData` 新增 `startOnDemand`/`idleTimeoutMs`）、`StatusDot.tsx`（新增 `startOnDemand` prop + Sleeping 渲染）、`ServerCard.tsx`（传 `startOnDemand` prop）、`ServerForm.tsx`（stdio 专属「按需启动」checkbox + idle timeout 输入框 + 初始化）、`serverFormPayload.ts`（payload 携带 `startOnDemand`/`idleTimeoutMs`）。全部为桌面端自定义文件，手动合并保留 hub 样式 / 隐藏 visibility / OAuth2 / 下载进度条等差异。locales 无改动（origin 用内联默认值）。 |

**已镜像到 desktop（Rust 后端）**

| 来源 commit | 说明 | desktop 镜像方式 |
| ----------- | ---- | ---------------- |
| `976b4ac` | feat: on-demand stdio server spawning (#1012) | Rust 后端完整实现（详见 §3.8）：`models/server.rs` 新增 `start_on_demand`/`idle_timeout_ms`（`ServerConfig`）+ `start_on_demand`（`ServerStatus`）；`db/migration.rs` `migrate_v17`（`TARGET_VERSION` 16->17）加两列；`server_service.rs` SELECT/INSERT/UPDATE/map_row 持久化 + `perSessionClient`/`startOnDemand` 互斥校验；新增 `mcp/on_demand.rs`（仿 `session_pool.rs`：懒建 client + 创建锁去重 + idle 定时器 + sleeping/awake 状态机）；`pool.rs` `connect_server` 对 on-demand stdio 插入睡眠占位、`call_tool` 路由到 on-demand store、`list_all_tools` 含睡眠 server 缓存工具、`disconnect_server`/`disconnect_all` 清理 on-demand client、新增 `mark_on_demand_awake/sleeping/error` 占位状态机；`mcp_manager.rs` 启动日志区分 sleeping、自动重连循环跳过 on-demand；`http_server.rs` `mcp_scope_server_filters` 含 `start_on_demand` server。 |
| `a4a628a` | fix: include upstream stderr in connection errors (#1015) | `stdio_transport.rs` 新增 `stderr_tail`（`Arc<std::sync::Mutex<String>>`，~32KB 滚动缓存），stderr drain task 累积每行；`connect()` 的 `initialize` 握手失败路径把 stderr tail 拼进 error message（`--- upstream stderr ---` 段），便于排查 Python traceback / 缺失依赖。 |

**未同步（经评估无需 / 无法同步）**

| 来源 commit | 说明 | 处理决策 | 原因分析 |
| ----------- | ---- | -------- | -------- |
| `be49869` | feat: support MCP Apps passthrough on multi-server groups (#1010) | **不同步** | 桌面端无 MCP Apps / MCPB / DXT 功能（`mcp_apps`/`_meta.ui`/`ui://` 资源代理等均未实现）；多服务器组透传建立在 Apps 功能之上，无对应架构。仅改 Node `mcpService.ts` + 文档；无 frontend/locales 改动。 |
| `f9944fa` | fix(oauth): strip static Authorization header when OAuth provider is active (#1013) | **不同步** | 桌面端 MCP 传输层（SSE/HTTP）无 OAuth provider 抽象，自定义 headers 原样透传，不存在「OAuth token 被静态 Authorization 覆盖」的问题。`http_server.rs` 的 OAuth 是 hub 自身 REST API 鉴权，非上游 MCP 连接。仅改 Node `mcpService.ts`/`mcpOAuthProvider.ts`；无 frontend/locales 改动。 |
| `5894e44` | test: add unit tests for smartRouting config resolution (#1021) | **不同步** | 纯 Node 单元测试，桌面端 Smart Routing 未实现（§7 待办）；无 frontend/locales 改动。 |
| `5a2a7c6` | fix: remove duplicated semver@7.8.5 entries from lockfile (#1023) | **不同步** | 仅改 `pnpm-lock.yaml`；桌面端前端用 npm 独立管理，不共享 origin pnpm 依赖图（4.1 策略 4）；无 frontend/locales 改动。 |
| `cbc862d` | chore(deps-dev): bump @vitejs/plugin-react 5.1.3->5.2.0 (#1020) | **不同步** | 仅改 `package.json` + `pnpm-lock.yaml`；桌面端 npm 独立管理（4.1 策略 4）；无 frontend/locales 改动。 |
| `3822d66` | chore(deps-dev): bump tailwind-merge 3.3.1->3.6.0 (#1019) | **不同步** | 同上，devDependency 升级，npm 独立管理；无 frontend/locales 改动。 |
| `a205d54` | chore(deps-dev): bump @typescript-eslint/parser 8.61.1->8.65.0 (#1018) | **不同步** | 同上，lint devDependency 升级；无 frontend/locales 改动。 |
| `dc10267` | chore(deps): bump better-sqlite3 12.6.2->12.11.1 (#1016) | **不同步** | 仅改 `package.json` + `pnpm-lock.yaml`；better-sqlite3 为 Node 后端依赖，桌面端 Rust 用 sqlx，不共享依赖图（4.1 策略 4）；无 frontend/locales 改动。 |
| `a89de63` | chore(deps-dev): bump globals 13.24.0->17.8.0 (#1017) | **不同步** | 同上，lint devDependency 升级；无 frontend/locales 改动。 |

**同步后验证**：`cd src-tauri && ORT_SKIP_DOWNLOAD=1 cargo check` 通过（asdf cargo 1.96.0）；`cd frontend && npm run build` 通过。桌面端自定义文件均手动合并未被覆盖；locales runtime* 键未受影响（本次无 locales 改动）。

#### 2026-07-30：同步 `a99c382` → `29c0704`（5 个 commit）

origin 版本 `v1.0.25` → `v1.0.26`；桌面端版本 `1.0.25001` → `1.0.26001`。

> 基线说明：`a99c382` = `v1.0.25` tag，为 desktop HEAD 实际记录的子模块指针（2026-07-27 `f417a12` 同步前进至此，当时未登记 §4.3/§4.4）。本次以 `a99c382` 为起点同步到 origin/main `29c0704`（v1.0.26 之后 1 个未发布提交）。

`cd mcphub-origin && git --no-pager log --oneline a99c382..29c0704` 共 5 个 commit；`git diff --stat a99c382..29c0704 -- frontend/ locales/` 仅 1 处改动（`frontend/src/components/ServerForm.tsx`，-1 行）。

**已同步到 desktop（前端 / locales）**

| 来源 commit | 说明 | desktop 应用方式 |
| ----------- | ---- | ---------------- |
| `e88f664` | fix: allow stdio servers without arguments (#1006) | 前端：`ServerForm.tsx` 手动删除 args 输入框（placeholder `e.g.: -y time-mcp`）的 `required={serverType === 'stdio'}`（⚠️ 自定义文件手动合并，保留 hub 样式/隐藏 visibility/OAuth2 差异；command 输入框的 `required` 保留，与 origin 一致）。locales 无改动。 |

**已镜像到 desktop（Rust 后端）**

无。`e88f664` 的 Rust 后端镜像**不需要**：`mcp/pool.rs` Stdio 分支已用 `cfg.args.clone().unwrap_or_default()`（空 args 默认空 vec）、仅校验 `command` 存在；`stdio_transport.rs` 的 `cmd.args(&resolved_args)` 对空 vec 正常；`server_service.rs` create/update 无 args 必填校验。桌面端 Rust 后端早已容忍 stdio 无 args，与 origin #1006 修复后行为一致。

**未同步（经评估无需 / 无法同步）**

| 来源 commit | 说明 | 处理决策 | 原因分析 |
| ----------- | ---- | -------- | -------- |
| `ca37a89` | Add MCP Toplist rank badge (#1001) | **不同步** | 仅改 `README.md`（+2），属文档文件（4.1 策略 4）；无 frontend/locales 改动。 |
| `446aed8` | fix: fail loudly on invalid vector embedding writes (#1007) | **不同步** | 纯 Node `VectorEmbeddingRepository.ts` 向量嵌入写入校验；桌面端 Rust 后端无 vector embedding 功能（Smart Routing 在 §7 待办，未实现），架构不对应；无 frontend/locales 改动。 |
| `bb72a8c` | chore(deps): bump better-auth 1.6.19→1.6.22 (#1005) | **不同步** | 仅改 `package.json` + `pnpm-lock.yaml`；better-auth 为 Node 后端依赖，桌面端 Rust 用 Better-Auth 的对应 Rust 实现（未集成，待办），不共享 origin pnpm 依赖图（4.1 策略 4）；无 frontend/locales 改动。注：`bb72a8c` = `v1.0.26` tag。 |
| `29c0704` | chore(deps-dev): bump postcss 8.5.12→8.5.18 (#1008) | **不同步** | 仅改 `package.json` + `pnpm-lock.yaml`；postcss 为前端构建 devDependency，桌面端前端用 npm（`package-lock.json`）独立管理，不共享 origin pnpm 依赖图（4.1 策略 4）；无 frontend/locales 改动。 |

**同步后验证**：`cd frontend && npm run build` 通过；`cd src-tauri && CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse cargo check` 通过。locales runtime* 键未受影响（本次无 locales 改动）；桌面端自定义文件仅 `ServerForm.tsx` 一处定点修改，未被覆盖。

#### 2026-07-24：同步 `14a832b` → `cb44e22`（2 个 commit）

origin 仍为 `v1.0.24`（无新 tag；两 commit 均为 v1.0.24 之后的未发布提交）；桌面端版本号不变（`1.0.24003`，本次无任何 desktop 文件改动，不递增）。

`cd mcphub-origin && git --no-pager log --oneline 14a832b..HEAD` 仅 2 个 commit，`git diff --stat 14a832b..HEAD -- frontend/ locales/` 为空（无前端/locales 改动）。

**已同步到 desktop（前端 / locales）**

无。本次两 commit 均不触及 `frontend/` 或 `locales/`。

**已镜像到 desktop（Rust 后端）**

无。

**未同步（经评估无需 / 无法同步）**

| 来源 commit | 说明 | 处理决策 | 原因分析 |
| ----------- | ---- | -------- | -------- |
| `b561101` | chore(deps): bump axios from 1.16.1 to 1.18.0 (#992) | **不同步** | 仅改 `pnpm-lock.yaml`；axios 为 Node 后端 HTTP 客户端依赖，桌面端 Rust 用 reqwest，不共享 origin pnpm 依赖图（4.1 策略 4）；无 frontend/locales 改动。 |
| `cb44e22` | chore(deps): bump js-yaml from 4.2.0 to 4.3.0 (#993) | **不同步** | 仅改 `package.json` + `pnpm-lock.yaml`；js-yaml 为 Node 后端 YAML 解析依赖（服务器配置/OpenAPI），桌面端 Rust 用 serde_yaml，不共享 origin pnpm 依赖图（4.1 策略 4）；无 frontend/locales 改动。 |

**同步后验证**：本次无任何源码文件改动（仅更新本节文档与基线 SHA），`frontend && npm run build` / `src-tauri && cargo check` 状态与同步前一致，无需重跑。


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
- [X]  版本号四源同步（`tauri.conf.json` / `Cargo.toml` / 根 `package.json` / `frontend/package.json`；当前 1.0.30001；详见 3.4.7）
- [X]  stdio 服务器按需启动（startOnDemand：跳过启动连接、首次工具调用懒建进程、空闲超时自动关闭、缓存工具保留；详见 3.8）
- [X]  stdio 连接错误包含上游 stderr（`stderr_tail` 滚动缓存拼接进 handshake 失败 error；详见 3.9）
- [X]  编辑服务器避免无谓重连 + proxy 持久化（镜像上游 #1055：`update_server` 比对连接相关字段，仅访问/元数据变更时保留实时连接；`ServerConfig.proxy` + DB v20 持久化使前端 round-trip 生效；详见 3.11）

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