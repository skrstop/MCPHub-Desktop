# R491+ 复核第二十轮报告（2026-10-02）

> 范围：第 18/19 轮未覆盖的剩余代码面（rag/chunker+extract、rag/git.rs、services batch 2、commands batch 2、rmcp 传输层轮询路径），5 个独立逐行扫描代理 + 亲核修复 + 新套件 + 全量回归。

## 1. 修复清单（15 项）

### High

| # | 位置 | 问题 | 修复 |
| --- | --- | --- | --- |
| H1 | `rag/chunker.rs push_atoms` | AST 深度递归（40k 嵌套括号 ≈ 8 万层）→ 栈溢出 SIGABRT 进程崩溃 | 重写为迭代显式工作栈（`Work{Node,Gap}`），MAX_DEPTH=512 保护；新增测试 `deep_nesting_does_not_overflow_stack`（40k 括号） |
| H2 | `commands/skills.rs` | 15 个命令中 9 个写/删/导入/卸载命令无 `require_admin`（auth 模式下任意用户可改 agent 配置、删 skill） | 9 命令补门控（save/create/delete_agent、import、scan_folder、export、uninstall、delete、open_in_explorer） |
| H3 | `rag/git.rs temp_dir_for` | 首次 git 导入：clone 落在 per-attempt 目录 `{hash}-{32hex}`，persist 阶段只查 legacy `{hash}` → 上传必失败 | 重写 temp_dir_for 解析最新存在的 `{hash}` 或 `{hash}-{32hex}` 目录（mtime 最大） |

### Medium（10 项）

| # | 位置 | 问题 | 修复 |
| --- | --- | --- | --- |
| M1 | `rag/git.rs` 清理路径 | delete_doc 清理用原始 URL 哈希、clone 用规范化哈希 → 两个拼写同名仓库残留 clone | delete_doc 先 canonicalize 再哈希 |
| M2 | `rag/service.rs refresh_git_repo` | 刷新锁/TTL 按原始哈希，clone 按规范哈希 → 同仓库两个拼写并发 `.new` 目录竞态 | 锁/TTL 键统一规范化哈希 |
| M3 | `prompt_service create/update` | 重名 prompt 偷走他人 FTS 行（ref_id=name）+ get_prompt 按名 find 歧义 | 事务内 COUNT 重名拒绝（create 全局查；update 排除自身且改名时才查） |
| M4 | `resource_service create/update` | 同 M3（name/uri 双重键） | 同 M3 |
| M5 | `resource_service update` | URI 改名只通知新 URI，旧 URI 订阅者永久悬挂 | 变更时补发 `notify_resource_updated(old_uri)` |
| M6 | `user_service create/update_password` | 空/空白密码可入库（settings_import 导入外部 JSON 可触达） | trim 拒绝 |
| M7 | `commands/rag.rs` 5 个读命令 | get_rag_doc(_paged)/get_rag_chunks(_paged)/rag_doc_search_paged 为内容泄露通道，无门控 | 全部补 require_admin + SessionState |
| M8 | `mcp/openapi_transport.rs` | `load_openapi_spec` CPU 密集（$ref 格状爆炸）在 async worker 同步执行，阻塞其他请求 | `spawn_blocking` 包裹 |
| M9 | `rag/extract/ocr.rs` | `image::load_from_memory` 无 Limits → 解压炸弹（小 PNG 声明 gigapixel 画布）OOM | `decode_limited`（64MiB alloc + 12000px 边长），两处解码点接入 |
| M10 | `commands/registry.rs` + `cloud.rs` | registry/MCPRouter 响应 `resp.json()` 无界 → 恶意/异常响应撑爆内存 | 32MB content_length + 实际 bytes 双重上限 |

### Low（5 项）

| # | 位置 | 问题 | 修复 |
| --- | --- | --- | --- |
| L1 | `mcp/rmcp_stdio_transport.rs disconnect` | `service.cancel().await` 无界 → 卡死的 worker 挂住断连（进程树已 kill，无意义等待） | 5s timeout 包裹 + 超时 warn |
| L2 | stdio + http transport 轮询 | `poll_interval_ms.clamp(200, u64::MAX)` → 上游恶意 10min interval 单次 sleep 就越过 600s deadline | clamp(200, 30_000) |
| L3 | 同上 | deadline 检查在 sleep **前**，clamp 后 30s sleep 会越过 deadline | 检查移到 sleep 后 |
| L4 | `rag/git.rs` | `temp_repo_dir` 死函数（temp_dir_for 重写后遗留） | 删除 |
| L5 | `ocr.rs` | `image::io::Limits` deprecated 路径 | `image::Limits` |

## 2. 新套件

`scripts/e2e/rmcp_supplement_v20.py`（36 项）：
1. **group allow-list 强制执行**（Test 组 tools/list 成员边界 + 组外工具调用拒绝）
2. **$smart 端点**（initialize / tools/list 仅 meta 工具 / search_tools 真实检索）
3. **GET SSE server-push 流 × 5 版本**（200 SSE + keep-alive 输出 + 流开后 POST 仍通）
4. **并发 4 会话 UTF-8 scope 独立公网IP 调用**（线程级隔离）
5. **大 payload 边界**（9MiB body 非 5xx + 服务存活）
6. **CacheableResult 逐版本**（legacy 无 2026 hint 门控 / 2025-11-25 形状）
7. **prompts/resources round-trip**（2025-06-18 会话式 + 2026 discover，验证重名拒绝不破坏正常路径）

套件调试记录：groups 表成员是对象数组（含 tools 过滤），非字符串数组——member 解析需 `it["name"] if isinstance(it, dict)`；scope 通道暴露**裸工具名**（非前缀聚合），call 需按通道选择名称形态；中文 body 必须 utf-8 显式编码（latin-1 默认崩）。

## 3. 验证结果

| 项 | 结果 |
| --- | --- |
| cargo check --lib | 0 错 0 警 |
| cargo test --lib | **93 passed**（+chunker 深嵌套） |
| tsc | 0 错误 |
| **24 套 E2E** | **1078/1078 全绿**（重建二进制后全量重跑） |
| 新增套件 | v20 36/36 |

## 4. 经验沉淀（为什么每轮都能跑出问题）

1. **数据形状假设**：套件作者假想 `groups.servers` 是字符串数组，实际是对象数组——与 v19 相同 DB，不同作者重犯。套件也应跨轮复用 helper 库。
2. **通道语义不对称**：root 聚合带前缀、scope 裸名——同一工具两种名字形态，任何新套件都要重学。
3. **clamp 方向**：`clamp(200, u64::MAX)` 是「防太急」却没「防太慢」——防御式编码只防了一个方向。
4. **检查时机**：deadline 检查放循环头 = sleep 后必越界；放 sleep 后才是对的。轮询循环模板应固化「sleep → check deadline → work」。
5. **库 API 迁移**：image 0.25 `Limits` 从 `io::Limits` 提升；`ImageReader::limits(&mut self)` 不返回 Self，链式调用编译错——用 vendored 源码核对签名再写。
