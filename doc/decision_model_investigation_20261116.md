# 决策模型调查报告：julia_1「1+1 答错」+ RMSNorm 修复（2026-11-16）

## 背景

用户报告 julia_1 决策模型对 `1+1等于几`（options: 4/3/2）选 "3"（0.96），对天气案例选 "下雨"，认为反常；并提到用 jev（参考工具）测试 1+1 和 10+11 都能做对，质疑「模型能力边界」结论。

## 决定性对照实验

用官方 fp32 safetensors 权重（HF `SupersonicLabs/Julia-1`，snapshot 9c4e0c58）+ 官方参考实现（encoder RMSNorm + gated MLP + RoPE half-split + head LayerNorm + scorer LayerNorm）跑用户原始案例：

| 案例 | 官方 fp32 | 修复前 Rust (Q8, LN) | 修复后 Rust (Q8, RMS) |
|---|---|---|---|
| 1+1等于几 | **"3"** @ 0.762（[-5.77,-3.89,-5.72]） | "3" @ 0.962（[-4.70,-0.81,-4.74]） | "3" @ 0.836（[-6.59,-4.20,-6.47]） |
| 天气下雨？ | **"下雨"** @ 0.707（[0.26,1.14]） | "下雨" @ 0.590（[1.28,1.64]） | "下雨" @ 0.747（[0.20,1.29]） |

**结论**：
1. **官方模型本身对 1+1 就选 "3"**——argmax 与我们一致。这不是我们实现的 bug，是 144M ModernBERT+分类头模型（为工具路由/选项打分训练）的能力边界：算术/因果推断属域外噪声。
2. **修复前实现存在真实偏差**：encoder 内部归一化误用 LayerNorm（减均值），官方是 RMSNorm（不减均值）。修复后数值与官方 fp32 对齐到 Q8 量化误差内（天气案例偏差 0.3-0.5 → ~0.1）。
3. jev 能做对算术——用户确认 jev 仅为参考，以当前模型为准；jev 若能做算术，大概率不是这个 144M 分类头模型。

## 修复内容

### [H] encoder norm 类型：LayerNorm → RMSNorm（`src-tauri/src/mv/decision.rs`）

- `EncoderBlock.attn_norm`/`ffn_norm`、`emb_ln`、`out_ln` 改 `candle_nn::RmsNorm`（bias-free，eps=1e-5，与官方 config `norm_eps=1e-05` 一致）。
- **保持不变**（官方确为 LayerNorm）：head 的 norm1/norm2（`nn.TransformerEncoderLayer` 标准 LN 带 bias）、scorer.0（`nn.LayerNorm(384)` 带 bias）。
- 修正了此前「官方全是 LayerNorm」的错误注释（当时对照的参考实现与官方代码不符）。

### 已验证的其余前向细节（与官方一致，无需改动）

- RoPE：half-split（NeoX rotate_half）✓；rope_base 从 GGUF `modern-bert.rope.freq_base` 读到 160000（与官方 config rope_theta 一致，150000 缺省不适用）✓
- MLP gated：前半 gelu 激活 × 后半 gate ✓（hidden_activation=gelu 从 GGUF 读到）
- layer 0 无 attn_norm（identity）✓；attention scale 1/√head_dim ✓
- sequence() 输入构造逐字对齐（type question 前缀 / state 截断 / option `[:48]` / head budget 256）✓

## Campaign5 W1 复核修复（同批落盘）

| 严重度 | 文件 | 修复 |
|---|---|---|
| H | DecisionPage.tsx | loadPage 无错误处理（rejection 悬空）→ try/catch + loadError + mounted 守卫 |
| M | DecisionPage.tsx | 翻页无加载态 → pageLoading + 按钮 disabled |
| M | useCostData.ts | refetch inFlight 去重吞手动刷新 → 新增 refetchForce 绕过去重，Dashboard 手动刷新改用 |
| L | Dashboard.tsx | skeleton 第二行按 smartRoutingConfig?.enabled 渲染 1-2 格 |
| L | decisionFormat.test.ts | pageSlice 补 pageSize=0 与超长 fetch 用例（+2） |
| M | AGENTS.md | 验证计数 436/185 → 451/197 |
| M | decision_service.rs | cached_model 发布竞态：发布前重读 device 校验 |
| L | decision_service.rs | set_device 白名单 auto\|gpu\|cpu |
| H | decision_service.rs | env 测试竞态：parse_device_env 纯函数化 |
| M | decision_service.rs | round_trip 测试加 test_lock |

## 验证

- `cargo test --lib -- --test-threads=1`：**451 passed / 0 failed**
- `cargo check`：0 错 0 警
- `tsc --noEmit`：0 错误
- `vitest run`：**197 passed**（+2 pageSlice）
- julia_demo_probe：域内 4/4 断言通过；数值与官方 fp32 对齐（见上表）

## 能力边界（维持）

julia_1 是 144M 路由/分类模型：**域内**（工具路由、选项挑选、相关度打分）可靠；**域外**（算术、因果推断、事实知识）输出为噪声——`decision_decide` 工具描述已声明此边界（六语）。demo 探针 `mv::decision::tests::julia_demo_probe` 与 `doc/decision_demo_cases.json` 保留域内/域外标注用例。
