# rmcp 复核第二十八轮（R28，2026-10-03）：rag git 规范化哈希 + updater 取消竞态

> 复核范围：rag/service.rs（git 生命周期 delta）、rag/git.rs（持久化）、commands/updater.rs。
> 方法：2 个并行逐行代理 + 亲核验证全部 4 项 + 12 项证伪记录 + 新套件 + 全量回归。

## 修复（4 项，全部 Medium）

### G1 [M] delete-time git 引用计数按 RAW url 比较 → 可误删在用 clone/凭证
`count_git_docs_for_repo` 逐 meta 比较 `g.url == repo_url`，而 clone/credential 工件以
canonical hash 为键。同一 repo 两种拼写（http→https 重定向）导入的多份文档：删除拼写 A
的最后一份文档时计数归零 → `remove_persistent` 删掉仍在被拼写 B 文档使用的 clone + 凭证。
**修复**：调用方先 `canonicalize_url` 再计数；计数函数改为两遍法——先收集全部 git url
（await 外提）、每 url canonicalize 一次缓存、再按 canonical hash 相等计数。

### G2 [M] TTL 强制刷新 + 错误清除按 RAW hash 计算键 → 静默 no-op
`refresh_source_update`/`preview_source_update` 的 TTL bypass 与
`clear_git_source_error_for` 均用 `repo_hash(&url)` 原样计算，而
`GIT_REFRESH_CACHE`/`git_refresh_errors` 以 canonical hash 为键（service.rs:384/402/433
）→ 显式刷新按钮不重新 clone、错误横幅清除无效。**修复**：三处全部改为
`repo_hash(&canonicalize_url(url).await)`。

### G3 [M] ensure_persisted 非原子 copy → 崩溃残留部分 clone 永久短路
`copy_dir_recursive` 直接拷到最终 `{hash}` 目录，`dst.exists()` 短路幂等——拷贝中途崩溃
留下缺 `.git` 的部分目录，此后每次 ensure_persisted 直接返回坏 clone，source-sync 判
"original lost" 批量删文档。**修复**：拷到 `{hash}.new` staging 后 rename 原子入位（失败
清 staging）；`sweep_stale_dirs` 补扫缺 `.git` 的 64 位 hex 目录（pre-atomicity 残留自愈
）。

### G4 [M] updater cancel 竞态：锁前读 install_id + 前序尝试无 terminal 事件
`cancel_update_install` 在 slot mutex 之前读 `CURRENT_INSTALL_ID`——与并发
`install_update_cancelable` 交错时给错误尝试打 "cancelled" 标记，新尝试 B 永远收不到
terminal 事件（对话框卡在 installing）。且 `install_update_cancelable` abort 前序尝试 A
时不发任何事件——A 的对话框同样悬挂。**修复**：①cancel 改为 slot lock 之后再读 id；
②install_update_cancelable abort 前序时捕获旧 id 并补发 `updater://install-result`
cancelled 事件（在其 id 被覆盖前）。

## 证伪（12 项，代理报假 / 有意设计）
updater TempPath RAII 无泄漏；logger 线程 panic 源不触达；tool_config update race 实为
UNIQUE 约束正常报错；tool filter list/call 一致；clone attempt 目录 uuid 无碰撞；fresh
pick 清 temp root 不误删；credential RwLock+tmp+rename 无损坏窗口；refresh 锁释放正确
；crafted-repo unwrap 均有前置校验；partial attempt dirs 无消费者；rag/git canonicalize
redirect 已剥凭证（本轮仅键不一致，非泄漏）。

## 新套件 rmcp_supplement_v28.py（13 项）
- 5 版本 × root 通道 initialize 版本回显逐版本断言（4 legacy 精确回显 + initialized 202）
- 2026 stateless discover（resultType:complete + supportedVersions×5 + ttlMs 3600000/public）
- 真实公网 IP MCP 调用（本机公网ip查询-getPublicIp，Response Body 断言 IPv4）
- 宽松模式：缺 Accept 头 2 版本放行、缺 jsonrpc 键归一放行
- /health 存活

## 验证
- `cargo check --lib` 0 错 0 警；`cargo test --lib` 102 passed；cargo build ✓
- 32 套件 **1180/1180 全绿**（v28 13/13 新计入；重建二进制后热跑）
- 公网 IP 断言：58.211.44.178 真实返回
