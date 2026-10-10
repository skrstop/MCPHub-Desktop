# OAuth 复核战役 第七轮（2026-10-09 晚，三代理：r6 修复自审 / http_server 全文 4521 行 / mv+models+小模块）

## 1. 修复清单（1H + 3M + 3L + 1I，含 1 项自修复引入回归的即时闭环）

### a1（第 6 轮修复自审）——最重要的发现
1. **[H] mark_conn_loss 字符串标记与 rmcp 3.4.1 实际错误文本不匹配 = 死代码** —— stdio 子进程死亡产生 `TransportClosed`（"Transport closed"），与 6 个标记全不匹配；"connection closed"/"send message error" 只在 initialize 期错误出现；"unexpected session state" 全 crate grep 零命中（真实变体是 "Unexpected response type"）；唯 broken pipe 可经内嵌 io 文本偶尔命中 → **改为按 ServiceError 变体判定**：TransportClosed/TransportSend/UnexpectedResponse 标记断连；McpError（服务器自己的 JSON-RPC 错误，消息文本常含 "connection closed"——健康连接会被误杀）与 Timeout（自家上限）不标记。教训：**修连接语义必须读 vendored 源码的 Display 实现，不能凭想象写错误串**。
2. **[M] HTTP 下游 MRTR 分支漏 mark_conn_loss** —— 同函数两个 call_tool_once 只挂了主路径 → 补齐。
3. **[M] McpError 文本误杀健康连接** —— 与 1 合并解决（变体判定天然排除 McpError）。
4. **[L] .meta 改名守卫在内容写入+重索引之后** —— 操作已生效才报错（无数据破坏但部分生效）→ 上提到 name sanitize 处（任何 fs 变异前）。
5. **[Info] pdf 括号深扫可被 512 个提前 `]` 绕过** —— 启发式缓解非保证，记录。
6. **[Info] reindex_all 部分成功混代** —— done>0 即清 flag，失败文档旧模型向量混入；记录 + 建议日志失败计数。

### a2（http_server 全文 4521 行首次全文扫）
7. **[M] refresh_token grant 无客户端认证/绑定** —— authorization_code 有 validate_client + client 绑定，refresh 全忽略：require_client_secret 客户端的 refresh token 被盗即可无 secret 兑换（RFC 6749 §6 要求 confidential 客户端在 refresh 也认证）→ refresh_owner 解析归属 + client_id 匹配 + validate_client。**即时闭环：修复自身引入 N7b 回归**（replayed token kind='refresh_used' 被 refresh_owner 判 unknown，提前 400 使家族吊销永不触发）→ refresh_owner 返回 (cid, replayed)，replayed 跳过认证直达 refresh_tokens 保住 §10.4 吊销语义；新增 N7c 用例（错误 client_id 400）36/36。
8. **[L] /mcp 静默忽略 jsonBodyLimit** —— SDK cap 取 max(snapshot, 64MB)、leniency 8MB floor：配置 1MB 时 /mcp 仍吞 8MB/64MB（rest_post_bearer 防线被最高流量面架空）→ 对齐 leniency 语义 max(8MB floor)。
9. **[L] POST /servers/:s/oauth/connect 鉴权前缓冲 body** —— Bytes extractor 先于 hub_admin_gate 执行，rest_post_bearer_middleware 只盖 /rest /api → 中间件扩盖 /servers/*/oauth/connect。
10. **[Info] Host 头反射进 OAuth issuer/metadata** —— 本地单用户可接受；若上共享代理需 pin issuer（记已知债）。
11. **[Info] 客户端 scopes 列存储但从不消费** —— 记录。

### a3（mv+models+小模块）
12. **[H] npx -p 包名采纳被首个位置参数覆盖（round-8/9/10 修复从未生效）** —— `-p foo@2 bar`：-p 分支 adopt 后 continue，下一迭代 "bar" 落入无条件 `pkg = Some(a)` break，覆盖 adopted 值 → 更新检查查错包、真实更新永不提示、无关包假角标 → `pkg_locked` 标志：adopt 后位置参数不再覆盖（含 `--` 分隔符分支同守卫）。
13. **[L] ensure_started 并发 waiter 失败后永久放弃** —— A 加载失败，B 硬报错不重试 → 记录（用户重试即恢复）。
14. **[L] 旧 *.gguf 残留被 find_gguf 按目录序误选** —— publish 不清理非本轮文件 → 记录。
15. **[I] open_external_url 校验 trimmed 但 spawn 原串** → spawn 用 &trimmed。

## 2. 验证
- cargo check 0 错 0 警；cargo test --lib **119 passed**（首轮 1 并行 flake 复跑过，AGENTS 已知模式）；tsc 0 错。
- **60 套 E2E 全绿**（新二进制全量；oauth_provider 升至 36 项含新 N7c）。

## 3. 复盘
1. **字符串匹配错误类型是最脆的修复形态**：round-6 的 mark_conn_loss 看似完备（6 个标记、有注释、编译过、测试绿——因为无套件覆盖「连接真死」），实际 5/6 标记是死串。**修复错误处理必须对着真实错误类型的定义写，变体匹配 > 字符串包含**。
2. **修 bug 的 bug 在第二轮就暴露**：refresh 认证修复引入 N7b 回归，被既有套件当场抓到（这是 36 项 oauth 套件的价值）——套件密度高的区域修复自愈快，盲区（连接死活）则靠 vendored 源码对照。
3. **「修过三次」≠「修对了」**：npx -p 修了 8/9/10 三轮仍在（每轮都只修了「=形式死分支」一类表象，没做完整参数流 trace）——对同一点第三轮以上修复，要求做端到端 trace 而非补丁叠加。
4. 收敛态势：http_server 4521 行首次全文扫仅 1M+2L（OAuth 路由层全部清洁）；mv/models/小模块 1H（三年老 bug 非新引入）——存量核心面收敛持续，缺陷集中在「语义层契约」（错误分类/认证边界）。
