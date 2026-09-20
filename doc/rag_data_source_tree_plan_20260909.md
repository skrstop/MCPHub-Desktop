# RAG 数据源统一 + Git 数据源 + 树形列表 设计与计划（2026-09-09）

> ✅ **状态（2026-09-09）：代码全部完成 + 五轮复核修复 9 问题**（P1/P2/P2b/P3/P4/P5/P6），版本 `1.0.35002`。复核记录见 §3.17.7（AGENTS.md）；进度详情见 §6；changelog 已并入 `doc/upgrade/1.0.35003.md`（35002 与 35003 合并发布）。剩余：手动回归清单（§6 末尾）待用户真机验证。

> 三个需求：
> 1. RAG 导入入口统一为「数据源选择」，文件选择 / 文件夹选择只是其中的选项；
> 2. 新增 **Git** 数据源：可通过 git 拉取远程文件，管理/解析流程同现有流程；
> 3. RAG 列表增加 **树形结构** 切换：现有平铺结构保留，树上展示「数据源 → 物理目录 → 文件」；存量历史数据统一归入「文件」数据源节点。

---

## 0. 现状梳理（设计依据）

- **导入入口（前端 `RagPage.tsx` UploadDialog）**：现有三个入口——多选文件（`pickFiles` → 折叠成单组伪 group）、扁平/递归文件夹（`pick_rag_folder` → `scan_folder` 分组扫描）、MCP 工具 `rag_file_create`（无 UI）。导入方式（软链接/拷贝）分段切换仅对磁盘导入有意义。
- **文档存储（Rust `rag/service.rs`）**：每文档一对文件 `{id}.meta` + 内容文件；`DocMeta { id, name, tags, size, version, file_type, method, original_path, md5, ... }`。**文档不进 DB 表**，FTS 侧只有 `fts_rag_docs(ref_id=id, text=name)`。
- **更新链路**：`check_rag_update` / `update_doc_from_original` / `update_doc_from_file` / 批量更新（`run_batch_update`，CAS `BATCH_UPDATE_RUNNING`）/ 自动定时更新（§3.13）。核心指纹 = `md5(original 字节)`。
- **扫描过滤**（`scan_folder`）：`file_support.json` 扩展目录 + dotfile/`folder_ignore.json`/symlink 跳过 + 8KiB 文本嗅探（extractable 格式跳过嗅探）+ `SCAN_FOLDER_FILE_CAP` 截断。

---

## 1. 数据源模型（核心抽象）

### 1.1 `DocSource`（写入 DocMeta）

```rust
// src-tauri/src/rag/service.rs
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct DocSource {
    pub kind: String,          // "file" | "folder" | "git" | "tool"
    /// 数据源展示标签：文件/文件夹 = 来源路径（file 用父目录或路径本身）；
    /// git = `repo url @ branch`；tool = "MCP 工具"
    pub label: String,
    /// 来源根（绝对路径 / clone 目录）。file kind = 文件所在目录；folder = 所选文件夹；git = clone 目录
    pub root: Option<String>,
    /// 相对 root 的路径（目录链），树形结构用；根下文件为 ""
    pub rel_path: Option<String>,
    /// kind = "git" 时的远程信息
    pub git: Option<GitSource>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct GitSource {
    pub url: String,
    pub branch: Option<String>,
    /// 首次导入时的 commit sha（展示用；更新以 fetch 后重扫为准）
    pub commit: Option<String>,
    /// 仓库内子目录（v1 不暴露 UI，预留）
    pub subdir: Option<String>,
}
```

- 序列化进 `{id}.meta` JSON，**不进 DB 列、无迁移**（RAG 文档本就不落表）。
- `#[serde(default)]` 全字段兜底，旧 meta 无 `source` → 反序列化为 `None`。

### 1.2 存量兼容（需求 3 的硬性要求）

- `classify_source(meta)`（集中式 helper，类似 `classify_original` 的角色）：
  - `meta.source` 存在 → 直接用；
  - 无 `source`（存量 legacy / 现版本写入的文档）→ **一律视为 `kind = "file"`**（树归入「文件」节点），`label` 取 `original_path` 的父目录（无 original_path 则取「文件」通用标签）。
- 读取路径收敛：`list_docs` / `get_doc` 统一调 `classify_source` 填充 `RagDocInfo`/`RagDoc` 的展示字段，**不改写 meta 文件**（惰性分类，避免全量重写磁盘）。

### 1.3 `RagDocInfo` / `RagDoc` 新增展示字段

```rust
#[serde(default)] pub source_kind: String,    // file|folder|git|tool（classify 后）
#[serde(default)] pub source_label: String,   // 数据源节点显示名
#[serde(default)] pub source_root: String,    // hover / 详情用
#[serde(default)] pub rel_path: String,       // 树形目录链
#[serde(default)] pub git_url: String,        // git 文档专属（跳转远程仓库）
#[serde(default)] pub git_branch: String,
```

前端 `types/index.ts` 同步加同名字段（全部可选 string）。

---

## 2. 需求 1：导入入口统一为「数据源选择」

### 2.1 UI（`UploadDialog` 重构）

- 弹框顶部新增**「数据源」分段切换**（仿现有导入方式 toggle 的样式）：`文件 | 文件夹 | Git`。
  - **文件**：现 `pickFiles` 行为不变（多选 → 合并进树）。
  - **文件夹**：现 `pickFolder` + 递归开关行为不变（replace 语义保留）。
  - **Git**：显示表单（仓库 URL、分支（可空=默认）、**可选**「用户名 / 密码(token)」输入框——公共仓库留空匿名拉取）+「拉取并扫描」按钮 → 后端 gix clone → 返回 `RagFolderScan`（分组树同文件夹扫描渲染）；认证失败（`GIT_AUTH_REQUIRED`）→ 弹账密输入对话框重试。
- 「导入方式（软链接/拷贝）」切换行：**Git 数据源下隐藏**（强制 copy，理由同 extractable 格式——clone 目录是派生物）。
- 勾选/分组/标签/上传管线全部复用现有逻辑——Git 扫描结果就是一个 `RagFolderScan`。

### 2.2 上传时写入 source

- 前端 `uploadRagDoc(filePath, tags, method)` 增加 `source` 参数（或按数据源类型分支传 `sourceKind`/`sourceRoot`/`relPath`/`git{...}`）。
- 后端 `upload_one_path` → `write_doc_and_index` 链路把 `DocSource` 写进 meta：
  - **文件夹**：`root = scan.root`，`rel_path = group.relPath`（扫描结果里已有，逐文件透传）；
  - **文件**：`root = 父目录`，`rel_path = ""`；
  - **Git**：`root = clone 目录`，`rel_path = group.relPath`，`git = { url, branch, commit }`；
  - **tool**（`rag_file_create`）：`kind = "tool"`（见 §1.2 决策）。
- `rel_path` 另外落到 `RagScanFile`：`RagScanFile { path, name, size, rel_path }`（serde default，向后兼容），上传循环按 path 匹配带下去。

> 设计决策：MCP 工具创建的新文档标 `kind="tool"`，树中单独归入「工具创建」节点；**存量**（无 source 字段，含历史 tool 文档）按需求统一归「文件」。边界清晰、实现简单。

---

## 3. 需求 2：Git 数据源集成

### 3.1 技术选型（决策记录，2026-09-09 更新）

**选 `gix`（gitoxide，纯 Rust）作为库直接编译进主二进制**，不捆绑 git 可执行文件、不用 `git2`/libgit2：

- **纯 Rust + 库形态**：`gix` 编译期并入主程序，用户机器**无需安装 git、无需捆绑便携版**（对比 CLI 方案的"捆绑 git"诉求，gix 天然满足——没有可执行文件要带）；
- 能力已核实（docs.rs 最新版）：
  - `gix::clone::PrepareFetch` + `PrepareCheckout`：clone 全流程，支持**进度回调**（映射到导入进度条）、**should_interrupt 中断**；
  - `gix::remote::fetch::Shallow` 枚举存在 → **浅克隆（depth）受支持**；
  - `PrepareFetch::with_credentials(...)` → **账密/token 可直接注入** fetch 握手（配合需求 2 的认证）；
  - async 支持但本质 blocking → 走 `tauri::async_runtime::spawn_blocking`（与 extract 管线同模式）。
- feature 依赖：`gix = { version, features = ["blocking-network-client", "worktree-mutation"] }`；HTTP 传输经 `gix-transport` 的 reqwest 后端——**编译期注意**项目主 reqwest 0.12 与 gix 依赖树的版本归并（P3 首项验证点，若冲突则锁 `curl` 后端或统一升 reqwest，见 §7）。
- 凭证体系：账密提示走**前端对话框**（见 §3.3），不走 gix-credentials 的终端 Prompt 模式、不读系统 credential helper（桌面端无可交互终端）。

### 3.2 新模块 `src-tauri/src/rag/git.rs`

```rust
pub struct GitSyncResult { pub dir: PathBuf, pub commit: String }

/// 浅克隆到 <app_data>/rag/git/{repo_hash}；credentials = (username, password)
pub async fn clone_shallow(url, branch, credentials: Option<(String, String)>,
                           on_progress, should_interrupt) -> Result<GitSyncResult>
/// 更新：浅克隆到 {repo_hash}.new 临时目录 → 原子 rename swap（见 §3.4）
pub async fn refresh(dir, url, branch, credentials) -> Result<GitSyncResult>
pub fn repo_hash(url) -> String   // url 的 blake3 短哈希，目录名安全
pub fn classify_auth_error(e: &gix Error) -> bool  // 判定 401/403/authentication required
```

- **存储分两段（2026-09-09 用户确认）**：
  - **扫描阶段（临时）**：clone 到 **OS 临时目录** `std::env::temp_dir()/mcphub-rag-git/{repo_hash}/`——用户未确认导入前不污染应用数据目录；扫描/勾选/取消全在临时目录，应用退出或下次 clone 前顺手清空 `mcphub-rag-git/` 即可，**无需逐仓库清理逻辑**（OS 亦有临时目录自愈）。
  - **确认导入后（持久）**：`upload_rag_doc` 循环开始前，把用到的 repo 从临时目录**拷贝**到 `<app_data>/rag/git/{repo_hash}/`（与 `rag/files` 平级；不在 `.meta` 扫描范围，天然隔离）。拷贝仅勾选导入时发生——取消导入的临时 clone 留在临时目录自然消亡。
- 浅克隆 depth=1 + 指定 branch；进度回调发 `rag://git-sync-progress`（clone 期间复用上传双进度条的仓库级进度）。
- 中断：用户关弹框 → `should_interrupt` 置位，gix 停止传输，临时目录清理（`PrepareFetch` drop 自带删除）。
- token 转账密：token 作为 password、用户名固定 `x-access-token`（GitHub/GitLab 惯例）由前端组装，后端只收 (username, password)。

### 3.3 命令与流程（含认证 + 账密提示）

- 新命令 `pick_rag_git_repo(app, url, branch, username: Option<String>, password: Option<String>) -> RagFolderScan`：
  gix clone → `scan_folder(clone_dir, recursive=true)` → 回填 `commit`。复用现有过滤规则，前端零新增渲染逻辑。
- **账密交互（两段式）**：
  1. **Git 信息在选择数据源时即全部输入**（2026-09-09 用户确认）：Git 表单包含仓库 URL、分支、**克隆深度 depth**（默认 1=只取最新提交）、可选「用户名 / 密码(token)」——公共仓库留空匿名拉取，已导入数据源下拉快捷回填；
  2. clone 失败且 `classify_auth_error` 判定为认证错误 → 后端返回结构化错误前缀 **`GIT_AUTH_REQUIRED:`** → 前端**表单内联报错**（不弹独立对话框），用户就地补账密后重试 `pick_rag_git_repo`。**认证成功后凭证按数据源持久化**（keyring + 注册表，见 §3.6），后续更新免重复输入；凭证不写 meta、不落日志。
- `lib.rs` 注册；`tauriClient.ts` 新增 `POST /rag/pick-git` 路由；`ragService.pickRagGitRepo(url, branch, username?, password?)`。
- 上传：勾选的文件走既有 `upload_rag_doc` 循环（`original_path` = clone 目录内绝对路径，`method = copy`，md5 对源字节）。**symlink 对 git 无意义**——clone 目录本身就是「仓库的拷贝」。
- 后续该 repo 的**更新拉取**凭证优先从数据源注册表 + keyring 取（§3.6，首次认证成功即持久化，免重复弹框）；注册表无凭证且远程要求认证时：手动批量更新 → 发 `rag://git-auth-required` 事件（带 repo label）弹账密框，用户输入后经 `sync_rag_git_repo(url, branch, username, password)` 传入继续；自动更新 tick → 跳过该 repo 并记日志，不打断批量流程。

### 3.4 更新链路打通（复用 §3.12/3.13 体系）

- **更新策略 = 重新浅克隆 + 原子 swap**（作用于**持久目录**）：`refresh(dir)` clone 到 `{repo_hash}.new` → 成功后旧目录 rename `.old` → `.new` rename 正式名 → 删 `.old`。不做 gix 内 fetch+reset——depth 1 重克隆本就便宜，且免去 gix worktree reset / 本地脏树 / 浅仓库 fetch 的全部复杂度；`.old`/`.new` 残留由启动时清理兜底。更新流程同样先刷**持久目录**；若持久目录不存在（首次导入后从未落盘，例如旧版本升级）则先走临时 clone → 拷贝落盘路径。
- **单条更新**（`check_rag_update` / `update_doc_from_original`）：`source.git` 存在时，先对该 repo `refresh`（同 repo 去重，进程内 `OnceLock<Mutex<HashMap<hash, Instant>>>` 短期缓存 60s，避免逐文档重复拉取），再按 `original_path` 读字节走既有 md5 比对/重索引。远程变更 → md5 变化 → 正常检出「有更新」。
- **批量 / 自动定时更新**（`run_batch_update`）：预扫描阶段（`preview_batch_update` / tick）先做 **git 同步步骤**：收集本批文档涉及的 repo → 逐个 `refresh`（进度事件 phase 增加 `"git"`，文案「正在拉取仓库 X」；认证仓库无可用凭证 → 跳过并记日志，见 §3.3）→ 之后逻辑不变。
- **打开文件夹**：git 文档 → reveal clone 目录内的具体文件（`open_file_location` 加 git 分支）。
- **删除文档**：删 meta + 向量（同 copy 语义，clone 内内容文件不删）；**clone 目录生命周期**：删除文档后若该 repo 无任何文档引用 → 清理目录（引用计数 = 遍历 meta 统计，量级小可接受）。
- **swap 与读取并发**：`refresh` 的 rename swap 后旧路径短暂失效，读文件失败按「源暂时不可用」容错（`content_available` 置 false，下次重试恢复），不丢数据。

### 3.5 数据源注册表（认证持久化，按数据源而非按文件）

- **文件**：`<app_data>/rag/sources.json`（version + sources 数组）：

  ```json
  { "version": 1, "sources": [
    { "id": "<repo_hash>", "kind": "git", "url": "…", "branch": "main",
      "label": "https://…@main", "addedAt": "…", "credentialRef": "rag-git-<repo_hash>" }
  ]}
  ```

  `id` = `repo_hash`（与 clone 目录同名）。**凭证不进 DocSource、不进每个文档**——`DocSource.git` 只存 url/branch。
- **凭证存储**：OS 钥匙串（项目已依赖 keyring 3，JWT secret 同款用法）：service=`app.mcphub.desktop`，account=`rag-git-{repo_hash}`，值 = `username\npassword`。**首次带认证 clone 成功即写入**（无需勾选"记住"）；keyring 不可用 → 记 warn 日志、该源降级为会话级凭证（每次更新弹框），不阻断。
- **读取方**：单条检查更新 / 批量 / 自动定时更新的 git refresh 步骤统一从注册表 + keyring 取凭证——认证仓库**免重复弹框**；注册表无凭证且远程要求认证时才走 §3.3 的弹框（手动路径）/跳过+日志（自动路径）。
- **清理**：该 repo 最后一个文档被删除 → 连带删注册表条目 + keyring 凭证 + clone 目录（与 §3.4 生命周期合并实现）。
- **前端辅助（低成本）**：`list_rag_git_sources` 命令返回注册表条目，Git 表单提供「已导入数据源」下拉——选中自动带 url/branch，凭证由后端从 keyring 取，用户无感重拉。
- 加锁：与 META_LOCK 同思路的 `SOURCES_LOCK` 串行化 sources.json 读改写 + 原子写（tmp+rename）。

### 3.6 与重名文件的交互

- **现状确认（代码级）**：上传路径（`upload_one_path_inner`）已**不覆盖重名**——每次上传新建 uuid 文档，重名并存。**本设计将 `rag_file_create`（MCP 工具）也改为不覆盖**：删掉其 `find_doc_ids_by_name` 覆盖块，重名一律并存，全路径语义统一为「创建新文档，覆盖只走显式 update」。
- Git 更新链路不受重名影响：`update_doc_from_original` 按 `docId → meta.original_path` 精确对应，逐文档 md5 比对。

### 3.7 安全与边界

- 凭证持久化按 §3.5（keyring，按数据源）；命令入参中的账密同样**不写 meta、不写日志**；URL 内嵌凭证形式（`https://user:pass@host`）入参校验拒绝，提示走账密框。
- URL 白名单不做（本地工具，用户自主）；`repo_hash` 目录名只保留 hex 哈希，防路径注入。
- clone 目录不在 FTS / list 扫描范围（见 §3.2）。
- 仓库超大：浅克隆 depth 1 + `SCAN_FOLDER_FILE_CAP`（500）截断 + `folder_ignore.json` 天然兜底；进度可见、可中断。

---

## 4. 需求 3：列表树形结构切换

### 4.1 原则

- **树形仅是 GUI 展示层（硬性约束）**：不引入任何持久化树结构、不改 `list_docs` 返回形态、不影响 FTS/搜索/标签/分页——后端契约零变化。树由前端在内存中从 `sourceKind/sourceLabel/sourceRoot/relPath` 聚合构建（`list_docs` 一次拉全量，现状如此），搜索/批量操作仍作用于平铺全集。
- **平铺视图零改动**：现有搜索（FTS + 标签 + 文件名）、批量操作、徽章、按钮全部不动。

### 4.2 树结构

```
▾ 📁 数据源节点（按 sourceKind + sourceLabel 分组）
    · 「文件」（存量 + 单文件导入）      ← 需求 3 的硬性归属
    · 「文件夹」: /path/to/folder           ← 每个 folder root 一个节点
    · 「Git」: https://…@main               ← 每个 repo 一个节点
    · 「MCP 工具」（新 tool 文档，`rag_file_create`）
  ▾ 二级：物理目录（rel_path 目录链，虚拟节点，如 docs/、src/api/）
      ▸ 三级…：继续嵌套
        📄 叶子：文档行（复用现有行渲染：徽章/版本/操作按钮全保留）
```

- 节点折叠/展开（默认展开一级）；显示每个目录节点下文档计数。
- 树形模式下**保留**顶部搜索框：命中时自动展开包含命中的分支（简单实现：搜索激活时全展开）。
- 行操作按钮与平铺一致；批量删除/更新仍走现有头部按钮（不做树节点勾选，v1 降复杂度——记录为后续增强）。

### 4.3 组件

- `RagPage` 列表头部加切换（segmented：`List | FolderTree` 图标，或 lucide `List`/`FolderTree`）。
- 新组件 `frontend/src/components/ui/RagDocTree.tsx`：入参 docs + 渲染回调（叶子行复用父组件现有 JSX 提取成 `renderDocRow(doc)`），内部负责分组/排序/折叠 state。
- 排序：数据源节点固定序（文件夹 → Git → 文件选择 → 工具创建，或按 label 字母序——实现时定，倾向前者）；目录按名称序；叶子按现有平铺序（uploaded_at desc）。

---

## 5. i18n（4 语言，rag 命名空间）

新键（预估 ~22）：
`dataSource`（数据源）/ `dataSourceFile`（文件）/ `dataSourceFolder`（文件夹）/ `dataSourceGit` / `dataSourceTool`（MCP 工具分组）/ `gitRepoUrl` / `gitBranch` / `gitBranchPlaceholder` / `gitUsername` / `gitPassword` / `gitAuthHint`（可留空=匿名拉公共仓库）/ `gitAuthRequiredTitle`（认证对话框）/ `gitAuthRetry`（带凭证重试）/ `gitAuthAnonymousRetry` / `gitFetchAndScan` / `gitCloning` / `gitPulling` / `gitCloneFailed` / `gitUrlEmbeddedCredRejected` / `viewFlat` / `viewTree` / `treeDocCount` / `legacyFileSource`（存量归属文案）等；`scanDialog` 相关文案复用。

---

## 6. 实施计划（Phase 1-6）

> **进度（2026-09-09，全部完成 + 五轮复核）**：P1、P2、P5（UI 获用户确认）、P2b、P3、P4 ✅；P6 验证全绿（cargo check 0 错 0 警 / cargo test --lib 34 passed / npm build ✓ / tsc 24=基线）、版本 `1.0.35002` + changelog、AGENTS.md §3.17 已落盘。**五轮全量复核发现并修复 9 个问题**（validate_url 收紧 / is_auth_error 误报 / update_doc_from_file 换源归类 / git 强制 copy / swap 回滚 / PICK_LOCK / per-repo 刷新锁等），记录见 AGENTS.md §3.17.7。
> 实现细节与踩坑记录见 `AGENTS.md` §3.17。评审 mock 已全部移除（MOCK_DATA_SOURCE 等）。
> 未做项（低优先级）：`rag://git-sync-progress` 进度事件（clone 有 120s timeout + spinner 兜底）；私有仓库真机冒烟（GitHub PAT / GitLab，留用户手动验证）；SSH 协议支持（后续增强）。

| Phase | 状态 | 内容 | 涉及文件 | 验收 |
|---|---|---|---|---|
| **P1 数据模型 + 存量兼容** | ✅ 已完成 | `DocSource`/`GitSource` 模型（models/rag.rs）；`DocMeta.source` 字段；`classify_source` helper（存量：有 original_path → folder、无 → file）；`RagDocInfo`/`RagDoc` 6 个展示字段（sourceKind/Label/Root/relPath/gitUrl/gitBranch）+ `list_docs`/`doc_info_from_meta`/`get_doc_inner` 填充；`RagScanFile.rel_path` | `rag/service.rs`、`models/rag.rs` | cargo check ✓ |
| **P2 写路径注入 source** | ✅ 已完成 | `write_doc_and_index` 加 `source` 参数（末位，全调用点补齐）；`upload_one_path`/`upload_one_path_inner` 透传；`upload_rag_doc` 命令签名扩展（前端已传）；`rag_file_create` 注入 kind="tool"（label「MCP 工具」）；手动/from-original 更新保留原 source | `rag/service.rs`、`commands/rag.rs` | cargo check ✓ |
| **P2b rag_file_create 不覆盖** | ✅ 已完成 | 已删除 `create_doc_from_content` 的 `find_doc_ids_by_name` 覆盖块（文件/meta 清理 + 向量 `delete_by_doc` + `schedule_deferred_prune` + `remove_doc_sql`），重名并存；`find_doc_ids_by_name` 函数随之删除（无调用方，dead-code 警告已消）；工具文档 label 统一「MCP 工具」 | `rag/service.rs` | cargo check 0 警 ✓ |
| **P3 Git 集成（后端）** | ✅ 已完成 | ①gix 0.73 features：`blocking-network-client` + **`blocking-http-transport-reqwest-rust-tls`**（实测必须显式加，否则无 HTTP backend）+ `worktree-mutation`；reqwest 0.12（gix-transport）与项目 0.13 共存验证通过。②`rag/git.rs`：`clone_blocking`（`PrepareFetch::new` → `with_ref_name`（⚠️ 选分支用 ref_name 非 with_remote_name）→ `with_shallow(DepthAtRemote(NonZeroU32))` → `fetch_then_checkout` + `main_worktree`（⚠️ fetch_only 不 checkout 工作区））/ `refresh_persistent`（重克隆 `.new` + 原子 swap）/ `repo_hash` / `is_auth_error`。③注册表 + keyring。④命令 + lib.rs 注册 + 启动 `sweep_stale_dirs`。⑤`GIT_AUTH_REQUIRED:` 前缀。⑥两段式存储 + `map_temp_to_persistent`/`ensure_persisted` | `Cargo.toml`、新 `rag/git.rs`、`rag/service.rs`、`commands/rag.rs`、`lib.rs` | cargo check ✓；公共仓库真实冒烟 ✓（octocat/Hello-World depth=1+branch，临时测试已删）；私有仓库冒烟待用户 |
| **P4 更新链路打通** | ✅ 已完成 | ①`refresh_git_repo`（60s TTL 去重 `GIT_REFRESH_CACHE`）+ `git::refresh_registered`（注册表→keyring→refresh_persistent，未注册仓库匿名兜底）；接入单条 `check_rag_update`（best-effort）+ `preview_batch_update`（手动/自动均刷新，保证 md5 判定反映远端）+ `run_batch_update`（doc 循环前刷新；两处刷新均**并发** `join_all`，N 离线仓库只等 1 个超时；去重缓存使 preview+run 双刷新只 clone 一次）。②凭证缺失：刷新失败 warn 日志跳过（自动不打扰；未做 `rag://git-auth-required` 弹框，记为后续增强）。③`open_file_location` 无需改动——git 文档 original_path 已指向持久 clone。④引用计数：`delete_doc` 后 `count_git_docs_for_repo==0` → 删 clone + 注册表 + keyring。⑤前端 mock 全删（`MOCK_DATA_SOURCE`/`MOCK_GIT_SOURCES`/`MOCK_SOURCE_DOCS`/`MOCK_GIT_SCAN`/`mkMockDoc` 及全部分支） | `rag/git.rs`、`rag/service.rs`、`commands/rag.rs`、`lib.rs`、`RagPage.tsx` | cargo check ✓ + 手动回归（见下，待用户） |
| **P6 验证 + 文档** | ✅ 已完成 | `cargo check` 0 错 0 警；`cargo test --lib` 34 passed；`npm run build` ✓；`npx tsc --noEmit` 24=基线；`AGENTS.md` 新增 §3.17（设计/git.rs 踩坑/验证结论）；版本四源 `1.0.35001 → 1.0.35003`（changelog 与 35003 合并为单文件）；自审修复 2 处（`gitSourceMeta` 补 commit、repo 刷新并发化） | 全部 | 全绿 ✓（手动回归清单待用户） |

依赖顺序：P1 → P2 → P5 → P2b/P3 → P4 → P6（**全部完成**）。

### P3 实施顺序细化（风险前置）

> 全部 6 步已完成（第 4 步进度事件为有意取舍：clone 有 120s timeout + spinner 兜底，`rag://git-sync-progress` 记为后续增强；第 5 步私有仓库冒烟待用户真机验证）。

1. **gix 依赖可行性验证**（半天内）：加 `gix = { version, features = ["blocking-network-client", "worktree-mutation"] }` → `cargo check` → 确认 reqwest 归并结论（记入 §7 风险表）；
2. `rag/git.rs` 核心（repo_hash + clone_shallow + classify_auth_error，先不接进度/中断）+ 最小命令 `pick_rag_git_repo` → 公共仓库冒烟；
3. 注册表 sources.json + keyring + `list_rag_git_sources` → 关前端 mock 后「已导入数据源」下拉真实化；
4. 进度回调（`rag://git-sync-progress`，事件 payload 对齐上传双进度条的仓库级进度）+ should_interrupt（弹框关闭中断）；
5. 认证：带凭证 clone + `GIT_AUTH_REQUIRED` 错误前缀 → 私有仓库冒烟（GitHub PAT + GitLab 两种）；
6. `refresh` + swap → P4 接续。

### 手动验证清单（P4/P6）

- 文件/文件夹/Git 三种数据源各导入 → 列表树形视图中归入正确数据源节点（文件夹/Git 显示类型标签）、目录链正确。
- 存量旧文档（升级前导入）：有 original_path → 「文件夹」节点（父目录）；无 → 「文件」节点。
- Git 公共仓库：URL + depth=1 → clone → 扫描树 → 勾选导入 → 树形归 Git 节点 → `open_file_location` 打开 clone 目录。
- Git 私有仓库：匿名失败 → 表单内联报错 + 高级参数自动展开 → 补账密重试成功 → keyring 持久化 →「已导入数据源」出现该仓库 → 后续批量/自动更新免输入。
- Git 更新：远程 push 新提交 → 单条「检查更新」检出（md5 变化）→ 批量更新重索引；depth>1 生效。
- Git 存储两段式：扫描阶段 `mcphub-rag-git/` 临时目录有 clone、应用数据目录无；确认导入后 `rag/git/{repo_hash}` 出现；取消导入（关弹框）后应用目录无残留。
- Git 清理：仓库全部文档删除 → 持久 clone 目录 + 注册表条目 + keyring 凭证被清理（临时目录无需清理，OS 自愈）。
- 一键展开/收起：树形工具栏按钮收起/展开全树；手动点节点后 override 退出；搜索激活强制全展开。
- 关 mock 后回归：真实数据下平铺/树形切换、搜索、分页与 mock 期行为一致。
- 重名回归：同名文件两次导入 → 两文档并存；`rag_file_create` 重名 → **并存（P2b 后新语义）**。
- 回归：软链接/拷贝、md5 更新检测、批量/自动定时更新、FTS 搜索、PDF/Office/图片导入全部不受影响。

## 7. 风险与开放问题

| 风险/问题 | 处理 |
|---|---|
| gix 依赖树 reqwest 版本归并（项目主 reqwest 0.13 vs gix-transport 0.48 的 reqwest 0.12.22 blocking 后端） | **已验证（P3 实测）**：两版本共存单份编译，`cargo check` 无冲突，无需 curl 后端。⚠️ 另一个实测坑：`blocking-network-client` 本身**不带** HTTP client backend，必须显式加 `blocking-http-transport-reqwest-rust-tls`，否则运行时 clone 报 CompiledWithoutHttp |
| gix 浅克隆细节坑（Shallow 配置 / 空仓库 / 重定向） | **已实测两个坑**：①选 checkout 分支用 `with_ref_name`（`with_remote_name` 是设 remote 名）；②`fetch_only` 只取对象不 checkout 工作区，必须 `fetch_then_checkout` + `main_worktree`。公共仓库真实冒烟通过；`PrepareFetch` 失败自带目录清理，重试安全 |
| 私有仓库 SSH key | v1 不支持（仅 https + 账密/token）；SSH agent 透传记录为后续增强 |
| 认证仓库的自动定时更新 | 无可用凭证 → 刷新失败 warn 日志跳过（不弹框打断后台任务）；手动批量更新补凭证弹框（`rag://git-auth-required`）记为后续增强 |
| 大仓库 clone 慢 | 浅克隆 depth 1 + 120s 超时；仓库级刷新并发执行（join_all）；进度事件未做（spinner 兜底），partial clone filter 记为后续增强 |
| 同名文件跨数据源导入 | 上传路径不覆盖重名（并存）；**`rag_file_create` 同名覆盖已按用户要求移除（P2b）**，重名一律并存，覆盖只走显式 update |
| `rel_path` 对 file kind 恒为 "" | 文件数据源内部不再分层（单次多选本就无目录语义） |
| 树形 + 前端分页交互 | v1 树形模式一次渲染全量（文档量级几十~几百可接受）；量大后做虚拟滚动（记录待办） |
