# OAuth 复核战役 — 第十六轮（R16xx）报告（2026-10-10）

## 本轮扫描面

| 代理 | 切片 | 结果 |
| --- | --- | --- |
| r16-a1 | R15 五项修复自查（fix-of-fix 回归猎杀） | **1 High**（R15 拒绝式改名修复自身引入延迟毁损）+ 1 Low + 2 Info + 验证 5 项全过 |
| r16-a2 | 薄覆盖切片逐行（rag/extract 900 行 / models serde 契约 / tray+lib setup / tauriClient 路由 / 契约脚本） | **1 Medium + 1 Low** + 2 Info + 残留零命中、路由零孤儿 |
| r16-a3 | 套件 r1600 + 冒烟 | 29 PASS/0 FAIL/4 具名 SKIP + 冒烟 3 套全绿 |

## 修复 5 项

| # | 级别 | 内容 |
| --- | --- | --- |
| 1 | **[H]** | R15 修复自伤：update_doc 改名碰撞 warn 后 `meta.name` 已持久化 → `content_path_for` 候选 4 解析到**另一文档的内容文件**——读串内容、后续 update 写穿他文档（比 R15 前的当场覆盖更隐蔽）。改为 `.meta` 守卫后立即硬失败 `NAME_COLLISION`（任何 fs 变更/内容写/重索引之前，meta 与磁盘保持一致） |
| 2 | **[M]** | Windows OCR `status()` 硬编码 available:true，但引擎创建可失败（无 OCR 语言）→ 前端预检/安装对话框永不触发 + `OCR_MISSING` sentinel 分支被绕过 → OnceLock 缓存引擎探测（TryCreateFromUserProfileLanguages → zh-Hans → en-US），失败时 engine 字段承载原因 |
| 3 | **[L]** | reindex_all symlink 分支 `std::fs::read` 无大小上限（8GB symlink 视频全量进内存才被 is_likely_text 拒）→ metadata 预检 ≤ MAX_UPLOAD_BYTES 对齐显示路径 |
| 4 | **[L]** | 契约脚本 `'oauth'` 白名单过期（Rust 侧 R10 起已有 oauth 字段）→ 移除白名单，oauth 键丢弃现在会 FAIL；复跑 0 mismatch |
| 5 | Info 记录 | 10min 负缓存：断网→恢复网络后 10min 内 http→https 规范化跳过（一次 clone 失败自愈）；get_public_config 用 get()（正确方向）但损坏配置无 UI 信号（Guest 界面 + 全 401）——产品决策待办 |

## 套件与验证

- **rmcp_round_r1600.py 29/29**（+4 具名 SKIP：reindex_all 负半程与 IPC-only 命令不可 HTTP 触达）：
  - F 组 5 版本 initialize 回显 + 2025-06-18 会话通道 + 2026-07-28 无状态通道**真实公网 IP 调用**
  - E 组改名碰撞：-32603 NAME_COLLISION + docB 内容完好 + docA 文件存活（真实 MCP 内建工具）
  - D 组 canonicalize 缓存：3 次 git 文档删除，热缓存后 <8s 无重探测
  - C 组 file 源消失：synthetic copy 文档**熬过真实 auto tick**（interval→60s，app_log 取证）
  - A 组 get_strict：corrupt config → admin gate 401 + /health 200 + `{}` 默认路径保留
- 冒烟：r1500 20/20、pin_matrix_r111 31/31、sync_baseline_r109 32/32
- **全量回归：见文末统计**；cargo check 0 错 0 警；cargo test --lib 148 passed；契约脚本 OK

## a2 清洁验证（薄切片逐行）

- **rag/extract 全 5 策略**：无 unwrap/panic（畸形输入全 sentinel 化）、spawn_blocking 正确、tesseract 临时文件全路径清理、64MiB/12000px 限额、pdf 括号深度预扫描防 SIGSEGV
- **models/**：max_buffer_size / passthrough_headers / oauth 11 字段 / pinned / BearerKey（token 仅服务端）/ ActivityEntry camelCase / RagSettings 双侧一致——serde 默认全覆盖，无静默丢弃
- **tray/lib setup**：菜单 handler 注册一次 swap 守卫、quit 全走 disconnect_all、autostart stash handle 无死锁、CloseRequested 先取消 RAG 导入、迁移先于 start_all
- **残留审计**：dispatch_mcp/mcp_version/SESSION_STRATEGY/SSE_CHANNELS/jsonrpc_response/create_tool_task/http_transport.rs **全仓零代码命中**；tauriClient 路由 ↔ lib.rs 注册机械交叉比对**零孤儿**

## 收敛判定（R15 预设停止条件核验）

R15 预设：连续 2 轮在已覆盖切片 0 新发现 + 全切片至少一轮逐行 → 停止无限轮次。

- 全切片逐行覆盖：**本轮达成**（rag/extract + models + tray/lib + 前端协议层为最后薄覆盖面，已清空）
- 已覆盖切片新发现：**未达成**——R15 自身的修复引入了 1 High（证明了「修复需一轮验证」的必要性，也证明 2 轮窗口判据是对的而非过早）
- **下一步**：R17 = R16 修复自查 + E2E 弱断言审计收尾。若 R17 已覆盖切片 0 新 High → 达成连续 2 轮条件 → 战役收敛，转「新功能/上游同步触发定点复核」模式。

---
*全量回归统计见 AGENTS.md 本轮条目。*
