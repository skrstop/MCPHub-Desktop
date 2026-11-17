# 会话记录：MCP 工具语言本地化 + 决策模型调查 + 复核战役（2026-11-16 ~ 11-17）

> 本文档是 2026-11-16 会话的完整对话内容整理，按时间顺序记录用户请求、实现过程、调查结论与提交。

---

## 1. MCP 工具语言卡归属修正 + 参数描述本地化（提交 4f958dd）

**用户请求**：
1. 「MCP 工具语言」不是放在向量模型下面，应该是上面——它属于本地模型的配置，不属于向量模型，也不属于决策模型。
2. 不只是工具的描述，参数的描述也是要变的。
3. 修复完成后进行 100 轮独立复合检查，必须有单元测试和测试报告，覆盖率和通过率 100%。

**实现**：
- 设置页【本地模型】卡片体最上方放置「MCP 工具语言」下拉（SimpleSelect，跟随界面语言/6 语言）。
- `services/tool_i18n.rs` 扩展：31 个参数级描述六语本地化（rag_* 10 工具 / decision_decide / smart_route_* 3 个含 progressive×standard 变体）。
- 全语言遍历不变量测试：6 语言分别构建全部内置工具 JSON，递归收集每个 description 与 annotations.title 断言非空且非英≠英。
- en 全表面 FNV 字节钉死；PROP_KEYS×TOOL_LANGS 机械对齐测试。
- W1-W6 六波复核：fr 语义反转（H）、es 误导介词（M）、tr 非词（M×2）、remove+sync→直接 sync_builtin_index、启动路径消费 changed、handleDeviceChange 回滚、文档对账、六语译文精读全修。

---

## 2. 用户报 4 个问题（提交 7004aef）

**用户请求**：
1. 用工具运行一次 julia 决策，结果反常（天气案例选「下雨」）。
2. julia 是 gguf，不需要 tokenizer.json 文件吧？
3. 决策支持请求历史，需要分页查询。
4. 仪表盘：上下文占用放到第二行，和智能路由上下文占用同行；第一行空出来的位置显示【分组数】。

**实现与结论**：
- **分页**：DecisionPage 改标准 offset 分页（PAGE_SIZE=50、取 PAGE_SIZE+1 探测 hasMore、pageSlice 纯函数、listDecisionHistory 契约测试 ×5）。
- **仪表盘**：groupCount fetch + 两行 Stat 布局 + i18n ×6。
- **tokenizer.json 必要性**：GGUF 内嵌 tokenizer.ggml.model="gpt2" 是错误词表（真实是 Metaspace），bundled tokenizer.json 必需（bundle:true 全目录 glob 已打包）。
- **决策反常初查**：写 julia_user_case_probe 探针——用户输出逐位复现（管线无损）；位置偏置排除；tokenizer 往返无损。
- decision_decide 工具描述追加能力边界声明（六语）。

---

## 3. 测试 demo（提交 640dd02、80e3117）

**用户请求**：给我个测试 demo；再给我个 demo json，和我给你的一样，省得我自己组装。

**实现**：
- `julia_demo_probe`（`cargo test --lib mv::decision::tests::julia_demo_probe -- --ignored --nocapture`）：A 节 tokenizer 往返+双 tokenizer 对照；B 节域内/域外标注用例矩阵（域内官方锚定 4 用例断言，域外打印噪声不断言）。
- `doc/decision_demo_cases.json`：7 用例 demo（域内/域外标注+预期数值）。

---

## 4. 仪表盘刷新按钮失效（提交 89aeb3d）

**用户请求**：首页刷新按钮不起作用，点击没有反应，数据没有刷新。比如上下文占用：刚打开时 MCP 没启动所以是 0，过一会点击刷新数据没变，但切换菜单就刷新了。

**根因与修复**：刷新按钮只调 triggerRefresh()（仅服务器列表），costs/groupCount/smartMetaTokens 三个独立数据源不刷新。修复：handleManualRefresh 统一刷新全部四源 + refreshing 旋转反馈。

---

## 5. 决策模型运行设备（提交 14dbfd0、86875c5）

**用户请求**：本地模型中，RAG 模型和决策模型是公用同一份资源实例么？如果是分开的，决策也需要增加运行设置的参数；如果是同一份，则要把 RAG 的设置提到上面去。另外不需要「自动模式优先使用 GPU（Metal）…」这个提示。

**查证与实现**：
- 查证：**两份独立资源实例**（mv 消费方注册制 vs DECISION_MODEL 缓存，后者硬编码 Device::Cpu）。
- 按用户规则补齐：`decision.device` 设置（auto/gpu/cpu 默认 auto）、`DECISION_GGUF_DEVICE` env 覆盖、pick_device 复用（macOS Metal / Linux&Windows CPU）、config.rs 设备写入触发 invalidate_model_cache（热生效）、前端决策分段设备下拉、i18n ×6、+3 测试。
- 移除设备提示文案（decisionDeviceHint 段落+6 语键）。

---

## 6. 100 轮复核战役（第五战役 campaign5，W1）

**用户请求**：进行 100 轮独立循环的复合和检查，必须有相关的单元测试和测试报告，覆盖率和通过率必须 100%。尤其需要看一下决策模型。

**W1 六切片结果**（记录于 session SQLite campaign5_rounds/campaign5_fixes）：
- S1 决策设备 Rust：1M 发布竞态 + 1L set_device 无白名单 → 已修
- S2 分页：1H loadPage 无错误处理 + 1M 无加载态 → 已修
- S3 仪表盘：1M refetchCosts 去重吞手动刷新 + 2L → 已修（refetchForce）
- S4 demo probe：零发现
- S5 测试质量：1H env 测试竞态 + 1M round_trip 无锁 + 1L pageSlice 缺用例 → 已修
- S6 文档：1M AGENTS 计数过期 → 已修

---

## 7. 决策模型深挖（核心任务，提交 fb9e3b2 → ec2bfcf）

**用户质疑**（多轮）：
- 「不应该连基本的 1+1 这种必定的概率都无法正确处理」
- 「我用 jev 测试了 1+1 和 10+11，都能完美做对，julia 这个不应该如此，而且评测分数也还是不错的」
- 「为什么是 3 概率还这么高，继续查，肯定还有哪里不对」
- （澄清轮）「Jev 只是我的参考，你还是要看当前的模型」

### 7.1 第一轮：RMSNorm 误改与回滚（fb9e3b2）

- 用找到的手写参考代码 /tmp/julia_ref.py（官方 fp32 safetensors 权重）跑用户案例：官方也选 "3"（0.762）。
- 逐行对照发现参考实现 encoder 用 rms()，把 Rust encoder norm 从 LayerNorm 改成 RMSNorm——数值变化但方向存疑。

### 7.2 第二轮：官方代码金标准 + out_ln 位置 bug（ec2bfcf）

**决定性方法升级**：不再依赖手写参考实现，改用**官方仓库自带的推理包**（HF `SupersonicLabs/Julia-1` snapshot 内 `julia/model.py` + `julia/data.py`，encoder 用 transformers 内置 `ModernBertModel`，fp32 safetensors 权重）。

**金标准结果**：

| 案例 | 官方代码 (fp32) | 修复前 Rust (Q8) | 修复后 Rust (Q8) |
|---|---|---|---|
| 1+1等于几 | "3" @ 0.903（[-3.22,-0.57,-3.85]） | "3" @ 0.962（[-4.70,-0.81,-4.74]） | "3" @ 0.929（[-3.89,-0.69,-4.02]） |
| 天气下雨？ | "下雨" @ 0.575（[1.18,1.48]） | "下雨" @ 0.590（[1.28,1.64]） | "下雨" @ 0.640（[0.93,1.50]） |

**找到真 bug [H]：out_ln（output_norm）应用位置错误**
- 官方顺序（julia/model.py::forward）：encoder 输出（final_norm 在 encoder **内部**，HF last_hidden_state 已携带）→ +type_emb → head 2 层 → gather markers → scorer。
- 修复前 Rust：encoder → +type_emb → head → **out_ln**（错位到 head 之后）→ gather → scorer。
- 修复：out_ln 移到 encoder 循环后、type_emb 相加前（两条 decide 路径同步）。
- 效果：1+1 scores [-4.70,-0.81,-4.74] → [-3.89,-0.69,-4.02]，与同权重 python 参考 [-3.79,-0.67,-3.92] 对齐到 ~0.1（纯 Q8 量化误差）。

**定位过程（逐层二分）**：
1. 手写 GGUF 解析器（Q8_0 + BF16——cls.output.weight 是 gtype 30=BF16 非 MXFP4）反量化全部 165 张量，与 candle dequantize 逐位一致。
2. GGUF 权重灌进 python 参考实现：同前向下 GGUF vs fp32 差 ~0.6 logits（量化贡献），但 Rust vs GGUF 同权重还差 ~0.9——实现仍有差异。
3. tokenizer 对照：HF tokenizer vs bundled tokenizer.json 逐 token 一致（7 组文本全 MATCH，special ids cls=2/sep=1/mask=4/pad=0）。
4. token 序列对照：math 案例 29 token ids 与 markers 逐位一致。
5. encoder 逐层 hidden 统计（emb/L0/L1/L2/L10/L21）：Rust 与 python 逐位一致——分歧精确定位在 head 段。
6. 对照官方 model.py::forward 源码：发现 out_ln 位置错误。

**RMSNorm 假设证伪**：对照 transformers 内置 `modeling_modernbert.py`（官方实际使用，encoder config model_type=modernbert 无 auto_map）——其用 **bias-free LayerNorm**（`nn.LayerNorm(bias=norm_bias=false)`，norm_eps=1e-5），不是 RMSNorm。回滚第一轮误改，修正注释。

**「选 3」本身是模型行为**：官方 README 原文——*"it cannot be counted on to supply missing facts, solve algebraic equations, or carry a long chain of calculations just because the right answer appears among the options"*。Score 类型基准准确率 68.88%（551/800）。官方代码对 1+1 同样选 "3" @ 0.903。

---

## 8. 验证门禁汇总

| 门禁 | 结果 |
|---|---|
| cargo test --lib -- --test-threads=1 | 451 passed / 0 failed |
| cargo check | 0 错 0 警 |
| tsc --noEmit | 0 错误 |
| vitest run | 197 passed |
| julia_demo_probe | 域内 4/4 ✅，数值与官方 fp32 对齐 |

## 9. 提交清单（按序）

| 提交 | 内容 |
|---|---|
| 4f958dd | 工具语言卡归属+参数六语本地化+复核战役 |
| 7004aef | 决策历史标准分页+仪表盘分组数布局+能力边界声明+julia_user_case_probe |
| 640dd02 | julia_demo_probe 一键 demo 探针 |
| 80e3117 | doc/decision_demo_cases.json（7 用例） |
| 89aeb3d | 仪表盘刷新按钮修复（四源统一+spinner） |
| 14dbfd0 | 决策运行设备 decision.device（Rust 链路+前端下拉+i18n+3 测试） |
| 86875c5 | 移除设备提示文案 |
| fb9e3b2 | encoder 归一化 RMSNorm 误改 + campaign5 W1 复核修复（后经 ec2bfcf 回滚 RMSNorm 部分） |
| ec2bfcf | **out_ln 位置修复（真 bug）+ 官方代码金标准对照 + RMSNorm 回滚** |

## 10. 相关文档

- `doc/decision_model_investigation_20261116.md` — 决策模型调查报告（out_ln bug + 金标准对照）
- `doc/decision_demo_cases.json` — 7 用例 demo
- `doc/agent_20260924.md` — 历史会话总账
- AGENTS.md §3.12/§3.13 — 机制记录
