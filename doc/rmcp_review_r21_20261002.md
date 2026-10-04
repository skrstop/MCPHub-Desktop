# R491+ 复核第二十一轮报告（2026-10-02）

> 范围：横切面审计——迁移一致性、IPC 契约、i18n 完整性、lifecycle（tray/lib）、OpenAPI/REST 鉴权面、smart_routing/mv 抽查。本轮后台复核代理连续空返回（基建抖动），全部改由主线脚本化交叉审计 + 亲自阅读完成。

## 1. 审计方法（脚本化，可复跑）

| 审计 | 方法 | 结果 |
| --- | --- | --- |
| 迁移一致性 | python 扫 migration.rs：migrate_vN 定义集 vs apply_migration 分支集 vs 00NN_*.sql 文件集 + 幂等扫描 | 26/26/26 对齐；**v12 裸 `.ok()` 漏网**（R19 只修了 v2-v11）→ 已修 |
| IPC 契约 | tauriClient invoke/路由表 vs generate_handler 注册表双向 diff | 160 注册命令中 3 个前端零引用（get_active_node_version / get_active_python_version / stop_http_server——死面，无孤儿 invoke，记录不修） |
| i18n | 4 locale 递归键集 diff + 前端 `t("...")` 键 vs locale 全集 | **26 键缺失**（见下）→ 已修；修后 4 语言零差异 |
| 鉴权面 | 16 个 /rest + /api handler 首 3KB 是否含 check_bearer_auth | 16/16 全门控（scoped_post 经 execute_openapi_impl 委托，实核通过） |
| lifecycle | tray.rs/lib.rs unwrap/expect 扫描 + quit 路径 + CloseRequested | 全部正确（custom quit + disconnect_all + import cancel + hide） |
| smart/mv | unwrap 扫描（非 test）+ scrypt 参数 vs 注释 + spawn 点 | 非测试 unwrap 仅 2 处静态参数 expect（安全）；scrypt N=2048(log2=11) 与注释一致 |

## 2. 修复清单

| # | 位置 | 问题 | 修复 |
| --- | --- | --- | --- |
| F1 | `db/migration.rs migrate_v12` | 裸 `.ok()` 吞 ALTER 错误（R19 修复漏掉 v12）——列已存在之外的失败（如磁盘满/锁）会静默通过，标记 v12 已迁移但列缺失 → 后续 SQL 全挂 | 换 `add_column_if_missing` |
| F2 | locales ×4 | 26 键缺失，回落硬编码中文：`accessUrl.*` ×8（AccessUrlDialog 整个命名空间无英文/法/土翻译）、`errors.failedToUpdate*` ×7 + `failedToReloadServer`（zh 在 api.errors 下错位）、`pages.rag.scanPhase*` ×7、`server.unknownError`、`users.fetchError`、`auth.user`、`settings.llmProviderApiKeyDescription`（fr/tr） | 全部补齐 4 语言（en/zh/fr/tr），修后递归 diff 零差异 |
| F3 | 套件 | v21 新套件调试中发现：`/rest/{server}/call` body 形状为 `{tool, arguments}`（与 /api/tools/{s}/{t} 的纯 arguments 不同）——套件初版用错形状得 422 | 套件修正（非产品缺陷，形状本就如此，origin 同构） |

## 3. 新套件

`scripts/e2e/rmcp_supplement_v21.py`（26 项）——REST/OpenAPI 面 bearer 矩阵：
- bearer-off：/rest/{server}/tools+call（openapi 上游真实公网IP）、/rest/group/tools+call、/api/openapi.json+servers+stats、/api/{中文名}/openapi.json、/api/tools/{s}/{t} GET+POST（global+scoped）、未知工具 404 语义
- bearer-on：无 token /rest 与 /api 均 401；带 key 上述全部通道真实公网IP 调用全通

## 4. 验证结果

| 项 | 结果 |
| --- | --- |
| cargo check --lib | 0 错 0 警 |
| cargo test --lib | **93 passed** |
| tsc | 0 错误 |
| locales JSON | 4 语言解析通过、递归键集零差异 |
| **25 套 E2E** | **1104/1104 全绿**（重建二进制重启后全量重跑） |

## 5. 经验沉淀

1. **逐点修复漏同型邻点**：R19 修「早期迁移裸 .ok()」时列出了 v2-v11，v12 恰好越过边界——同类修复必须用脚本枚举全量而非目测清单。
2. **i18n 无 CI 守护**：键靠人肉维护，建议加 locales 递归 diff 的 CI 步骤（本轮脚本可复用为 lint）。
3. **后台代理空返回**（本轮 5/5 无输出）：基建抖动时主线脚本化审计（grep 矩阵 + 双向 diff）是可靠降级路径，且更快。
