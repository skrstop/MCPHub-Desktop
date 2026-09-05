# RAG 文档导入支持 PDF/Office 解析 + 图片 OCR（提取为 Markdown 存储）— 实施计划 v2.1

> 修订（2026-09-05，应用户修改意见）：
> 1. **PDF 改用 `pdf_oxide`**（原方案 pdf-extract）— 质量更高，且自带 `to_markdown_all()`。
> 2. **Office 文档改用 `office_oxide`**（原方案 quick-xml 手写 docx + calamine xlsx）— 一个 crate 覆盖 docx/**doc**/xlsx/**xls**/pptx/**ppt** 全部六种格式（含 Legacy 二进制格式），自带 `to_markdown()`。
> 3. **提取结果统一转为 Markdown 存储**（原方案存纯文本）— 后续逻辑（chunker → embedding → lancedb → 查看/分片/搜索）与普通文本完全一致。
> 4. **新增图片 OCR**（v2.1）：`uni-ocr` 0.1.5，支持 png/jpg/jpeg/gif/bmp/webp/tiff；macOS/Windows 用系统内置引擎（Vision / Windows.Media.Ocr），**Linux 依赖外部 tesseract**——OCR 前置检测，缺失时前端弹框展示各发行版安装命令。
> 5. **OCR 设计为可复用核心**（v2.2）：OCR 能力封装为独立函数，不仅服务图片导入，还嵌入 PDF/Word/PPT 的解析链路——这些文档内嵌的图片同样过 OCR 提取文字，一并进入 Markdown。
> 6. **提取层采用策略模式**（v2.3）：沿用项目既有的 `ChunkStrategy` 风格（chunker.rs:49 trait + `Box<dyn>` 派发），新增 `ContentExtractor` 策略接口——纯文本是兜底策略，PDF/Office/图片各自是独立策略实现，`content_from_source` 退化为纯派发逻辑。

## Context（背景）

RAG 文档导入目前只接受纯文本文件：上传管线用 `is_likely_text`（NUL 字节嗅探，service.rs:4250）拒绝一切二进制，PDF/Word/Excel 等办公文档被 `UNSUPPORTED_FORMAT` 哨兵拒之门外，UI 文案明确写着"仅支持纯文本文件"。需求：导入 RAG 时支持解析 **PDF / Word(.docx/.doc) / Excel(.xlsx/.xls) / PowerPoint(.pptx/.ppt)**，**提取为 Markdown** 后走现有的 chunker → embedding → lancedb 管线。

**现状已回滚**：上一轮草稿实现（extract.rs + Cargo.toml 依赖改动）已全部撤回，工作区干净（基于 commit 38d5691）。

## 类库选型（已核实，2026-09 经 crates.io API + 源码确认）

| 格式 | 选型 | 版本 | 理由 |
|---|---|---|---|
| **PDF** | `pdf_oxide` | 0.3.77（81.8 万下载） | 用户指定。纯 Rust；`PdfDocument::open_from_bytes(Vec<u8>)` + `to_markdown_all(&ConversionOptions)` 一行出 **Markdown**（默认 detect_headings=true 语义模式、表格开启、图片关闭）。默认 features 只有 `icc`(qcms)+`legacy-crypto`(md-5)；硬依赖 37 个全纯 Rust（image 仅 png/jpeg/tiff 且 default-features=false、brotli、taffy、nom 等）；tokenizers/ort/pyo3 全在可选 feature（默认关）。rust-version 1.88。**它自己还依赖 office_oxide**（版本自动对齐） |
| **Office 全家（docx/doc/xlsx/xls/pptx/ppt）** | `office_oxide` | 0.1.9（50.4 万下载） | 用户指定。一个 crate 六种格式（含 Legacy 二进制 .doc/.xls/.ppt），**自带 `to_markdown()`**；`Document::from_reader(Read+Seek, DocumentFormat)` 支持内存字节。依赖极轻（12 个，全纯 Rust）：quick-xml 0.41 + **zip 8.6（default-features=false, deflate）** — 与本项目拟声明的 zip 逐字一致，单份编译；默认 features 为空（mmap/rayon/pyo3/wasm 全可选）。rust-version 1.88 |
| ~~.doc 旧格式~~ | ~~不支持~~ → **改为支持** | — | office_oxide 原生支持 Legacy 二进制格式（DocDocument/XlsDocument/PptDocument），不再拒绝 |
| **图片 OCR**（v2.1） | `uni-ocr` | 0.1.5（1.7 万下载，mediar-ai） | `OcrEngine::new(Auto)` → `recognize_image(&DynamicImage)`，返回 (text, json, confidence)。**Auto 后端平台路由**：macOS→Vision（cidre 系统框架，无外部依赖）、Windows→Windows.Media.Ocr（系统内置）、Linux→**tesseract 外部二进制**。⚠️ uni-ocr 的 tesseract 路径内部 `unwrap()`，tesseract 缺失会 panic 而非返回 Err——因此必须在调用前自行检测（`tesseract --version` + `--list-langs`），这正是本计划的 OCR 预检设计。依赖 image 0.25（与 pdf_oxide 共用）、reqwest 0.12（项目已有同版）、tokio |
| zip（读 OOXML 容器 + Windows 解压 node runtime） | 复用 `zip` = "8" | 8.6（已在依赖树） | 从 `[target.'cfg(target_os="windows")']` 提升为全平台 `default-features = false, features = ["deflate"]`（与 office_oxide/pdf_oxide 内声明一致，单份编译；顺带砍掉 default features 的 zstd-sys 等 C 依赖）。`commands/runtime.rs` 的 ZipArchive 用法不变（node zip 是 deflate） |

**依赖增量**：新增 crate ≈ 45（pdf_oxide 硬依赖 ~37 + office_oxide ~12，zip/quick-xml/chrono 等与现有树重叠），全部纯 Rust、零 C/C++ 构建依赖（符合项目避开 esaxx/onig 的既有约束；tokenizers 仍是项目自己的 0.23 default-features=false，pdf_oxide 的 tokenizers 在 `ml` feature 后面默认不拉）。冷编译 +40~90s，增量 ≈ 0；二进制体积 +2~4MB。

## 修改点

### 1. `src-tauri/Cargo.toml` — 新增依赖

```toml
# RAG binary office-document import (rag/extract.rs): pure-Rust extractors,
# both output Markdown directly.
pdf_oxide = "0.3"      # PDF -> markdown (default features: icc + legacy-crypto)
office_oxide = "0.1"   # docx/doc/xlsx/xls/pptx/ppt -> markdown (default features: none)
# 从 Windows target 提升为全平台,只留纯 Rust deflate(与 office_oxide 内部声明一致,
# 单份编译;顺带砍掉 default features 的 zstd-sys 等 C 依赖)
zip = { version = "8", default-features = false, features = ["deflate"] }
```
同时**删除** `[target.'cfg(target_os = "windows")']` 段里的旧 `zip = "8"`（避免 Windows 构建时 feature 合并拉回全量默认 → zstd-sys/bzip2/lzma C 依赖）。

### 2. 重写 `src-tauri/src/rag/extract/` — 提取模块（策略模式，v2.3）

沿用项目既有的策略模式风格（`chunker.rs` 的 `ChunkStrategy` trait + `Box<dyn>` 派发 + 兜底降级），提取层设计：

```
src/rag/extract/
├── mod.rs        # ContentExtractor trait + 策略注册表 + 派发入口
├── ocr.rs        # OCR 可复用核心（被多个策略共享）
├── pdf.rs        # PdfExtractor（pdf_oxide + 内嵌图片 OCR）
├── office.rs     # OfficeExtractor（office_oxide + IR 内嵌图片 OCR）
├── image.rs      # ImageOcrExtractor（uni-ocr 直连）
└── text.rs       # PlainTextExtractor（现有通用文本路径，兜底策略）
```

**策略接口**（mod.rs）：
```rust
pub trait ContentExtractor: Send + Sync {
    /// 策略是否处理该文件（按扩展名/嗅探判断）
    fn can_handle(&self, filename: &str) -> bool;
    /// 提取为 Markdown；失败返回 Err（EXTRACT_FAILED:/OCR_MISSING: 前缀哨兵）
    fn extract(&self, filename: &str, bytes: Vec<u8>) -> Result<String>;
    /// 策略名（日志用）
    fn name(&self) -> &'static str;
}

/// 派发入口：按注册表顺序首个 can_handle 命中者执行；全不命中 → 兜底策略。
/// service.rs 的 content_from_source 退化为 extract::run(name, bytes)。
pub async fn run(filename: &str, bytes: Vec<u8>) -> Result<String>;
```

**注册表**（静态数组，按优先级排序）：
```rust
static EXTRACTORS: &[&dyn ContentExtractor] = &[
    &PdfExtractor,      // .pdf
    &OfficeExtractor,   // .docx/.doc/.xlsx/.xls/.pptx/.ppt
    &ImageOcrExtractor, // .png/.jpg/.jpeg/.gif/.bmp/.webp/.tiff/.tif
    &PlainTextExtractor,// 兜底：is_likely_text 嗅探 + decode_text（现有路径）
];
```

**各策略实现要点**：
- `PlainTextExtractor`（text.rs）：现有通用文本处理原样搬入——`is_likely_text` 嗅探不过报 `UNSUPPORTED_FORMAT`，过则 `decode_text`。行为与现状逐字节一致
- `PdfExtractor`（pdf.rs）：`PdfDocument::open_from_bytes` → `to_markdown_all(&ConversionOptions::default())`；随后 `extract_images(page)` 逐页取内嵌图（`PdfImage::to_dynamic_image()` 已处理 CMYK JPEG）→ 逐张 `ocr::image_bytes()` → 以 `> OCR(图片N):\n> ...` 引用块追加到文档尾；空文字层（扫描件）时若 OCR 可用则整篇靠 OCR 兜底，否则报 `EXTRACT_FAILED: pdf has no text layer`
- `OfficeExtractor`（office.rs）：`DocumentFormat::from_extension` → `Document::from_reader(Cursor)` → `to_ir()` → 自行遍历公开 IR（`sections[].elements[]`，含 Table→Row→Cell / List→Item / TextBox / Note 嵌套）渲染 Markdown 并收集 `Element::Image`（`data: Option<Vec<u8>>`，Emf/Wmf 矢量格式跳过）→ 逐张 `ocr::image_bytes()` → OCR 文本以 `[图片OCR]\n> ...` 追加到文档尾（保守策略，不做位置映射）
- `ImageOcrExtractor`（image.rs）：直接 `ocr::image_bytes`；空结果报 `EXTRACT_FAILED: no text recognized in image`

**OCR 可复用核心**（ocr.rs，独立于任何策略）：
```rust
pub fn available() -> bool;      // 引擎预检（OnceLock 缓存）
pub fn status() -> OcrStatus;    // { available, platform, engine, missingLangs }
pub async fn image_bytes(bytes: &[u8]) -> Result<String>;  // 唯一 OCR 入口
```
- `available()`：macOS/Windows 恒 true（系统内置 Vision / Windows.Media.Ocr）；Linux 跑 `tesseract --version` + `--list-langs`（校验 eng/chi_sim），结果缓存。⚠️ uni-ocr 的 tesseract 路径内部 `unwrap()`，缺失引擎会 panic——必须前置检测，绝不让它走到
- `image_bytes()`：前置检测失败报 `OCR_MISSING: <platform>`（前端弹框哨兵）；`image::load_from_memory` → `OcrEngine::new(Auto).languages([Chinese, English]).timeout(120s)` → recognize → 清洗。语言固定中英双语
- 单张图片失败记 rag_log warn 后由调用方跳过（返回 Err，策略层 catch），不阻断整篇文档

### 2.1 OCR 缺失的弹框告知（v2.1，v2.2 扩展：三种触发点）

- **后端新命令** `get_ocr_status() -> { available, platform, engine, missingLangs }`（`commands/rag.rs` + `lib.rs` 注册）——上传弹框打开时可预检，也可在失败弹框里复用
- **触发点**：①独立图片导入失败（`OCR_MISSING` 哨兵）；②上传弹框打开时的预检（图片/PDF/Office 均可能用到 OCR）；③PDF/Office 内嵌图片 OCR 环节（文档文字已提取成功时不弹框，只 log + 在结果里注明"图片未识别"）
- **上传失败分流**（useRagData）：`OCR_MISSING` 前缀 → 调 `get_ocr_status()` 拿平台 → **弹框**（非 toast）：标题 `pages.rag.ocrMissingTitle`，正文说明 + **具体安装命令**（等宽字体块 + 复制按钮），按平台展示：
  - Debian/Ubuntu: `sudo apt install tesseract-ocr tesseract-ocr-chi-sim tesseract-ocr-eng`
  - Fedora/RHEL: `sudo dnf install tesseract tesseract-langpack-chi_sim tesseract-langpack-eng`
  - Arch: `sudo pacman -S tesseract tesseract-data-chi_sim tesseract-data-eng`
  - macOS/Windows 理论不触发（系统内置）；兜底文案"当前平台 OCR 引擎不可用"
- 命令文案放 i18n（`pages.rag.ocrInstallApt/Dnf/Pacman` 等 4 语言）；弹框按 `get_ocr_status().platform`（`/etc/os-release` 探测发行版，兜底 "linux"）选择显示哪条

### 3. `src-tauri/src/rag/mod.rs` — 注册模块
`pub mod extract;`（目录模块）+ 模块文档一行。**注意**：需删除已建的旧单文件 `extract.rs`（被 `extract/` 目录替代）。

### 4. `src-tauri/runtimes/rag/file_support.json` — 扩展目录（15 条）
文档 7 条：`.pdf/.docx/.doc/.xlsx/.xls/.pptx/.ppt`；图片 8 条（v2.1）：`.png/.jpg/.jpeg/.gif/.bmp/.webp/.tiff/.tif`。此目录同时被后端 `file_type_map()`（scan_folder 扩展名过滤 + file_type 标签）与前端 `fileType.ts` 显示标签使用——加进来后扫描对话框才会列出这类文件。

### 5. `src-tauri/src/rag/service.rs` — 打通管线

**核心设计：extractable 文档一律按 "copy" 语义处理（提取出的 Markdown 落盘为内容文件），method 强制写 "copy"**：
- 内容文件是**派生的 Markdown**（不是原文件拷贝），symlink/copy 的"是否复制原文件"区分对它无意义；强制 "copy" 后所有既有读路径（查看/列表/reindex_all/删除/批量更新/classify_original）零改动自动正确
- 受益：symlink 导入的 PDF 也能查看（存的是提取出的 markdown，不再读原始二进制乱码）——顺带修掉了 v1 计划的已知限制
- 原始字节语义不变：`md5` 始终对**原始字节**计算，`original_path` 记录源路径，更新检测（单条/批量/自动）完全不变

具体改动（4 处小改）：

**(a) 新增 `content_from_source` helper + 三个入口分流**
```rust
// extract::run 策略派发（PDF/Office/图片提取、兜底文本嗅探）；
// UNSUPPORTED_FORMAT / EXTRACT_FAILED / OCR_MISSING 哨兵语义不变。
async fn content_from_source(name: &str, raw: Vec<u8>) -> Result<(String, &'static str)>
```
`upload_one_path_inner`（~L2651）、`update_doc_from_file`（~L2456）、`update_doc_from_original`（~L2568）三处 `is_likely_text → UNSUPPORTED_FORMAT → decode_text` 块统一替换为该 helper。

**(b) 内容文件命名**：extractable 源的内容落盘为 **`{id}.md`**（真实反映内容格式），非 extractable 维持 `{id}{ext}`。配套 `content_path_for` 在 candidate 1（`{id}{meta_name ext}`）与 candidate 2（`{id}`）之间插入 candidate `dir/{id}.md`。meta.name 保持源文件名（"report.pdf"），`file_type` 标签走既有 `file_type_label`（PDF/Word 等，来自 file_support.json）——前端据此显示真实类型。

**(c) method 强制**：三个写入入口对 extractable 源把 `method` 写成 `Some("copy")`（`update_doc_from_file`/`update_doc_from_original` 的透传逻辑同步覆盖）。

**(d) `scan_folder` 的 8KiB 文本嗅探（~L2283）**——`is_extractable` 的文件跳过 `is_likely_text` 检查（PDF 头 8KiB 必有 NUL，现会被误杀）；文本类保持嗅探。

### 6. 前端 — 错误文案 + 提取文档按 Markdown 渲染

**(a) `frontend/src/hooks/useRagData.tsx` `upload()`（~L564）**：错误分支加一档 `EXTRACT_FAILED` 前缀 → 新 i18n key `pages.rag.extractFailed`（带 `{{reason}}` 参数，显示后端具体原因如"扫描件无文字层"）。

**(b) `frontend/src/utils/fileType.ts`**：新增 `EXTRACTED_MARKDOWN_EXTS`（pdf/docx/doc/xlsx/xls/pptx/ppt）——这七类桌面端存的都是提取出的 Markdown，`isMarkdown()` 对它们返回 true（查看/搜索片段按 Markdown 渲染：标题/表格/列表高亮），`hlLangFor` 对它们返回 undefined。

**(c) locales 四语言**（zh/en/fr/tr）：更新 `unsupportedFile` 文案（删掉"仅支持纯文本"旧说法）+ 新增 `extractFailed`。

**(d) 导入弹框提示文案（用户补充）**：`RagPage.tsx:1707` 渲染的 `pages.rag.uploadHint` 当前写"仅支持纯文本文件，不支持 PDF/Word/XLSX"——四语言全部更新为新文案（纯文本 + PDF/Word/Excel/PPT 均支持，自动提取为 Markdown）。

### 7. 版本 + 文档

- 版本四源 `1.0.33002 → 1.0.33101`（tauri.conf.json / Cargo.toml / 根 package.json / frontend/package.json + Cargo.lock）
- `doc/upgrade/1.0.33101.md`：说明新增 7 种格式、Markdown 存储语义、扫描件 PDF 不支持（需 OCR 超出范围）
- `agent.md` 新增 §3.14 记录本特性

## 明确不做（Non-goals）

- **扫描版 PDF（图片型）**：纯文本提取库无法 OCR，提取结果为空时报 `EXTRACT_FAILED` 并提示用户，不引入 OCR 引擎（pdf_oxide 的 ocr/ml/gpu feature 全部不开启）
- **.ods/.xlsm/.xlsb 等**：office_oxide 不支持，`UNSUPPORTED_FORMAT` 拒绝
- **MCP 工具 `rag_file_create`**：输入本来就是 UTF-8 文本，无需解析能力
- **提取文档的"从原始更新"重提取**：`update_doc_from_original` 已覆盖（重新提取 + 刷 md5），自动定时更新/批量更新自动受益，零额外改动

## 实施记录（2026-09-05 完成）

- ✅ 全部按计划落地，**一处重要偏离**：图片 OCR 弃用 `uni-ocr`——其 macOS 后端 `cidre` 的构建脚本硬依赖完整 Xcode 的 `xcodebuild`（本机仅 CommandLineTools，无法编译）。改为自研三平台实现（接口与哨兵不变）：macOS `objc2-vision`（链接系统 Vision，参考 macocr 0.4.7 用法）、Windows 复用已有 `windows` crate（Windows.Media.Ocr WinRT 管线）、Linux 直接调 `tesseract` 子进程（自控预检，消除 uni-ocr 内部 unwrap panic 风险）。详见 AGENTS.md §3.14。
- Office Markdown 渲染直接复用 `DocumentIR::to_markdown()`（crate 自带），未自行遍历 IR 渲染文本（图片收集仍自行遍历 IR）。
- 验证：`cargo check --lib` 通过；Windows/Linux cfg 代码 scratch-crate 交叉 type-check 通过；临时集成测试（真实中文 PDF ×4 + 最小 docx/xlsx/pptx + OCR 管线 + 文本兜底）4/4 通过后按计划移除；`cargo test --lib` 23/23；`npm run build` 通过；`tsc --noEmit` 24 = 基线 24（零新增）。
- 版本 `1.0.33101`；changelog `doc/upgrade/1.0.33101.md`；AGENTS.md 新增 §3.14。

## 验证

1. `ORT_SKIP_DOWNLOAD=1 cargo check -p mcphub`——新依赖编译 + extract.rs 类型检查
2. 真实样本实测解析（临时单元测试）：PDF（仓库 doc/test/*.pdf）+ 手工构造 docx/xlsx/pptx（Python zipfile 生成最小 OOXML），验证中文、表格、多 sheet 的 Markdown 输出
3. `cargo test --lib`（确认既有测试不回归）
4. 前端 `npm run build`（tsc 类型检查通过）
5. 真机验证留给用户：`tauri dev` 后在 RAG 页面上传 pdf/docx/xlsx/pptx，确认导入 → 分片 → 搜索 → 查看（Markdown 渲染）链路 + 失败场景（扫描件 PDF）的 toast 文案
