# MCPHub Desktop

> ## ⚠️ 重要声明（请先阅读）
>
> - 仓库内的 [`mcphub-origin/`](./mcphub-origin) 目录 **不属于本项目**，它是第三方开源项目 [samanhappy/mcphub](https://github.com/samanhappy/mcphub) 的源码快照，**版权归原作者 [@samanhappy](https://github.com/samanhappy) 及其贡献者所有**。
> - 之所以把它放进仓库，仅用于：① 让本项目的改写过程可追溯；② 方便比对桌面端与 Web 端的差异；③ 离线查阅原文档。
> - **本项目自身的代码只包含**：[`frontend/`](./frontend)、[`src-tauri/`](./src-tauri)、[`scripts/`](./scripts)、[`locales/`](./locales) 以及根目录配置文件。
> - 严禁修改 `mcphub-origin/` 中的任何文件；如需理解、调试或贡献代码，请只在上述本项目目录内操作。

[![Tauri](https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri)](https://tauri.app/)
[![Upstream](https://img.shields.io/badge/Upstream-samanhappy%2Fmcphub-orange?logo=github)](https://github.com/samanhappy/mcphub)
[![License](https://img.shields.io/badge/License-MIT-blue.svg)](./mcphub-origin/LICENSE)

![MCPHub Desktop 主界面](./doc/imgs/home.png)

## 项目简介

**MCPHub Desktop** 是一款基于 [Tauri 2](https://tauri.app/) 构建的**本地优先（Local-first）**桌面客户端，用于在个人电脑上统一管理 MCP（Model Context Protocol）服务器、**Skills**、**RAG 知识库**与**嵌入模型**。所有进程、配置、密钥与向量数据都运行/保存在用户自己的机器上，不依赖任何远端服务。

除 MCP 服务器聚合外，本项目面向个人用户重点扩展了三大本地能力：

- **Skills 管理** —— 统一管理 AI Agent 的技能库，一键安装/导出到数十种 Agent。
- **RAG 知识库** —— 本地文档自动分块、向量化、入库与混合检索，作为 Agent 的**本地长期记忆（Long-term Memory）**。
- **嵌入模型管理** —— ONNX / GGUF 双后端、多架构嵌入模型随包分发，本地推理、数据不出本机。

> 💡 **本地长期记忆**：RAG 知识库让 AI Agent 拥有跨会话、可检索、可增删的长期记忆 —— 你上传的文档、笔记、代码、规范会被分块并向量化保存在本机，Agent 通过语义 + 关键词混合检索按需取用，不再受单次会话上下文窗口限制。所有向量数据**只存于你的电脑**，隐私可控、离线可用。

- **产品名称**：MCPHub Desktop
- **应用标识**：`app.mcphub.desktop`
- **当前版本**：见 [`src-tauri/tauri.conf.json`](./src-tauri/tauri.conf.json)
- **上游项目**：<https://github.com/samanhappy/mcphub>（仓库内副本位于 [`mcphub-origin/`](./mcphub-origin)，**非本项目代码**）

## 核心功能

### MCP 服务器管理
- 本地统一查看、启停、调试、分组多个 MCP Server，进程由 Rust 托管。
- 随包分发 Node / UV / Bun 运行时（见 [`src-tauri/runtimes/`](./src-tauri/runtimes)），装上即用、无需预装环境。
- 支持分组、智能路由、Bearer Key 鉴权、OAuth/OIDC、活动日志。

### Skills 管理
- 本地统一管理 **Skills 库**：导入（扫描已安装 Agent 的技能目录或手动指定文件夹）、卸载、查看，集中沉淀个人技能资产。
- **一键安装/导出到 AI Agent**：内置数十种 Agent 目录（见 [`src-tauri/runtimes/skill/install.json`](./src-tauri/runtimes/skill/install.json)），涵盖 Claude Code、Cursor、Windsurf、Cline、Roo Code、Continue、Goose、OpenHands、Codex 等主流客户端；也支持自定义 Agent 路径。
- **两种安装方式**：软链接（symlink，源库变更自动同步、省空间）或文件拷贝（copy，独立副本、便于分发），按 Agent 灵活选择。
- Skills 在多个 Agent 间**共享与同步**，避免重复维护；导出状态可追溯（每个 Skill 记录已安装到哪些 Agent、用何种方式）。

### RAG 知识库（Agent 本地长期记忆）
- 上传文档自动 **分块 + 向量化 + 入库**，提供**混合检索**（向量语义相似度 + 关键词），让 Agent 拥有跨会话的**本地长期记忆**。
- **语义化分块（策略模式）**：基于 [`text-splitter`](https://crates.io/crates/text-splitter) —— 普通文本走 Unicode 词/句边界，Markdown 走块/标题边界，源代码走 tree-sitter AST 边界（按扩展名自动分派）。
- **分块参数模型自适应**：`chunk_size` / `chunk_overlap` 默认 "Auto"（按模型 `deploy.json` 推荐值取用，受模型上下文窗口钳制）；关闭 Auto 后可手动覆盖。
- 支持查看任意文档的分片内容、按 Tag 过滤检索、文档删除时同步清理本地文件与向量。
- **文件查看可视化**：Markdown / 代码 / 纯文本按类型渲染（代码经 highlight.js 着色），搜索片段同样按类型高亮。
- **MCP 工具暴露**：`rag_file_create` / `rag_file_update` / `rag_search` / `rag_get` / `rag_tag_search` 等工具经 MCP `tools/call` 暴露，Agent 可直接读写知识库 —— 即**把长期记忆当成可调用的工具**。

### 嵌入模型管理
- 支持 ONNX（ort）与 GGUF（candle）两种后端，格式自动探测。
- 已支持架构：Gemma3、Qwen3、nomic-bert-moe、LFM2、**modern-bert**（Granite Embedding 97M Multilingual R2 等）。
- 模型随包分发或按需下载，GPU 优先（Metal / CUDA / CoreML / DirectML）、CPU 兜底。
- 每个模型尺寸带 `deploy.json`：声明平台（GPU/CPU/AUTO）、描述、是否默认、非对称嵌入前缀、推荐分块尺寸。

### 其他
- 多语言界面（en / zh / fr / tr）、托盘常驻、自动更新、暗色模式同步。
- 鉴权密钥写入操作系统钥匙串（keyring）；数据存于本机 SQLite（`$APPDATA/mcphub.db`）。

## 与上游项目的定位差异

> 上游 [mcphub](https://github.com/samanhappy/mcphub)（仓库内副本位于 [`mcphub-origin/`](./mcphub-origin)，版权归原作者所有）是面向**服务端 / 团队侧**的 MCP 聚合 Web 服务；本项目是面向**个人本机**的桌面客户端。两者赛道不同，互补而非替代。

| 维度 | 上游 mcphub（Web，第三方项目） | MCPHub Desktop（本项目） |
| --- | --- | --- |
| 定位 | 服务端 MCP 聚合 Hub，被多客户端共享访问 | 个人本机 MCP 工具统一管理客户端，本地优先 |
| 形态 | Node.js Web 服务 + 浏览器访问 | Tauri 2 原生桌面应用 |
| 后端语言 | TypeScript (Express.js, ESM) | Rust（位于 `src-tauri/`） |
| 前端 | React + Vite + Tailwind | 复用上游前端（拷贝至 `frontend/`，按需适配 Tauri invoke） |
| 数据存储 | JSON 文件 / PostgreSQL | 本机 SQLite（`$APPDATA/mcphub.db`，sqlx 0.8） |
| 鉴权 | JWT + bcrypt（环境变量配置） | JWT + bcrypt，密钥写入操作系统钥匙串（keyring 3） |
| MCP 进程 | 由 Node 服务托管 | 由 Rust 进程托管，随包分发 Node/UV/Bun 运行时（见 [`src-tauri/runtimes/`](./src-tauri/runtimes)） |
| 通信方式 | HTTP `/api/*` | Tauri `invoke`（前端 `fetchInterceptor` 透明转发） |
| RAG / Skills / 嵌入模型 | 无 | 内置：Skills 一键分发 + RAG 知识库作本地长期记忆（GGUF/ONNX 嵌入 + lancedb 向量库 + tree-sitter 分块 + MCP 工具暴露） |
| 安装与分发 | Docker / npm CLI | 平台原生安装包（dmg / msi / AppImage），支持自动更新 |

本项目在**保留上游前端 UI 与交互**的前提下，将后端用 Rust + Tauri 重新实现，并扩展了 **Skills 管理、RAG 知识库（Agent 本地长期记忆）、嵌入模型管理** 三大本地能力，沉淀为面向个人用户的桌面客户端。

## 仓库结构

> 下表中标注 **【本项目】** 的目录才属于本仓库自有代码；标注 **【第三方】** 的为上游 mcphub 的源码副本，仅作参考。

```
mcphub-desktop/
├── frontend/          # 【本项目】桌面端使用的前端（源自上游 frontend，按需适配 Tauri）
├── src-tauri/         # 【本项目】Tauri / Rust 后端
│   ├── src/           #   Rust 源码（业务逻辑、MCP 管理、鉴权、SQLite、RAG、Skills 等）
│   ├── migrations/    #   SQLite 迁移脚本
│   ├── runtimes/      #   随包分发的本地运行时（Node / UV / Bun）+ 嵌入模型
│   └── tauri.conf.json
├── locales/           # 【本项目】i18n 翻译文件（en / zh / fr / tr）
├── scripts/           # 【本项目】构建辅助脚本（运行时下载、暗色模式同步等）
├── doc/               # 【本项目】升级说明与设计文档
├── AGENTS.md          # 【本项目】迁移与开发完整参考（强烈建议先阅读）
├── package.json       # 【本项目】桌面端入口（tauri dev / tauri build）
└── mcphub-origin/     # 【第三方】🔒 上游 samanhappy/mcphub 源码快照
                       #            版权归原作者所有，仅供参考，禁止修改
```

> ⚠️ **重要约束**：`mcphub-origin/` **不是本项目的代码**，而是第三方上游项目的只读快照，**禁止以本项目名义修改、提交或重新发布其内容**。所有改动请只在标注 **【本项目】** 的目录下进行，详情见 [`AGENTS.md`](AGENTS.md)。

## 快速开始

### 环境要求

- macOS / Windows / Linux
- [Node.js](https://nodejs.org/) ≥ 18
- [Rust](https://www.rust-lang.org/) stable（含 `cargo`，MSRV 见 `src-tauri/Cargo.toml`）
- [Tauri 2 系统依赖](https://tauri.app/start/prerequisites/)

### 准备运行时（首次）

```bash
# 下载随包分发的 Node / UV / Bun 运行时到 src-tauri/runtimes/
bash scripts/download-runtimes.sh
```

### 安装依赖

```bash
# 桌面端壳（Tauri CLI）
npm install

# 前端依赖
cd frontend && npm install && cd ..
```

### 开发模式

```bash
npm run dev   # 等价于 tauri dev，自动启动前端 dev server 并加载 Rust 后端
```

### 构建发布包

```bash
npm run build # 等价于 tauri build，产物输出至 src-tauri/target/release/bundle/
```

## 安装使用

从 [Releases](https://github.com/skrstop/MCPHub-Desktop/releases) 页面下载对应平台的安装包。

### macOS

安装后打开应用时，如果系统提示 **"MCPHub Desktop.app 已损坏，无法打开"**，这是因为应用未使用 Apple 开发者证书签名导致的。请在终端中执行以下命令后重新打开即可：

```bash
xattr -cr "/Applications/MCPHub Desktop.app"
```

### Windows

下载并运行 `.msi` 或 `.exe` 安装程序。

### Linux

根据你的发行版下载 `.deb`、`.AppImage` 或 `.rpm` 包安装。

## 文档

- [`AGENTS.md`](AGENTS.md)：迁移背景、目录约定、模块划分、待办事项等完整开发参考。
- [`doc/upgrade/`](./doc/upgrade)：各版本升级说明。
- [`mcphub-origin/README.md`](./mcphub-origin/README.md)：**第三方上游项目** 的 README（英文）。
- [`mcphub-origin/README.zh.md`](./mcphub-origin/README.zh.md)：**第三方上游项目** 的 README（中文）。

## 致谢与许可

- **上游项目归属**：[`mcphub-origin/`](./mcphub-origin) 内的全部代码、文档、资源均来自第三方开源项目 [samanhappy/mcphub](https://github.com/samanhappy/mcphub)，**版权归原作者 [@samanhappy](https://github.com/samanhappy) 及其贡献者所有**，本项目仅作镜像保留以便溯源，未对其主张任何权利。
- **致谢**：感谢 [@samanhappy](https://github.com/samanhappy) 及所有上游贡献者提供了优秀的开源实现。
- **许可证**：上游项目许可证见 [`mcphub-origin/LICENSE`](./mcphub-origin/LICENSE)；本桌面端在严格遵守该许可证的前提下进行二次开发与发布。
