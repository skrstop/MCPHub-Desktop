# rmcp 复核第三十二轮（R32，2026-10-03）：rag_search 未门控（High）+ 版本缓存钉死离线兜底列表

> 复核范围：commands/runtime.rs（1450 行全量重读）+ commands/rag.rs（Rust）；
> serverFormPayload / serverDuplicate / clipboard / GroupCard / AccessUrlDialog / Dashboard（前端）。
> 方法：侵蚀审计全在 → 2 个并行逐行代理 → 亲核验证 5 项 → 修复 → v32 套件 → 全量回归。

## 修复（5 项：High×1，Medium×2，Low×2）

### C1 [H] rag_search_command 无 admin 门控——击穿全部读取面防线
upload/get_doc/get_chunks 全部 require_admin，注释明言理由是「内容可经 rag_search 读
回」——而 **rag_search_command 本身（返回 chunk 文本）无任何门控**。多用户模式下任意
非 admin 会话可读全部已索引文档（含 admin 经任意路径导入的文件），让其它门控形同虚设
。**修复**：补 `require_admin` + SessionState（skipAuth 短路不影响单用户）；同病灶
`list_rag_docs`（泄露文件名/original_path 清单，与已门控的 rag_doc_search_paged 同
数据）一并门控。

### C2 [M] runtime 版本列表缓存永久钉死离线兜底列表
`*guard = Some(fetched.clone())` 无条件缓存——首次启动离线（或 nodejs.org 短暂不可达
）时 fetch 返回 fallback 内置列表并被 OnceLock 缓存到进程结束：之后联网也永远看不到
新版本。**修复**：fetch_* 改返回 `Option<Vec<String>>`（None=走了兜底），仅 Some 时
写缓存；None 保持缓存为空，下次调用重试。

### C3 [L] cancel_rag_git_pick 无门控可中断他人 clone
abort 信号按 canonical URL 键控，多用户下任何非 admin 可取消 admin 在飞的 clone（
temp 清理与 upload 的 map_temp_to_persistent 竞态）。**修复**：补 require_admin（与
pick/refresh 一致）。

### F1 [M] GroupCard 复制的端点 URL 未编码
`/mcp/${group.name}` 原样拼进复制按钮——含空格/CJK 的组名（如 `my tools`）复制出的
URL 所有客户端无法解析。代码里 clientConfigEndpoint 专门为此做了 encode，注释却说
「legacy Copy URL 有意保留原样」——坏 URL 不是风格选择。**修复**：三处端点全部
encodeURIComponent，clientConfigEndpoint 复用同一编码值。

### F2 [L] AccessUrlDialog 假「已复制」
execCommand('copy') 返回值丢弃——fallback 路径 copy 被拒时仍显示 ✓ 1.5 秒。**修复**：
捕获布尔值，false 时 error 日志 + 不置 copiedKey（clipboard.ts 共享 helper 本来就返
回布尔，本组件复制的 fallback 漏了检查）。

## 清洁
- runtime.rs 其余全对（char_indices PATH 截断、validate_version_arg 全调用点、per-
  runtime AsyncMutex 串行化、300MB 帽 + SHASUMS 校验、tar symlink/hardlink 逃逸含悬
  空目标、Windows zip mangled_name、全部失败路径的临时目录清理、CREATE_NO_WINDOW 12
  处全在）
- rag.rs 无路径穿越/panic 面
- serverFormPayload / serverDuplicate / clipboard / Dashboard 零缺陷
- AccessUrlDialog routingConfig 非空（SettingsContext 默认初始化）验证安全

## 新套件 rmcp_supplement_v32.py（10 项）
- 4 legacy 回显 + 公网 IP ×3（2025-03-26 / 2025-06-18 root + 2026 stateless，IPv4
  Body 断言）
- **CJK 服务器名 percent-encoded `/api/{name}/openapi.json` 端到端解析**（编码路径
  segment 与 bridge percent-decode 的闭环验证——F1 的协议面等价物）
- 2026 discover（ttlMs 3600000）+ /health

## 验证
- `cargo check --lib` 0 错 0 警；`cargo test --lib` 102 passed；cargo build ✓
- `tsc` 0 错误；`npm run build` ✓
- 36 套件 **1216/1216 全绿**（v32 10/10 新计入；重建二进制热跑）

## 教训
- **门控一致性必须按「数据面」而非「命令面」审计**：C1 的所有邻近命令都有门控且有正
  确注释，唯独真正吐内容的命令漏了——按命令逐个看门控像检查完毕，按「谁能读到什么数
  据」对账才暴露。
- 缓存语义：fallback 值（错误路径的产物）与正常值混进同一缓存 = 一次瞬时故障被放大成
  进程生命周期问题。错误产物永不入缓存。
