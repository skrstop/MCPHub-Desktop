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

## 项目简介

**MCPHub Desktop** 是一款使用 [Tauri 2](https://tauri.app/) 构建的跨平台桌面客户端，基于第三方开源项目 [samanhappy/mcphub](https://github.com/samanhappy/mcphub) 的前端复用 + 后端重写而来：

- 复用了上游 mcphub 的 React/Vite 前端 UI；
- 将上游基于 Node.js + Express 的后端能力，使用 Rust 在本地进程中重新实现；
- 让用户无需常驻 Web 服务，即可在本机统一管理多个 MCP（Model Context Protocol）服务器。

> 上游 mcphub 项目并非本仓库作者所有；本项目是对其的二次开发与桌面化改写。

- **产品名称**：MCPHub Desktop
- **应用标识**：`app.mcphub.desktop`
- **当前版本**：见 [`src-tauri/tauri.conf.json`](./src-tauri/tauri.conf.json)
- **上游项目**：<https://github.com/samanhappy/mcphub>（仓库内副本位于 [`mcphub-origin/`](./mcphub-origin)，**非本项目代码**）

## 改写自上游 mcphub 的原因

> **本项目与上游 mcphub 处于不同赛道**：上游聚焦于"服务端 / 团队侧"的 MCP 聚合服务，本项目聚焦于"个人客户端本地"的 MCP 工具统一管理。
> 因此重写的出发点 **不是** 部署门槛、性能或资源占用，而是 **使用场景与产品形态完全不同**。

上游 [mcphub](https://github.com/samanhappy/mcphub)（仓库内副本位于 [`mcphub-origin/`](./mcphub-origin)，**版权归原作者所有，并非本项目自有代码**）是一个面向服务端部署的 MCP 聚合管理 Web 服务，定位是"被多客户端共享访问的中心化 Hub"。

而本项目想解决的是另一个问题：**个人开发者/普通用户在自己电脑上同时使用 Claude Desktop、Cursor、Cherry Studio、各类 IDE 插件等多种 MCP Client 时，本地散落着大量 MCP Server 配置，缺乏一个统一的本地管理入口。** 具体诉求包括：

1. **本地统一管理**：把分散在不同 MCP Client 配置文件里的 Server，集中到一个本地客户端中查看、启停、调试、分组。
2. **本地优先（Local-first）**：所有 MCP Server 进程、配置、密钥都运行/保存在用户自己的机器上，不依赖任何远端服务，也无需暴露监听端口。
3. **桌面原生体验**：托盘常驻、原生窗口、跨平台安装包（dmg / msi / AppImage）、自动更新、随包分发的 Node / UV / Bun 运行时，做到"装上即用"。
4. **面向最终用户而非运维**：不需要懂 Docker、反向代理、数据库，直接双击安装即可使用。
5. **与上游解耦演进**：在桌面客户端这一形态下，可以独立迭代 UI 交互、本地存储、进程管理等能力，不必受 Web 端架构约束。

简言之：**上游 mcphub 解决"如何把多个 MCP Server 聚合成一个共享服务"，本项目解决"如何在我自己的电脑上统一管理本地的多个 MCP 工具"**——两者互补，而非替代。

因此本项目在**完全保留上游前端 UI 与交互**的前提下，将后端用 **Rust + Tauri** 重新实现，沉淀为面向个人用户的桌面客户端。

## 与上游项目的核心差异

> 再次强调：以下差异是因为**两者赛道不同**（服务端聚合 Hub vs 个人本地 MCP 工具管理客户端），并非"谁更好"的对比。

| 维度 | 上游 mcphub（Web，第三方项目） | MCPHub Desktop（本项目） |
| --- | --- | --- |
| 定位 | 服务端 / 团队侧的 MCP 聚合 Hub，被多客户端共享访问 | 个人本机的 MCP 工具统一管理客户端，本地优先 |
| 形态 | Node.js Web 服务 + 浏览器访问 | Tauri 2 原生桌面应用 |
| 后端语言 | TypeScript (Express.js, ESM) | Rust（位于 `src-tauri/`） |
| 前端 | React + Vite + Tailwind | 复用上游前端（拷贝至 `frontend/`，按需适配 Tauri invoke） |
| 数据存储 | JSON 文件 / PostgreSQL | 本机 SQLite（`$APPDATA/mcphub.db`，sqlx 0.8） |
| 鉴权 | JWT + bcrypt（环境变量配置） | JWT + bcrypt，密钥写入操作系统钥匙串（keyring 3） |
| MCP 进程 | 由 Node 服务托管 | 由 Rust 进程托管，随包分发 Node/UV/Bun 运行时（见 [`src-tauri/runtimes/`](./src-tauri/runtimes)） |
| 通信方式 | HTTP `/api/*` | Tauri `invoke`（前端 `fetchInterceptor` 透明转发，无需改动业务代码） |
| 安装与分发 | Docker / npm CLI | 平台原生安装包（dmg / msi / AppImage 等），支持自动更新 |

## 仓库结构

> 下表中标注 **【本项目】** 的目录才属于本仓库自有代码；标注 **【第三方】** 的为上游 mcphub 的源码副本，仅作参考。

```
mcphub-desktop/
├── frontend/          # 【本项目】桌面端使用的前端（源自上游 frontend，按需适配 Tauri）
├── src-tauri/         # 【本项目】Tauri / Rust 后端
│   ├── src/           #   Rust 源码（业务逻辑、MCP 管理、鉴权、SQLite 等）
│   ├── migrations/    #   SQLite 迁移脚本
│   ├── runtimes/      #   随包分发的本地运行时（Node / UV / Bun）
│   └── tauri.conf.json
├── locales/           # 【本项目】i18n 翻译文件（en / zh / fr / tr）
├── scripts/           # 【本项目】构建辅助脚本（运行时下载、暗色模式同步等）
├── agent.md           # 【本项目】详细的迁移与开发文档（强烈建议先阅读）
├── package.json       # 【本项目】桌面端入口（tauri dev / tauri build）
└── mcphub-origin/     # 【第三方】🔒 上游 samanhappy/mcphub 源码快照
                       #            版权归原作者所有，仅供参考，禁止修改
```

> ⚠️ **重要约束**：`mcphub-origin/` **不是本项目的代码**，而是第三方上游项目的只读快照，**禁止以本项目名义修改、提交或重新发布其内容**。所有改动请只在标注 **【本项目】** 的目录下进行，详情见 [`agent.md`](./agent.md)。

## 快速开始

### 环境要求

- macOS / Windows / Linux
- [Node.js](https://nodejs.org/) ≥ 18
- [Rust](https://www.rust-lang.org/) stable（含 `cargo`）
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

## 文档

- [`agent.md`](./agent.md)：迁移背景、目录约定、模块划分、待办事项等完整开发参考。
- [`mcphub-origin/README.md`](./mcphub-origin/README.md)：**第三方上游项目** 的 README（英文）。
- [`mcphub-origin/README.zh.md`](./mcphub-origin/README.zh.md)：**第三方上游项目** 的 README（中文）。

## 致谢与许可

- **上游项目归属**：[`mcphub-origin/`](./mcphub-origin) 内的全部代码、文档、资源均来自第三方开源项目 [samanhappy/mcphub](https://github.com/samanhappy/mcphub)，**版权归原作者 [@samanhappy](https://github.com/samanhappy) 及其贡献者所有**，本项目仅作镜像保留以便溯源，未对其主张任何权利。
- **致谢**：感谢 [@samanhappy](https://github.com/samanhappy) 及所有上游贡献者提供了优秀的开源实现。
- **许可证**：上游项目许可证见 [`mcphub-origin/LICENSE`](./mcphub-origin/LICENSE)；本桌面端在严格遵守该许可证的前提下进行二次开发与发布。

