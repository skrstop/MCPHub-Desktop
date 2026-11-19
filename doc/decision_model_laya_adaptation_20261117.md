# 决策模型适配：laya-multilingual（ggmlc 融合图解释器）（2026-11-17）

## 背景

决策模型目录已从 `runtimes/rag/model/decision` 独立为 `runtimes/decision/model`（与 rag 平级）。在此基础上适配第二个决策模型 `convaiinnovations/laya-multilingual`（mmBERT-base 322M，Q8_0，361MB）。

## laya GGUF 与 julia 的差异

laya 使用 `ggmlc` 转换器导出，与 julia 的 `modern-bert.*` 布局完全不同：

| | julia_1 | laya-multilingual |
|---|---|---|
| architecture | `modern-bert` | `ggmlc`（融合计算图） |
| 张量命名 | `blk.N.*` / `cls.*` | 扁平 `qkv.N` / `mlp_wi.N` / `head_*` / `scorer_*` / `act_*` |
| 元数据 | `modern-bert.*` | `laya.*`（max_len=1024, head_max_len=256, max_opts=16, 特殊 token id） |
| RoPE | 运行时计算 | 预计算表 `cos_full/sin_full/cos_slide/sin_slide [1025, 64]` |
| 滑窗 | 运行时掩码 | 预计算 `slide_bias [1025,1025]`（±64 窗口，窗外 -10000） |
| LayerNorm 仿射 | 独立权重 | **折叠进后续线性层**（`linear_96_baked_bias`），图中 LN 为纯归一化 |
| 额外输出 | - | act/escalate 头（`[2, B]` logits）+ 校准段（cummax/熵/温度特征） |

## 实现（`src-tauri/src/mv/ggmlc.rs`）

通用 `ggmlc` 图解释器：解析 `ggmlc.graph_spec` JSON（1252 tensor / 1118 节点），按节点 id 拓扑执行。关键约定（全部经数据验证）：

- spec 的 `ne` 为 ggml 序；candle 直接返回该序（不反转）。2D 参数转置为 torch 序后统一处理。
- `slice`/`getitem` 用 `ggml_dim` 寻址（0 = 最快/torch 末维）；`end: -1` 在 `start==0` 且轴长 `max_len+1` 时表示截断到动态序列长度，否则为开放末端。
- `transpose` 节点是融合 SDPA 的标记（`fused_transpose: 1`），激活保持 torch `[B,S,H,D]`，SDPA 内部换轴。
- opcode 200 = `gelu(x)`（偏置已由线性层加过）；`select`（`is_select:1`）= 切片后压缩该维。

序列构造完全对照官方 `laya` 包 `common.py::build_sequence`：`[CLS] <type> question: ins [SEP] [MASK] opt… [SEP] state [SEP]`；score 选项渲染 `level N: text`，noul 渲染固定 false/true 对；选项截断 48 token、head 预算 256、总长 1024。

## 验证

1. **numpy 参考实现**（同一 GGUF 权重执行图）↔ **官方 `laya` pip 包**（HF safetensors fp16）：
   - choice：billing @ 0.9994 vs 0.9994、technical @ 0.9982 vs 0.9983、sales @ 0.9989 vs 0.9990
   - noul：false @ 0.9985 vs 0.9984
   - score：概率差 ~0.005（Q8 量化误差量级；choice/noul 的 logit 差 <0.01）
2. **Rust 探针** `mv::ggmlc::tests::laya_golden_probe`（4 用例 scores/probs/argmax 对照 numpy 金标准，容差 0.02）：通过。
3. 全量 `cargo test --lib`：451 passed / 0 failed（julia 路径无回归）。

## 其他修正

- `build_gguf_tokenizer`：`model=gpt2` 但 `pre=gemma`（Metaspace 词表）时走 gemma 构建器，否则 ByteLevel 预分词产生大量 `<unk>`。
- laya 目录补齐 `tokenizer.json`（与 julia 的完全相同——同一个 gemma 256k tokenizer）。
- laya/julia 的 `deploy.json` downloadUrls 修正（laya 原本错指 julia 的模型文件；两者还带着目录迁移前的 `rag/model/decision` 旧路径）。
