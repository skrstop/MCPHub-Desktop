# R491+ 复核第二十五轮报告（2026-10-03）

> 范围：修复侵蚀审计（R19-R24 全部修复在位性验证）+ market/cost + prompt/resource 服务全文 + SettingsPage 全文（4524 行）。3 个独立复核代理 + 亲核修复。

## 1. 修复侵蚀审计（新工具类检查）

29 项 R19-R24 修复签名逐一 grep 在位验证：**29/29 全在位**（9 个初判 MISS 全为脚本 `\|` 转义伪象，逐个复核确认）。并发 IDE 编辑环境下，这应成为每轮固定开头步骤。

## 2. 修复清单（7 项）

| # | 位置 | 问题 | 修复 |
| --- | --- | --- | --- |
| M1 | `prompt_service.rs render_template` | 模板注入：顺序 `String::replace` 让参数值展开其他占位符（`{"a":"{{b}}","b":"secret"}` → a 的值被 b 再替换）+ HashMap 迭代序致结果不确定 | 重写为单遍扫描（每个 `{{name}}` 恰替换一次，替换文本不再扫描）；首版实现踩了 `end >= 4` 守卫错（`{{a}}` 的 `}}` 在 index 3，单字符占位符全落 verbatim 分支）——单测抓到后修为 `end >= 3` |
| M2 | `prompt_service` get 路径 ×2 | required 参数从不校验——缺失时 `{{placeholder}}` 字面量直接进 LLM 内容 | `validate_required_args` + bridge get_prompt / call_builtin_prompt 两处接入（invalid_params） |
| L3 | prompt/resource LIKE fallback | `\` 未自转义（声明了 ESCAPE '\\' 却只转义 %/_）——搜含反斜杠的名字永不命中 | 链首补 `replace('\\', "\\\\")` ×2 |
| M4 | `SettingsPage.tsx:1772` | 导出 JSON 快照陈旧——首次打开后永不刷新，Copy/Download 供过期配置 | 每次打开 section 重新 fetch |
| M5 | `SettingsPage.tsx` BearerKeyRow | 保存失败仍关闭编辑器——updateBearerKey 失败返回 null 被吞，用户输入丢弃 | 失败 throw，行组件仅在成功时退出编辑 |
| M6 | 同页 Smart Routing | 同节立即写开关（progressiveDisclosure 等）触发 saved 对象身份变更 → useEffect 无条件重建 temp → 未保存草稿静默清空 | `smartRoutingDirtyRef` 守卫：草稿态跳过重建，Save 成功后清 flag |
| — | 套件 v25 | 首版断言用 root 会话打 prompts（无影响）+ required 分支在真实库无数据（prompt 无 required 参数）→ 诚实 SKIP 桶指向 Rust 单测 | — |

## 3. 证伪（代理疑点亲核排除）

market/cost 全净（只读 catalog、无安装流、cost 无溢出面）；FTS desync（四写者全同事务）；enabled 读路径全过滤；二进制 content 无 TEXT 污染路径；分页稳定性；mcpStrictValidation 双写键与深合并兼容；各节保存互不覆盖（按命名空间 PUT）；隐藏模块桌面零写入。

## 4. 新套件

`scripts/e2e/rmcp_supplement_v25.py`（4 项）——builtin prompts 端到端：list 形状 + required 透传、填参渲染无占位残留、值含 `{{token}}` 不崩（交叉展开由 Rust 单测精确覆盖：`render_single_pass_no_cross_expansion` 等 5 个新单测）、缺 required → invalid_params。

## 5. 验证结果

| 项 | 结果 |
| --- | --- |
| cargo check --lib | 0 错 0 警 |
| cargo test --lib | **101 passed**（+5 prompt 渲染单测） |
| tsc / npm build | 0 错 / 通过 |
| **29 套 E2E** | **1152/1152 全绿**（重建二进制重启后全量重跑） |

## 6. 经验沉淀

1. **修复侵蚀是真实风险**：并发编辑环境下已修问题可能被回退——「每轮开头 grep 全部历史修复签名」30 秒换回滚保护，应固化。
2. **边界条件要用最小用例验证实现**：单遍扫描器的 `end >= 4` 看似合理，`{{a}}`（最常见形态）即崩——新实现必须配最小输入单测再接入调用方。
3. **快照型 UI 状态**（导出 JSON）默认值即陈旧源——任何 `if (!state)` 守卫的 fetch 都该问「数据会过期吗」。
