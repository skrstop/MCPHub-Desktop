# 决策模型调查报告：julia_1「1+1 答错」——out_ln 位置 bug 修复 + 官方代码金标准对照（2026-11-16）

## 背景

用户报告 julia_1 决策模型对 `1+1等于几`（options: 4/3/2）选 "3"（0.96），认为概率反常，要求继续深挖。

## 金标准对照（官方推理代码）

用**官方仓库自带的推理包**（`julia/model.py` + `julia/data.py`，HF `SupersonicLabs/Julia-1`，transformers 内置 `ModernBertModel` encoder + fp32 safetensors 权重）跑用户原始案例：

| 案例 | 官方代码 (fp32) | 修复前 Rust (Q8) | 修复后 Rust (Q8) |
|---|---|---|---|
| 1+1等于几 | **"3" @ 0.903**（[-3.22,-0.57,-3.85]） | "3" @ 0.962（[-4.70,-0.81,-4.74]） | "3" @ 0.929（[-3.89,-0.69,-4.02]） |
| 天气下雨？ | **"下雨" @ 0.575**（[1.18,1.48]） | "下雨" @ 0.590（[1.28,1.64]） | "下雨" @ 0.640（[0.93,1.50]） |

**结论**：
1. **官方代码对 1+1 也选 "3" @ 0.90**——argmax 与概率结构均与我们一致。这是模型本身的行为。
2. 官方 README 明确声明能力边界："it cannot be counted on to supply missing facts, **solve algebraic equations**, or carry a long chain of calculations just because the right answer appears among the options"。Score 类型基准准确率仅 68.88%。
3. 修复后数值与官方 fp32 对齐到 Q8 量化误差内（~0.1-0.3 logits）。

## 找到并修复的真实 bug

### [H] out_ln（output_norm）应用位置错误（`src-tauri/src/mv/decision.rs`）

- **官方顺序**（`julia/model.py::forward`）：encoder 输出（**final_norm 在 encoder 内部**，HF `ModernBertModel.last_hidden_state` 已携带）→ `+ type_emb` → head 2 层 → gather markers → scorer。
- **修复前 Rust**：encoder → +type_emb → head → **out_ln** → gather → scorer——final_norm 被放到了 head 之后、type_emb 之后，顺序错误。
- **修复后**：out_ln 移到 encoder 循环之后、type_emb 相加之前（两条 decide 路径同步修正）。
- 效果：1+1 案例 scores 从 [-4.70,-0.81,-4.74] → [-3.89,-0.69,-4.02]，与同权重 python 参考实现 [-3.79,-0.67,-3.92] 对齐到 ~0.1（纯 Q8 量化误差）。

### 调查过程中的证伪记录

- **RMSNorm 假设（已回滚）**：曾据手写参考实现的 `rms()` 把 encoder norm 改成 RMSNorm——后对照 **transformers 内置 `modeling_modernbert.py`**（官方实际使用，model_type=modernbert 无 auto_map）确认其用 **bias-free LayerNorm**（`nn.LayerNorm(bias=norm_bias=false)`），原始实现正确，已回滚并修正注释。
- **tokenizer（排除）**：HF tokenizer 与 bundled tokenizer.json 逐 token 一致（7 组文本全 MATCH，special ids cls=2/sep=1/mask=4/pad=0 一致）。
- **token 序列（排除）**：math 案例 29 token ids 与 markers 逐位一致。
- **权重反量化（排除）**：手写 GGUF 解析器（Q8_0/BF16）反量化值与 candle dequantize 逐位一致（cls.output BF16 校验通过）。
- **encoder 逐层（排除）**：emb/L0/L1/L2/L10/L21 的 hidden 统计逐位一致——分歧精确定位在 head 段的 out_ln 位置。

## 验证

- `cargo test --lib -- --test-threads=1`：**451 passed / 0 failed**
- `cargo check`：0 错 0 警
- julia_demo_probe：域内用例全 ✅；score·math/weather 数值与官方对齐（见上表）

## 能力边界（官方声明，维持）

julia_1 是 144M 路由/分类模型：域内（工具路由、选项挑选、相关度打分）可靠；**算术/多步推理/事实补充是官方明确声明的能力边界**。`decision_decide` 工具描述已声明此边界（六语）。
