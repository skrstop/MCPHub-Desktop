# rmcp 复核第三十七轮（R37，2026-10-04）：skill import 绕过文件锁（High）+ LIKE 反斜杠漏网第三处

> 复核范围：prompt_service / resource_service / skill_service 全量重读；
> tauriClient（1490 行重读）/ fetchInterceptor / types 契约对账。
> 方法：侵蚀审计全在 → 2 个并行逐行代理 → 亲核验证 5 项 → 修复 3 项 → v37 套件 → 全量回归。

## 修复（3 项：High×1，Medium×1，Low×1）

### K1 [H] import_skills 未获取 skill_fs_lock——并发导入互毁
export/uninstall/delete 全部持锁（锁的 doc 注释明言防「并发 remove_link vs
copy_dir_recursive」），唯独 import_skills 对同一棵树做同样的 remove/copy 却不持锁：
- import vs import 同 dir_name：双双通过 exists() 检查 → 双 INSERT → 双
  copy_dir_recursive 同 dst——copy_dir_inner 是**先删再拷**，第二次拷贝的删除阶段毁掉第
  一次的半成品树，双双标 ok（目录损坏 + FTS ref_id 偷行）；
- import vs export 同 dir_name：导出拷到截断树并标 ok。
**修复**：import_skills 顶部获取 skill_fs_lock()（exists 复查在锁内自然闭合双插窗口）。

### K2 [M] skill LIKE 漏网反斜杠转义（第 3 处同类）
`key.replace('%','\\%').replace('_','\\_')` 缺第一步 `replace('\\','\\\\')`——搜索
`foo\bar` 把 `\b` 当转义字面量错配，尾随 `\` 直接吃掉 `%` 通配符零命中。prompt/resource
两处早修了，skill 这处漏网。**修复**：补反斜杠优先转义（顺序敏感）。

### T1 [L] optionCommands 列表不全——null 结果伪装成功
null→`{success:true}` 只特判 get_server/get_market_server——get_rag_doc（paged）/
get_builtin_prompt/get_builtin_resource 同为 `Result<Option<T>>`，陈旧列表打开详情时
拿到无 data 的「成功」渲染空对话框而非「已不存在」错误。**修复**：列表补 4 项。

## 记录不修 / 重报证伪
- **oauth 前端收集后 Rust 无落点**：R10 已记录「OAuth2 字段 Rust 无落点（UI 收集即丢，
  需产品决策）」——代理以「前端注释承诺 round-trip」重报。维持产品决策待办，不加
  `Option<Value>` 透传字段（半成品存储比明确不存储更误导）。
- **prompt/resource 重名拒绝 check-then-insert TOCTOU**：需 v27 迁移加 UNIQUE 索引——
  存量数据可能已有重名（索引建失败 = 启动崩溃），风险大于收益。**记录为已知债**，现
  有 COUNT 预检查窗口极窄（FTS 偷行可由 rebuild 自愈）。

## 清洁
- render_template 单遍扫描器（替换值不再扫描/{{}}/未知 token 分支）、validate_required_args
- valid_dir_name 在全部 fs 变更 join 点在场（其余 join 均为只读 exists 检查）
- copy_dir_inner visited 集合 DAG 安全、agents 锁全覆盖、Windows 提权流自洽
- fetchInterceptor（WeakMap 引用计数按钮状态）、tauriClient 路由映射（source-update
  segs 分支/registry 版本路由/change_password 签名/headers HashMap/passthroughHeaders
  Vec）逐项对账无误

## 新套件 rmcp_supplement_v37.py（8 项）
- 4 legacy 回显 + 公网 IP ×2（2025-03-26 + 2026 stateless，IPv4 Body 断言）
- 2026 discover（ttlMs 3600000）+ /health

## 验证
- `cargo check --lib` 0 错 0 警；`cargo test --lib` 102 passed；cargo build ✓
- `tsc` 0 错误；`npm run build` ✓
- 41 套件 **1260/1260 全绿**（v37 8/8 新计入；重建二进制热跑）

## 教训
- **锁契约按「操作树」而非「命令」审计**：K1 的所有兄弟命令都持锁且锁注释写明动机，唯
  一新加的 import 漏了——与 R32 rag_search 同模式（门控/锁的一致性按数据面对账）。
- LIKE 转义链是三段式（反斜杠→%→_），漏一段就错配——同链条在仓内已有 N 处正确实现时
  ，新实现必须逐字符对照现范本。
- 前端注释承诺的 round-trip 与 Rust serde 实际行为之间需要显式对账（R23 曾为此写过
  check_frontend_rust_contract.py，本轮该工具价值再次确认）。
