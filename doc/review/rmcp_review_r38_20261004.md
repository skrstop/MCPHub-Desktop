# rmcp 复核第三十八轮（R38，2026-10-04）：MRTR 畸形 inputRequests 死信箱 + RAG builtin 无超时 + port 0 旁路

> 复核范围：commands 8 小文件（tools/prompts/resources/smart_routing/cost/http_server/users/auth）；
> rmcp_bridge.rs 1681 行全量重读（R7 后经十余轮修复的最新状态）+ mcp/client.rs。
> 方法：侵蚀审计全在 → 2 个并行逐行代理 → 亲核验证 3 项 → 修复 → v38 套件 → 全量回归。

## 修复（3 项，全 Low 但真实）

### B1 [L] MRTR 畸形 inputRequests → 无可应答 InputRequired
守卫只挡 `input_requests.is_none() && request_state.is_none()`——上游发
`resultType: input_required` 带一个**反序列化失败**的 inputRequests（未知字段形状/新版
本变体）+ 可解析 requestState 时：`.ok()` 把畸形 payload 折叠成 None，绕过守卫 → 客户
端收到 request-less InputRequired，无法构造 inputResponses，交互死路（正是守卫注释要
防的）。**修复**：区分「键缺失」与「键存在但不可解析」——后者走同一 errored-Complete
回退。

### B2 [L] RAG builtin 是唯一无超时包装的执行路径
共享 pool（744/760 行）与 $smart meta（meta.rs:504）都包 timeout_tool_call，唯
call_builtin_tool 裸奔——本地模型调用挂死 / 大 reindex 持同锁时 /mcp 调用无限阻塞
（后台任务场景 stuck `working` 直到 TTL 兜底，无 ttl 则永久）。**修复**：包
timeout_tool_call 对齐。

### B3 [L] start_http_server 命令路径 port 0 旁路
R27 的 1-65535 校验只在两个 config 启动路径——命令直传 port 0 绑 OS 临时端口：返回
值/追踪 h.port 恒 0，loopback-hijack 检查对着 0 检测（该会话静默失效）。**修复**：
require_admin 后补 port==0 拒绝。

## 清洁
- tools.rs（call_tool 门控/禁用标记法/600s 超时/unwrap_or_default fail-close）、
  prompts/resources/smart_routing/cost/users/auth 八文件零缺陷（auth 的 skipAuth
  Err→true 维持既往已记录取舍）
- rmcp_bridge 历史疑区全部核过：apply_tool_filters 标记法（无隐藏路径）、
  mcp_tasks::authorized fail-closed、NATIVE_PEERS 剪枝 ABA 安全（dead_tokens 保 Arc 活
  过 unlock/relock 窗口）、$smart meta 自带超时
- mcp/client.rs 纯透传零缺陷

## 新套件 rmcp_supplement_v38.py（10 项）
- 4 legacy 回显 + 公网 IP ×2（2025-06-18 + 2026 stateless，IPv4 Body 断言）
- RAG builtin 列表/调用（RAG 未启用时 SKIP 对齐 v27 语义，非恒真占位）
- 2026 discover（ttlMs 3600000）+ /health

## 验证
- `cargo check --lib` 0 错 0 警；`cargo test --lib` 102 passed；cargo build ✓
- 42 套件 **1262/1262 全绿**（v38 10/10 新计入；重建二进制热跑）

## 教训
- **`.ok()` 折叠「存在但畸形」为「缺失」改变了守卫的语义**：Option 化错误处理时必须区
  分 None 的两种来源（B1 与 R36 迁移 skills 类型检查同族——都是「缺失」被拿来当万能分
  支）。
- 超时覆盖按「执行路径」枚举而非「我记得包过的」：三条执行路径（pool 隔离/共享/builtin
  +meta），枚举清单比对记忆可靠（B2 是全部 42 轮里最后一条裸奔路径——按路径枚举审计才
  找得到它）。
