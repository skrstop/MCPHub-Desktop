# 嵌入模型中英文跨语言检索评测（2026-11-12）

> 探针：`cargo test --lib mv::gguf_modernbert::tests::f2llm_zh_crosslingual_probe -- --ignored --nocapture`
> 场景复现用户实测：「如何查询公网ip」查不到 getPublicIp 工具（描述为英文）。

## 正向对（相关，越高越好）与负向对（无关，越低越好）

| 模型 | zh查询↔en描述 (pair0) | en查询↔en描述 (pair1) | zh↔zh服务器行 (pair2) | **区分度 pair0−neg0** | 负向最高 (neg) |
|---|---|---|---|---|---|
| **F2LLM v2-160M** | **0.617** | 0.810 | 0.829 | **+0.57** | 0.05 |
| F2LLM v2-80M（当前默认） | 0.488 | 0.588 | 0.628~0.871 | +0.27 | 0.26 |
| bekko v1-a25m | 0.416 | 0.882 | 0.854 | +0.36 | 0.30 |
| bekko v1-a8m | 0.425 | 0.787 | 0.853 | +0.23 | 0.31 |
| granite 311m-r2 | 0.920 | 0.965 | 0.970 | +0.14 | 0.78 ⚠️ |
| granite 97m-r2 | 0.819 | 0.929 | 0.957 | +0.15~0.19 | 0.63~0.67 ⚠️ |
| harrier oss-v1-270m | 0.448 | 0.450 | 0.565 | −0.10 | 0.55 |
| F2LLM v2-330M | **已修复加载**（`decoder_embed` arch 别名 + 键/张量名重映射 + 官方 tokenizer.json），但检索质量差：zh↔enTool 仅 0.254（带前缀）/0.169（裸），en↔enTool 0.393/0.171——连英文查询都过不了 0.5 阈值；zh↔zh 0.72 说明中文内部尚可、跨语言与英文对齐都弱。**不可用** | 0.254 | 0.393 | ❌ | |

## 结论

1. **F2LLM v2-160M 是唯一「中文查询能过 0.5 阈值 + 区分度极大」的模型**：
   - zh 查询↔en 工具描述 0.617（>0.5 阈值，能命中）
   - 无关对仅 0.03~0.05（区分度 +0.57，检索精度极高）
   - 自相似 0.955（带前缀）/1.0（裸文本），编码器健康
2. **当前默认 v2-80M 中文能力不足**：zh↔en 仅 0.488，过不了 0.5 阈值（与用户实测 0 命中一致）；负向 0.26 也偏高。
3. **granite 两款是「假高分」**：无关对 0.63~0.78，各向异性严重——所有文本都打 0.8+，看似跨语言最强实则无区分度，做检索会大量误召回。
4. **bekko**：en 查询很强（0.88）但 zh 查询↔en 描述 0.42 过不了阈值；zh↔zh 0.85 说明中文内部对齐好、跨语言弱（偏日语优化）。
5. **harrier**：全面最弱，zh 查询比无关对得分还低（负区分度）。
6. **v2-330M 加载失败**：GGUF 未内嵌可用 tokenizer，需补 tokenizer.json 才能用（单独问题）。

## 中英文阈值对比（阈值扫描，2026-11-12 补）

> 每模型固定分值下扫描阈值 0.30~0.60：`zh pass?` = 中文查询能否命中英文工具描述；`en pass?` = 英文查询能否命中；`clean?` = 三个无关负向对是否全部低于阈值（无误召回）。

| 模型 | zh↔enTool | en↔enTool | 负向最高 | zh 通过的阈值上限 | en 通过的阈值上限 | 无误召回的阈值下限 | **可用阈值窗口** |
|---|---|---|---|---|---|---|---|
| **F2LLM v2-160M** | 0.617 | 0.810 | 0.033 | ≤0.60 全通过 | ≤0.60 全通过 | ≥0.04 | **0.04 ~ 0.60（极宽）** |
| F2LLM v2-80M | 0.488 | 0.588 | 0.288 | ≤0.45 | ≤0.55 | ≥0.29 | **0.29 ~ 0.45** |
| bekko v1-a25m | 0.416 | 0.882 | 0.298 | ≤0.40 | ≤0.60 全通过 | ≥0.30 | **0.30 ~ 0.40（窄）** |
| bekko v1-a8m | 0.425 | 0.787 | 0.307 | ≤0.40 | ≤0.60 全通过 | ≥0.31 | **0.31 ~ 0.40（窄）** |
| granite 311m-r2 | 0.920 | 0.965 | 0.786 | 全通过 | 全通过 | **无**（负向 0.78，任何阈值都误召回） | ❌ 不存在 |
| granite 97m-r2 | 0.819 | 0.929 | 0.683 | 全通过 | 全通过 | **无**（负向 0.68） | ❌ 不存在 |
| harrier 270m | 0.448 | 0.450 | 0.502 | ≤0.40 | ≤0.40 | ≥0.51 | ❌ 空窗（zh/en 上限 < 负向下限） |

### 🔴 重大修正：pooling 错误（2026-11-12 第二轮）

**用户质疑「330M 不应该这么差」是对的——根因是我们代码的 pooling 用错了，不是模型差。**

- **官方事实**（各 size 的 `1_Pooling/config.json`）：F2LLM-v2 全系（80M/160M/330M）都是 **last-token pooling**（`pooling_mode_lasttoken=true`，causal LM 取末 token）。
- **我们的错误**：
  - v2-80M GGUF **没有** pooling 元数据键 → 代码默认 mean → **向量全错**（此前 80M 的 0.488 低分是 bug 产物！）
  - v2-160M GGUF 写 `qwen3.pooling_type=3`（last）→ 碰巧正确
  - v2-330M 第三方 GGUF 写 `pooling_method=2`（CLS）→ 与官方矛盾 → 向量全错
- **修复**：qwen3 架构 pooling 缺省改为 last（3）；`decoder_embed` 强制 last（忽略错误的 CLS 标记）。

**修复后复测**：

| 模型 | zh↔enTool | en↔enTool | 负向最高 | 判定 |
|---|---|---|---|---|
| **F2LLM v2-80M** | **0.620**（原 0.488）| **0.814**（原 0.588）| 0.036（原 0.288）| **✅ 完全可用！窗口 0.04~0.60** |
| F2LLM v2-160M | 0.617（不变，原本就对）| 0.810 | 0.033 | ✅ |
| F2LLM v2-330M | 0.336（原 0.254）| 0.445（原 0.393）| 0.397 | ⚠️ 仍不达标（负向 0.397 与相关分重叠），该转换版质量确实差 |

**结论翻转**：
1. **当前默认 v2-80M 修复 pooling 后即可用**——zh 0.620 / en 0.814 / 负向 0.036，与 160M 同级，**无需换模型**！用户实测「如何查询公网ip」0 命中的根因就是这个 pooling bug（真机 0.446 是 mean-pooling 的错误向量）。
2. 160M 与 80M 修复后同级（0.617 vs 0.620），默认模型保持 80M 即可。
3. 330M 第三方转换版即使 pooling 修正后仍差（0.336/0.445，负向 0.397），维持不可用判定。
4. **重要**：修复后**必须重建智能路由索引**（旧向量是 mean-pooling 产物，与新 last-pooling 向量不可比）。

### 全模型 pooling 交叉审计（2026-11-12 第三轮）

逐一对照 GGUF 元数据、官方 sentence-transformers pooling 配置与我们的实现：

| 模型 | GGUF 标记 | 官方 pooling | 我们的实现 | 判定 |
|---|---|---|---|---|
| F2LLM v2-160M | qwen3.pooling_type=3 | lasttoken | last ✅ | 正确 |
| F2LLM v2-80M | （无键）| lasttoken | 修复后默认 last ✅ | 已修 |
| F2LLM v2-330M | decoder.pooling_method=2 | lasttoken | decoder_embed 强制 last ✅ | 已修 |
| bekko v1-a25m | modern-bert.pooling_type=1 | mean | mean ✅ | 正确 |
| bekko v1-a8m | modern-bert.pooling_type=1 | mean | mean ✅ | 正确 |
| granite 311m-r2 | modern-bert.pooling_type=2 | cls | cls ✅ | 正确 |
| granite 97m-r2 | modern-bert.pooling_type=2 | cls | cls ✅ | 正确 |
| **harrier oss-v1-270m** | gemma3.pooling_type=3 | **lasttoken**（microsoft 官方 README: "decoder-only architectures with last-token pooling"）| **修复前 mean+bidirectional ❌ → 修复后 causal+last ✅** | **已修** |

**harrier 修复**（gemma arch 第二处 pooling bug）：gemma arch 原来硬编码 mean-pool + 双向注意力；现按 GGUF `pooling_type` 分派——1=mean（gemma-embedding 家族，双向）保持原行为，3=last（harrier）走 causal mask + last-token pooling（复用 qwen3 的 causal_mask/pool）。

**修复后 harrier 复测**：zh↔enTool 0.386 / en↔enTool 0.387 / 负向最高 0.496——仍不达标（负向高于相关分），该模型本身检索能力弱，维持不可用判定（但至少向量语义现在是正确的）。

**测试注记**：并行 `cargo test --lib` 出现 3-4 个 `database is locked` 失败（group_service/bearer_exclude 测试共享文件池的锁竞争，与本次改动无关），`--test-threads=1` 串行 411/411 全绿。

### v2-330M 修复记录（2026-11-12）

- **加载失败根因**：GGUF 来自第三方转换（cstr/f2llm-v2-330m-GGUF）——arch 名写成 `decoder_embed`（非 `qwen3`），元数据键用 HF 风格（`decoder.hidden_size`/`num_hidden_layers`/`num_attention_heads`/`num_key_value_heads`/`head_dim`/`max_position_embeddings`/`rms_norm_eps`/`pooling_method`/`rope_theta`），张量名用 `dec.{i}.attn.q|k|v|o`、`ffn.gate|up|down`（非 `blk.{i}.attn_q`、`ffn_gate`），且丢失 `tokenizer.ggml.model/.pre/.token_type`。
- **修复**：①`gguf.rs` dispatch 增加 `decoder_embed` 别名；②`gguf_qwen3.rs` 键名双轨回退（qwen3 风格优先、HF 风格兜底）+ 张量名重映射闭包 + `pooling_method`/`rope_theta`/`rms_norm_eps` 兜底读取；③size 目录放入官方 `tokenizer.json`（codefuse-ai/F2LLM-v2-330M，BPE 151643 词）。
- **修复后实测**：模型正常加载（dim=896），但检索质量差——zh↔enTool 0.254、en↔enTool 0.393（带前缀），裸文本更低（0.169/0.171）；zh↔zh 0.72。**结论：330M 这个转换版的嵌入质量不达标，不推荐使用**（可能系转换量化损失或该蒸馏尺寸本身能力不足）。

### 阈值结论

1. **v2-160M 窗口 0.04~0.60**：任何合理阈值（含当前默认 0.5）都同时满足中英文命中 + 零误召回——换默认模型后阈值无需调整。
2. **v2-80M 窗口 0.29~0.45**：当前默认 0.5 恰好落在窗口外（中文必丢）。若坚持用 80M，阈值须降到 0.40~0.45，但负向 0.29 说明已贴近误召回边缘，鲁棒性差。
3. **bekko 窗口仅 0.30~0.40**：中文上限 0.40 与负向下限 0.30 只剩 0.10 余量，实际查询措辞稍有变化就会失效。
4. **granite 两款在任何阈值下都误召回**（无关文本 0.64~0.79），不可用于检索。
5. **harrier 无可用窗口**：中文查询得分（0.448）比无关对（0.502）还低，阈值无论怎么调都无法同时满足命中与不误召回。

## 建议

- **把智能路由/RAG 默认模型从 v2-80M 换成 v2-160M**（同为 F2LLM 家族，体积 160M 可接受，中文跨语言能力质变）。
- deploy.json 的 `searchQueryPrefix` 对 F2LLM 无益（v2-160M 带前缀 0.617 vs 裸 0.606，差异微小；v2-80M 带前缀反而全面更差）——smart_routing 代码路径未应用前缀是正确行为，无需改动。
- v2-330M 补 tokenizer.json 后可再评测（参数量更大，可能更好）。
