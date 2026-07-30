# Skills（技能）菜单与导入/导出功能 — 实现计划

## Context

mcphub-desktop 目前没有 "skill" / "agent" 的概念。本需求要在左侧菜单新增 **Skills（技能）** 一级菜单，构建一个 skill 管理库：

- **导入弹框**（顶部「导入已有」按钮）：按用户配置的 **agent → skills 安装路径** 扫描各 agent 路径下的 skill 目录（**只跳过指向本应用库目录的符号链接**——即本功能导出时自己创建的产物；其余符号链接如 Claude Code 的 `~/.agents/skills` 集中管理视为正常 skill 跟随读 `SKILL.md`），读取每个 skill 的 `SKILL.md` 取名称与描述；弹框按 agent 分组、可折叠展开，每分组头显示 agent 名 + 技能数 + skillsPath（超长 `…` 截断 + 悬浮 tooltip）+ 📂「打开文件夹」按钮；已导入的 skill（按 dir_name 比对）禁用选择并标「已导入」。
- **导入**：把选中的 skill **复制**到 app 的统一库目录，成为本地管理库。
- **主列表**：分页显示库中所有 skill，按目录名排序；每行左 checkbox（多选）、中 name/dirName/「已安装的 Agent」徽标（每条导出显示方式图标 `🔗/📄` + agent 名）/description、右侧「安装」「查看」「删除」三个按钮。
- **安装弹框**（行内「安装」按钮，单技能）：agent 列表分「已安装 / 未安装」两段；每行 per-agent `[软链接 | 文件拷贝]` 方式切换（切换自动勾选）、已安装的显示「当前方式」徽标 + 卸载按钮；支持「添加自定义 Agent」（名字 + 文件夹路径系统选择器）；目标 Agent 标题旁 `?` 圆圈按钮（点击+悬浮展示软链接/文件拷贝区别与优点，离开自动消失）。
- **查看弹框**（行内「查看」按钮）：显示 skill 详情 + 「已安装的 Agent」列表（方式徽标 + 时间），每条可卸载（带二次确认）。
- **导出弹框**（多选触发「导出到 Agent」按钮）：可筛选的 agent 多选 + 软链接/文件拷贝 radio + 目标 Agent 标题旁 `?` 帮助；确认后按选择把库中的 skill 软链接/复制到目标 agent 路径（覆盖式重建，天然支持方式切换）。
- **删除弹框**（行内「删除」按钮）：库内副本必删；软链接导出**必删**（防悬挂链接）、文件拷贝导出**可选删**（复选框）。
- **已安装卸载**：安装弹框行内、查看弹框每条 export 行均可卸载单 (skill, agent) 安装（带二次确认）。
- **设置页**：新增「Agent 安装路径管理」卡片，增删改 agent（name + skillsPath，skillsPath 旁带 📂 文件夹选择器），按区块独立保存。

### 关键默认决策（用户未答复澄清问题，采用如下合理默认）

1. **agent→skills 路径 JSON 来源**：持久化到 SQLite `system_config.config_json` 的 `skills.agents` 数组（`{id, name, skillsPath}`），由 **设置页** 新增「Agent 安装路径管理」区块增删改。首次迁移时种子一组已知 agent 默认值（Claude Code / Cursor / Windsurf / Cline 等），用户可覆盖。无需打包额外 JSON 文件。
2. **agent 列表**：完全由上述配置驱动（导入弹框分组、导出弹框选项均来自此）。
3. **导入语义**：复制 skill 实体文件到 app 库目录（`$APPDATA/skills/<dirName>/`），原 agent 路径文件不动。软链接/复制的选择仅在**导出**环节。
4. **「查看」弹框的已安装 agent**：DB 跟踪导出记录（`skill_exports` 表），比文件系统探测更可靠，能正确处理已被外部清理的目标。

---

## 技术栈与既有模式（复用）

- Tauri v2 (Rust) + React 19 + TS + Tailwind + react-i18next；SQLite via **sqlx 非宏 API**（`sqlx::query` + `Row::try_get`）。
- 版本化迁移：`src-tauri/src/db/migration.rs`（`TARGET_VERSION` 常量 + `migrate_vN` + `apply_migration` match 分支）。
- 命令注册：`src-tauri/src/lib.rs` 的 `generate_handler!`。
- REST→command 映射：`frontend/src/utils/tauriClient.ts` 的 `mapRestToCommand` + `transformTauriResponse`。
- 列表页范式：`frontend/src/pages/PromptsPage.tsx`（`selectItemPage`/`getItemFilterCounts` from `utils/listFilters.ts`、`Pagination` 组件、`ConfirmDialog`、内联 modal `fixed inset-0 bg-black/50`）。
- 数据上下文范式：`frontend/src/contexts/BuiltinDataContext.tsx` + `hooks/useBuiltinPromptData.ts` + `services/builtinPromptService.ts`（`apiGet/apiPost/apiPut/apiDelete` from `utils/fetchInterceptor`）。
- 配置读写：`src-tauri/src/services/config_service.rs`（`get()`/`update(patch)` 深合并 `config_json`）。
- 侧边栏：`frontend/src/components/layout/Sidebar.tsx` 的 `workspaceItems` 数组。
- 路由：`frontend/src/App.tsx` lazy import + `<Route>`。
- FS 权限：`src-tauri/capabilities/default.json` 已含 `fs:scope-appdata-recursive` / `fs:scope-home-recursive` / `fs:allow-read-dir` 等（本功能 Rust 侧直接用 `std::fs`，不经 tauri-plugin-fs JS，故无需改 capability）。

---

## 阶段划分（先 UI 定稿，再后端）

按用户要求分两阶段，**先出 UI 供定稿，定稿后再做后端**：

### 阶段一：UI 定稿（仅前端，可运行可点）
- 完成前端全部界面：Sidebar 菜单、路由、`SkillsPage`（列表 + 分页 + 多选 + 行内安装/查看/删除）、五个弹框（ImportDialog / InstallDialog / ViewDialog / ExportDialog / DeleteSkillDialog）、SettingsPage 的「Agent 安装路径管理」卡片、i18n 键、新增类型、`MethodHelpIcon` 共享组件。
- **Mock 数据驱动**：后端命令尚未实现，在 `tauriClient.ts` 的 `mapRestToCommand` 中把 `/skills/*` 路由映射到 `__stub__`（已有 stub 机制，见 tauriClient.ts 中 `__stub__` + `__response`），返回硬编码示例数据：
  - `list_skill_agents` → 12 个示例 agent（Claude Code / Cursor / Windsurf / Cline / Roo Code / Continue / Aider / GitHub Copilot / Zed / Trae / Void / Gemini CLI），其中 GitHub Copilot 路径较长用于验证截断。
  - `list_skills` → 8 个示例 skill，其中 5 条带 exports（覆盖多 agent / 软链接 + 文件拷贝混合），验证列表「已安装」徽标与安装弹框「已安装」段。
  - `scan_skills_for_import` → 6 个 agent、13 条 skill，含与已导入重名（验证「已导入禁用」）与未导入（验证可勾选导入）。
  - `get_skill` → 对 `s2`(code-review)、`s7`(deep-research) 返回与列表一致的丰富 exports，其余返回空，验证 ViewDialog。
  - `import_skills` / `export_skills_to_agents` / `save_skill_agents` / `delete_skill` / `uninstall_skill` / `open_path_in_explorer` / `pick_directory` → `__stub__` 返回 `{success:true}`（`pick_directory` 返回 mock 路径 `/Users/demo/custom-skills`）。前端走通流程即可，不真写盘/不真开系统对话框。
  - `transformTauriResponse` 对 `__stub__` 已天然走 `args.__response`，无需特判。
- 交付目标：`npm run tauri dev` 启动后可在侧边栏进 Skills 页，完整点通导入 / 安装（含自定义 agent + 文件夹选择）/ 查看（含卸载）/ 导出（含 `?` 帮助）/ 删除（含关联文件清理）/ 设置页 agent 路径卡片（含文件夹选择），分页排序、多选导出按钮出现/隐藏。**用户定稿 UI**。

### 阶段二：后端实现 + 联调（UI 定稿后）
### 阶段二：后端实现 + 联调（UI 定稿后）
拆为 6 个可独立验证的子阶段，每步 `cargo check` + 局部联调通过后再进下一步：

- **2.1 基础设施**：DB 迁移 `migrate_v13`（`skills` + `skill_exports` 表 + `skills.agents` 种子）+ `migrations/0013_skills.sql` 占位 + `models/skill.rs`（5 结构体）+ `models/mod.rs`/`services/mod.rs`/`commands/mod.rs` 声明。验证：启动迁移无报错、表已建。
- **2.2 Agent 配置 + 设置页联调**：`list_agents`/`save_agents` service + `list_skill_agents`/`save_skill_agents` 命令 + `lib.rs` 注册 + `tauriClient` 把 `/skills/agents`(GET/PUT) 两路由从 `__stub__` 改回真实命令。验证：设置页 agent 路径卡片持久化（重启仍在）、种子默认 agent 首启已存在。
- **2.3 库目录 + 扫描 + 导入**：`home_dir`/`resolve_agent_path`/`library_dir`/`copy_dir_recursive` + `scan_for_import`/`list_library`/`get_skill`/`import_skills` service + 命令 + `tauriClient` 把 `/skills`、`/skills/:id`、`/skills/scan`、`/skills/import` 改回真实命令。验证：导入弹框真实扫描（读 `SKILL.md`、只跳过指向库目录的符号链接、`~` 展开）、导入到库目录、列表分页排序显示。
- **2.4 安装/导出（幂等重建）**：`export_to_agents` service + 命令 + `tauriClient` 把 `/skills/export` 改回真实命令。验证：安装/导出弹框真实软链接/文件拷贝、软链接↔文件拷贝切换、同方式刷新（先删后建）。
- **2.5 卸载 + 删除（关联清理）**：`uninstall_skill`/`delete_skill` service + 命令 + `tauriClient` 把 `/skills/uninstall`、`/skills/delete` 改回真实命令。验证：卸载单 (skill, agent)、删除带关联清理（软链接必删、文件拷贝可选删）。
- **2.6 系统集成 + 全流程联调**：`open_path_in_explorer`/`pick_directory` 命令 + capability 补 `dialog:allow-open` + `tauriClient` 把 `/skills/open-path`、`/skills/pick-directory` 改回真实命令 + 端到端全流程联调。验证：打开文件夹、系统文件夹选择器、删除 `DELETE /skills/:id` 兼容路由可删可留、`doc/agent_20260724.md` 更新。

> 子阶段顺序有依赖（2.1 是其它所有步骤的前置；2.3 依赖 2.1；2.4/2.5 依赖 2.3；2.6 依赖 2.2–2.5）。每步完成后 `cd src-tauri && CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse cargo check`（按 memory 用本地代理 `127.0.0.1:7890`、rustup toolchain）+ 对应前端路由 stub→真实切换后 `npx vite build` + `npm run tauri dev` 局部联调。

---



## 后端改动（Rust）— 阶段二实现

### 1. DB 迁移（`migration.rs`，`TARGET_VERSION` 12 → 13，新增 `migrate_v13` + match 分支 13）

```sql
CREATE TABLE IF NOT EXISTS skills (
  id          TEXT PRIMARY KEY,
  dir_name    TEXT NOT NULL UNIQUE,
  name        TEXT,
  description TEXT,
  source_agent TEXT,
  source_path TEXT,
  status      TEXT NOT NULL DEFAULT 'pending',  -- 'pending' | 'ok'；仅 'ok' 视为导入成功
  created_at  TEXT NOT NULL DEFAULT (datetime('now','localtime'))
);
CREATE TABLE IF NOT EXISTS skill_exports (
  id         TEXT PRIMARY KEY,
  skill_id   TEXT NOT NULL REFERENCES skills(id) ON DELETE CASCADE,
  agent_id   TEXT NOT NULL,
  method     TEXT NOT NULL,            -- 'symlink' | 'copy'（实际采用的方式）
  status     TEXT NOT NULL DEFAULT 'pending',  -- 'pending' | 'ok'；仅 'ok' 视为安装成功
  created_at TEXT NOT NULL DEFAULT (datetime('now','localtime')),
  UNIQUE(skill_id, agent_id)
);
```
> **状态字段（防 crash 误判成功）**：`skills.status` / `skill_exports.status` 仅在导入/导出**完整成功后**写 `ok`；中途插入时为 `pending`。`list_library`/`get_skill`/列表徽标**只统计 status='ok'** 的记录——status≠ok 即使文件恰好在也按**未成功**处理。启动时跑 `reconcile_pending()`：扫描 `status='pending'` 的 `skills` → 删 `library_dir/<dir_name>` 残留目录 + 删行；`status='pending'` 的 `skill_exports` → 删 `<agent_path>/<dir_name>` 残留 + 删行。这样 crash 中断后下次启动不会误判成功，且磁盘状态一致。
同 migration 内：若 `config_json` 无 `skills.agents`，写入一组种子默认值（Claude Code `~/.claude/skills`、Cursor `~/.cursor/skills`、Windsurf `~/.codeium/windsurf/skills`、Cline `~/.cline/skills`）。

> **home 路径解析（跨平台，区分 mac/linux/windows）**：skill 的安装路径是**用户 home 根**下的目录（如 `~/.claude/skills`），与 app 数据目录（`app_data_dir` 拼了 `mcphub-desktop` 子目录）不同。`Cargo.toml` 无 `dirs`/`directories`，无现成 home 解析器。沿用 `runtime_env::app_data_dir`（`src-tauri/src/services/runtime_env.rs:725`）的 `#[cfg(target_os = ...)]` 逐平台环境变量惯例，在 `skill_service.rs` 内新增公共助手（不拼 `mcphub-desktop`）：
> ```rust
> /// 用户 home 根（跨平台）：mac/linux=HOME，windows=USERPROFILE
> pub fn home_dir() -> Option<PathBuf> {
>     #[cfg(target_os = "windows")]
>     let h = std::env::var("USERPROFILE").ok().map(PathBuf::from);
>     #[cfg(not(target_os = "windows"))]
>     let h = std::env::var("HOME").ok().map(PathBuf::from);
>     h
> }
> ```
> 用户在设置页填的 `skillsPath` 支持两种形式：`~/.claude/skills`（以 `~` 开头，用 `home_dir()` 展开）或绝对路径（`/Users/...`、`C:\Users\...` 直接用）。扫描/导出时统一在 `skill_service::resolve_agent_path(raw)` 内归一：以 `~` 开头 → `home_dir()?.join(strip 前导 ~)`，否则原样 `PathBuf::from`。`~` 无法展开或路径不存在时该 agent 跳过（返回 message 提示）。
>
> 选用「自研 home 助手」而非加 `dirs` 依赖，理由：① 与 `runtime_env::app_data_dir` 既有 `#[cfg]` 风格一致；② 零新依赖、零编译风险；③ skill 路径只需 HOME/USERPROFILE，不需要 `dirs` 的 config/cache 等细分目录。若后续需更稳健的 home 解析（如 Windows 回退 `HOMEDRIVE+HOMEPATH`），可在该助手内增量补充，不影响调用方。
> 配套补 `migrations/0013_skills.sql` 占位（沿用编号命名；注意：`migrations/*.sql` 是旧的 `sqlx::migrate!` 体系，真正生效的是 `migration.rs` 的 `migrate_vN` 函数，SQL 文件仅作兼容占位，按既有约定保留）。

### 库目录解析
`library_dir(app)` 用 `app.path().app_data_dir()`（同 `db/mod.rs:6` 既有用法），需在该命令文件 `use tauri::Manager` 以获得 `.path()`。库目录 = `app_data_dir().join("skills")`。

### 2. Model：`src-tauri/src/models/skill.rs`
```rust
#[serde(rename_all = "camelCase")]
struct SkillAgent { id, name, skills_path }
struct ScannedSkill { agent_id, agent_name, dir_name, name, description, path }
struct Skill { id, dir_name, name, description, source_agent, source_path, created_at, exports: Vec<SkillExport> }
struct SkillExport { agent_id, agent_name, method, created_at }
struct ExportResultItem { skill_id, agent_id, success, message }
```
在 `models/mod.rs` 加 `pub mod skill;`。

### 3. Service：`src-tauri/src/services/skill_service.rs`
- `library_dir(app: &AppHandle) -> anyhow::Result<PathBuf>`：`Ok(app.path().app_data_dir()?.join("skills"))`。
- `list_agents() -> Vec<SkillAgent>`：读 `config_service::get()` 的 `skills.agents`，无则返回种子。
- `save_agents(agents: Vec<SkillAgent>)`：`config_service::update(json!({"skills":{"agents":agents}}))`。
- `scan_for_import(app) -> Vec<ScannedSkill>`：遍历每个 agent 的 `skills_path`；对每个子目录用 `symlink_metadata`，**跳过符号链接**（`file_type().is_symlink()` 为真则跳过）；读取 `<dir>/SKILL.md`，解析 YAML frontmatter 取 `name`/`description`（无则 `name=dir_name`、`description=""`）。frontmatter 解析手写：取 `---` 包裹的首段，按行 `key: value` 提取。
- `list_library(app) -> Vec<Skill>`：`SELECT ... FROM skills WHERE status='ok' ORDER BY dir_name`，聚合 `skill_exports` 中 `status='ok'` 的导出（每行附 `exports`）。导出列表用一次 `SELECT skill_id, agent_id, method, created_at FROM skill_exports WHERE status='ok'` + 内存分组，避免 N+1。agent 名称按 `agent_id` 从配置映射。**只统计 ok**——pending 不计入已安装。
- `get_skill(id) -> Option<Skill>`：含 `status='ok'` 的 exports。
- `reconcile_pending()`：启动时（`db::initialize` 或 `lib.rs` setup）调用。扫描 `skills WHERE status='pending'` → 删 `library_dir/<dir_name>` 残留目录 + `DELETE`；`skill_exports WHERE status='pending'` → 解析 agent 路径删 `<agent_path>/<dir_name>` 残留 + `DELETE`。使 crash 中断后下次启动磁盘与 DB 一致、不误判成功。
- `import_skills(app, items: Vec<{agent_id, dir_name}>) -> Vec<{dir_name, success, message}>`：对每项，源 = 对应 agent 的 `skills_path/dir_name`，目标 = `library_dir/dir_name`：
  1. 若已有 `status='ok'` 的 `skills` 行（dir_name 命中）→ 跳过（`success=false, message="already imported"`）。
  2. `INSERT INTO skills(..., status='pending')`（`id=Uuid::new_v4`，导入时解析 SKILL.md 填充 name/description）——**先落 pending 行占位**。
  3. `copy_dir_recursive(src, target)`（见下，**先删后拷、不增量覆盖**）。
  4. 成功 → `UPDATE skills SET status='ok' WHERE id=?`（**只在此刻算成功**）；失败 → 删 target 残留 + `DELETE FROM skills WHERE id=?`，返回 failure。
  5. crash 在 3–4 之间 → 行留 `pending` → `list_library` 过滤掉 + 启动 `reconcile_pending()` 清残留目录与行。
- `copy_dir_recursive(src, dst)`：**若 dst 存在先 `remove_dir_all(dst)` 再拷**，绝不增量覆盖（防止旧版本被删的文件残留）；`create_dir_all(dst.parent())` 后 `fs::read_dir` + `fs::copy` 递归；遇到符号链接复制链接目标内容——保持库为实体文件。
- `export_to_agents(app, skill_ids: Vec<String>, agent_ids: Vec<String>, method: String) -> Vec<ExportResultItem>`：双层循环；对每个 `(skill, agent)`：
  - 取 skill `dir_name`、agent `skills_path`；`fs::create_dir_all(agent_path)`。
  - `INSERT` 或 `UPDATE` 对应 `skill_exports` 行为 `status='pending'`（占位）。
  - 目标 `agent_path/dir_name`：**无论原方式与新方式是否相同，都先 `remove_dir_all`/`remove_file` 删除原有安装再重装**（`symlink_metadata` 判断删链接还是删实体目录）；**copy 方式走 `copy_dir_recursive`（内部先删后拷、不增量覆盖）**。理由：库内文件可能已更新，需把最新内容重新分发——防止旧版本被删的文件残留 + 防止链接失效/目标变更。
  - 此「先删后建」天然支持安装方式切换（软链接↔文件拷贝）与同方式刷新两种场景。
  - 成功 → `UPDATE skill_exports SET status='ok', method=<实际方式> WHERE id=?`（**只在此刻算成功**）；失败 → 清残留 target + `DELETE` 该行，记录 failure。
  - crash 在建/更新之间 → 行留 `pending` → `get_skill`/列表徽标不统计 + 启动 `reconcile_pending()` 清残留与行。
  - `symlink`：**真符号链接（支持跨卷；自定义目录场景必需）**。mac/linux：`std::os::unix::fs::symlink(library_dir/dir_name, target)` 直连（无需特权）。Windows：先试无提权 `std::os::windows::fs::symlink_dir(target, link)`；若因权限失败（`ERROR_PRIVILEGE_NOT_HELD`/拒绝访问）→ 把本批所有软链接目标打包进一份 manifest 临时文件，经 `ShellExecuteExW("runas", current_exe, "--symlink-helper --manifest <tmp>")` 拉起**主 exe 自身的提权副本**创建（UAC 显示应用自身签名身份；一次 manifest = 一次 UAC，避免 N×M 弹窗）；拒绝/仍失败 → 返回 failure + 提示改选文件拷贝（**不静默回退复制**）。不用 junction（不可跨卷 + Rust std 对其 reparse point 检测有坑）。helper 是主 exe 的 `#[cfg(windows)]` 隐藏 CLI 分支，无独立 sidecar——详见 `doc/agent_20260724.md` §3.8.5。
  - **删除安全**：symlink-to-dir 同样有 footgun——**绝不 `remove_dir_all` 链接**（会顺着链接递归删库内真实文件）；统一 `remove_link`：`symlink_metadata` 判断为 symlink → `remove_dir`/`remove_file`（删链接本身），实体目录才 `remove_dir_all`。卸载/删除均不提权。
  - `copy`：`copy_dir_recursive(library_dir/dir_name, target)`。
  - 记录 success/message。
  > 注：**批量安装（ExportDialog 多选）与单个安装（InstallDialog 行内）共用此命令**，走同一「占位 pending → 先删后建 → 成功写 ok」逻辑；前端 InstallDialog 按 per-agent 方式分组多次调用（每次一种 method），ExportDialog 单次调用（全局 method），但每次调用内部均执行先删后建 + 状态机。
- `open_path_in_explorer(app, path: String) -> ()`：**新增**（满足「打开对应文件地址」）。`resolve_agent_path` 展开 `~`（mac/linux=HOME、windows=USERPROFILE）后，用 `tauri_plugin_shell` 的 `Opener`（`tauri_plugin_shell` 已在 `lib.rs` 注册、`shell:allow-open` 已在 capability 授权）调用系统打开：mac `open`、linux `xdg-open`、windows `explorer`。无 JS 绑定故走自定义命令（同 `save_settings_json` 范式）。路径不存在时返回 `Err` 提示。
- `pick_directory(app) -> Option<String>`：**新增**（满足「文件夹路径用系统选择器」）。用 `tauri_plugin_dialog` 的 `DialogExt`（`tauri_plugin_dialog` 已注册；capability 需补 `dialog:allow-open` 权限，或在 Rust 命令内直接调 `app.dialog().file().set_directory(..).blocking_pick_folder()`——后端调用不受前端 capability 限制，但保险起见补 `dialog:allow-open`）。返回用户选择的绝对路径，取消返回 `None`。无 JS 绑定故走自定义命令。
- `delete_skill(app, id, cleanup_agent_ids: Vec<String>) -> bool`：**签名变更**（满足「删除时是否删除关联文件」）。流程：
  1. `SELECT dir_name` + 该 skill 的全部 `skill_exports`。
  2. 软链接方式的导出**必删**：遍历 `method='symlink'` 的 exports，删除 `<agent_path>/<dir_name>` 符号链接（否则库副本删除后留悬挂链接）。不受 `cleanup_agent_ids` 控制。
  3. 文件拷贝方式的导出**可选删**：仅当该 agent_id 在 `cleanup_agent_ids` 中时，删除 `<agent_path>/<dir_name>` 实体目录。
  4. `fs::remove_dir_all(library_dir/dir_name).ok()` 删库内副本。
  5. `DELETE FROM skills WHERE id=?`（外键级联清 skill_exports）。
  - 返回 true。
- `uninstall_skill(app, skill_id, agent_id) -> bool`：**新增**（满足「已安装的卸载」）。按 `(skill_id, agent_id)` 查 `skill_exports` 取 method；解析 agent `skills_path` + skill `dir_name`，删除目标 `<agent_path>/<dir_name>`（软链接删链接、文件拷贝删实体目录）；`DELETE FROM skill_exports WHERE skill_id=? AND agent_id=?`。返回 true。前端在 InstallDialog 行内、ViewDialog 每条 export 行各提供卸载按钮（带 ConfirmDialog 二次确认）。
- `services/mod.rs` 加 `pub mod skill_service;`。

> **自定义 agent 持久化**：安装弹框「添加自定义 Agent」（名字 + 文件夹路径）调用 `save_skill_agents([...existing, custom])` 把自定义 agent 写入 `config_json.skills.agents`，随即可被扫描/导出/设置页使用。`export_to_agents` 按 agent_id 从配置取 `skills_path`，因此自定义 agent 必须先持久化（前端已这么做）。
> **per-agent 安装方式**：InstallDialog 按 agent 逐行选 symlink/copy；前端按方式分组多次调用 `export_skills_to_agents`（每次一种 method），后端单次调用仍接收单一 method——无需改后端签名即可支持 per-agent 方式与切换。每次调用内部均执行「先删原有 → 按当前选择重装」，同方式也会重建（刷新已更新的库文件）。

### 4. Commands：`src-tauri/src/commands/skills.rs`
每个 service 函数包一层 `#[tauri::command]`，`app: AppHandle` 由 Tauri 注入：
`list_skill_agents`、`save_skill_agents`、`scan_skills_for_import`、`list_skills`、`get_skill`、`import_skills`、`export_skills_to_agents`、`delete_skill`(id, cleanup_agent_ids)、`uninstall_skill`(skill_id, agent_id)、`open_path_in_explorer`(app, path)、`pick_directory`(app)。错误统一 `.map_err(|e| e.to_string())`。
`commands/mod.rs` 加 `pub mod skills;`。

### 5. 注册：`lib.rs` 的 `generate_handler!` 末尾追加上述 11 个命令。

---

## 前端改动 — 阶段一（UI 定稿）

> 阶段一全部用 mock 数据驱动（见上方「阶段一 · Mock 数据驱动」）。以下路由映射在阶段一先全部指向 `__stub__` + 硬编码示例；阶段二再改回真实命令名。下面列出**阶段二的最终映射**供对照，阶段一只需把这些 command 名替换成 `__stub__`。

### 1. `utils/tauriClient.ts` — `mapRestToCommand` 新增
```
GET  /skills/agents          -> list_skill_agents
PUT  /skills/agents          -> save_skill_agents {agents: body}
GET  /skills/scan            -> scan_skills_for_import
GET  /skills                 -> list_skills
GET  /skills/:id             -> get_skill {id}
POST /skills/import          -> import_skills {items: body}
POST /skills/export          -> export_skills_to_agents {skillIds, agentIds, method}
POST /skills/open-path       -> open_path_in_explorer {path}        # 阶段一 stub
POST /skills/pick-directory  -> pick_directory {}                   # 阶段一 stub（返回 mock 路径）
POST /skills/delete          -> delete_skill {id, cleanupAgentIds}  # 阶段一 stub；软链接必删、文件拷贝可选删
POST /skills/uninstall       -> uninstall_skill {skillId, agentId}  # 阶段一 stub；卸载单 (skill, agent) 安装
DELETE /skills/:id           -> delete_skill {id}                   # 兼容保留
```
`transformTauriResponse` 对返回数组的命令走既有 `Array.isArray` 通用分支即可，无需特判。

### 2. `types/index.ts` 新增类型：`SkillAgent`、`ScannedSkill`、`Skill`、`SkillExport`、`ExportResultItem`（camelCase）。

### 3. `services/skillService.ts`（新）：用 `apiGet/apiPut/apiPost/apiDelete` 封装上述端点，含 SettingsPage 用的 `updateSkillsAgents`。

### 4. `hooks/useSkillData.ts`（新）：仿 `useBuiltinPromptData`，自管 `skills/loading/error/refresh` + `importSkills/exportSkills/removeSkill`。**不并入 `BuiltinDataContext`**（保持独立，降低耦合；侧边栏 badge 留空 undefined）。

### 5. `components/layout/Sidebar.tsx`：`workspaceItems` 在 resources 与 market 之间插入
`{ path: '/skills', label: t('nav.skills'), icon: <Wrench className="h-4 w-4" /> }`（从 `lucide-react` 导入 `Wrench` 扳手图标；无 badge）。

### 6. `App.tsx`：`lazy(() => import('./pages/SkillsPage'))` + `<Route path="/skills" element={<SkillsPage/>} />`。

### 7. `pages/SkillsPage.tsx`（新，仿 `PromptsPage.tsx` 结构 + hub-* 样式）

**共享组件 `MethodHelpIcon`**：`HelpCircle` 圆圈按钮 + `absolute` 气泡；**点击 + 悬浮均展示**，`onMouseLeave` 150ms 延迟关闭（桥接按钮与气泡缝隙，气泡可悬浮阅读），离开自动消失。内容为软链接/文件拷贝区别与优点（`symlinkHelp` / `fileCopyHelp`）。用于安装/导出弹框「目标 Agent」标题旁。

**头部**：`hub-h1` 标题 + 副标题（count）；右上 `hub-btn primary` 「导入已有」（`Plus` 图标）→ 开 ImportDialog。

**Toolbar**：搜索框（过滤 `dir_name + name + description`）+ 右侧 count；选中时右侧出现 `hub-btn primary`「导出到 Agent」（带选中数）。

**列表**：`useMemo` 先 `[...skills].sort((a,b)=>a.dir_name.localeCompare(b.dir_name))` 再传 `selectItemPage`（其本身不排序）。每行：左 checkbox（受控 `Set<string>`）、中 name（粗）+ dirName（mono）+ 「已安装的 Agent」徽标行（每条 export：方式图标 `Link2`/`CopyIcon` + agent 名，hover 显示方式）+ description（truncate）、右「安装」`PackageCheck`（accent 色）、「查看」`Eye`、「删除」`Trash2`（err 色，admin only）。

**分页 footer**：复用 `Pagination` + 每页条数 `<select>`（5/10/20/50），同 PromptsPage。

**ImportDialog**（内联 modal）：
- 加载调 `scan_skills_for_import`，spinner。
- 已导入 dirName 集合 = `new Set(skills.map(s=>s.dir_name))`。
- 按 agent 分组可折叠（ChevronDown 旋转）；分组头显示 agent 名 + 可选数/总数 + skillsPath（`truncate flex-1 min-w-0` + `title` 悬浮全路径）+ 📂「打开文件夹」`Folder` 按钮（调 `openAgentPath`，toast）。
- 每分组下 skill 行：checkbox + name + description + dirName；重名行 checkbox 禁用 + 「已导入」灰标签。
- 底部「取消」+「导入选中」（统计数）；提交调 `importSkills`，成功 refresh + 关闭。

**InstallDialog**（行内「安装」按钮，单技能）：
- 加载调 `list_skill_agents`；`installedMethods` 本地 Map（init 自 `skill.exports`，卸载后乐观更新）。
- agent 列表分「已安装 (N)」/「未安装 (M)」两段（空段隐藏标题），均随搜索过滤。
- 每行：checkbox + name + （已安装的）「当前 软链接/文件拷贝」徽标 + （切换时）「将切换为 X」徽标 + skillsPath（truncate + title）+ per-agent `[软链接 | 文件拷贝]` 分段控件（`setMethodFor` 切换时**自动勾选**该 agent）+ （已安装的）红色 `Trash2` 卸载按钮。
- 「添加自定义 Agent」表单：name 输入 + path 输入（带 `Folder` 选择按钮调 `pickDirectory` 回填）+ 「添加」按钮（校验非空 → `save_skill_agents([...existing, custom])` 持久化 → 加入本地列表、自动勾选、toast）。
- 底部「将导出 1 skill → N agent」+「取消」+「安装」；提交按方式分组多次调 `exportSkills([skillId], agentIds, method)`。
- 卸载走 ConfirmDialog 二次确认 → `onUninstall(skill.id, agentId)` → 成功乐观移除 installedMethods + 取消勾选 + toast。

**ViewDialog**（行内「查看」按钮）：加载调 `getSkill`；显示 name/dirName/description + 「已安装的 Agent」列表（每条 agent 名 + 方式徽标 + 时间 + 红色 `Trash2` 卸载按钮）；卸载走 ConfirmDialog → `onUninstall` → 成功 refetch + toast。

**ExportDialog**（多选触发）：搜索 input + agent 复选列表（每行 checkbox + name + skillsPath truncate+title）；「目标 Agent」标题旁 `MethodHelpIcon`；radio `软链接 / 文件拷贝`；底部「将导出 N skill → M agent」+「取消」+「导出」→ `exportSkills(skillIds, agentIds, method)` → toast + refresh + 清空选中。

**DeleteSkillDialog**（行内「删除」按钮）：显示「将删除库内副本: dirName」+ 软链接导出**必删**列表（不可选）+ 文件拷贝导出**可选删**复选列表；底部「取消」+「删除」→ `deleteSkill(id, cleanupAgentIds)`（`cleanupAgentIds = 软链接 agent 全部 ∪ 勾选的文件拷贝 agent`）→ toast + 清选中。

### 8. `pages/SettingsPage.tsx`：新增「Agent 安装路径管理」卡片（`SkillsAgentsCard` 自管 state）。该页采用**按区块独立保存**模式，故本卡片自带本地 state + 保存按钮，新增前端 service `updateSkillsAgents`（= `saveSkillAgents`，`apiPut('/skills/agents', agents)`）；增删 agent 行、编辑 `name`/`skillsPath`，每行 skillsPath 输入框右侧带 `Folder` 文件夹选择按钮（调 `pickDirectory` 回填，Phase 1 mock 路径），底部「+ 添加 Agent」+ 保存。`sectionsVisible` 新增 `skillsAgents` 键 + `toggleSection` 联合类型补 `'skillsAgents'`。图标用已导入的 `Wrench` + `Folder`、`Plus`、`Trash2`。

### 9. i18n：`locales/en.json`、`zh.json`（fr/tr 走 fallbackLng='en'）新增
`nav.skills`、`pages.skills.title`、`skills.*`（importExisting/importDialogTitle/scanLoading/scanError/alreadyImported/importSelected/import/importSuccess/importError/importFirst/noAgents/installedAgents/installedCount/installedSection/notInstalledSection/emptyExports/viewSkill/view/empty/fetchError/exportToAgent/exportDialogTitle/exportTargetAgents/searchAgents/exportMethod/exportMethodHelp/symlink/fileCopy/symlinkHelp/fileCopyHelp/exportSummary/export/exportSuccess/exportError/openFolder/openingPath/openPathError/install/installDialogTitle/installTargetAgents/installMethod/currentMethod/switchTo/installSuccess/installError/deleteDialogTitle/deleteLibraryCopy/deleteSymlinkMandatory/deleteCopyOptional/deleteNoExports/confirmDeleteBtn/uninstall/uninstallConfirm/uninstallSuccess/uninstallError/addCustomAgent/customAgentName*/customAgentPath*/pickFolder/customAgentNameRequired/customAgentPathRequired/customAgentAdded/customAgentAddError/pickFolderError）、`settings.skillsAgents*`（title/description/namePlaceholder/pathPlaceholder/addSkillsAgent/saved/saveError）。

---

## 验证

### 阶段一（UI 定稿）
1. `cd frontend && npx vite build`（前端构建通过；根 `npm run build` 走 `tauri build` 需 cargo，Phase 1 不必）。
2. `npx tsc --noEmit` 无**新增**类型错误（基线已有 ~56 个预存错误，均不在本功能文件）。
3. `npm run tauri dev` 启动（后端仍是 mock stub）：
   - 侧边栏出现 Skills 菜单（扳手图标），点开显示 mock 列表（5 行带「已安装的 Agent」徽标，3 行无）。
   - 设置页「Agent 安装路径管理」卡片可增删改 12 个 mock agent；skillsPath 旁 📂 按钮回填 mock 路径。
   - 「导入已有」弹框：按 agent 分组、可折叠、显示名称描述 + 路径（超长 `…` 截断 + 悬浮全路径）+ 📂 打开按钮；重名项禁用并标「已导入」；未导入项可勾选导入 → 列表刷新。
   - 行尾「安装」弹框：已安装/未安装两段；per-agent 方式切换自动勾选；切换时显示「将切换为」；自定义 agent 表单（名字+文件夹选择）；`?` 圆圈按钮点击+悬浮展示软链接/文件拷贝说明、离开消失；已安装 agent 卸载按钮二次确认。
   - 行尾「查看」弹框：已安装 agent + 方式徽标 + 时间 + 卸载按钮。
   - 多选 →「导出到 Agent」弹框：搜索 agent、`?` 帮助、软链接/文件拷贝 radio。
   - 行尾「删除」弹框：库副本必删 + 软链接必删 + 文件拷贝可选删。
4. **交用户定稿 UI**，按反馈调整样式/交互；定稿后进入阶段二。

### 阶段二（后端 + 联调，按 2.1–2.6 子阶段逐步验证）
每个子阶段完成后：
- `cd src-tauri && CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse cargo check`（按 memory 用本地代理 `127.0.0.1:7890`、rustup toolchain）。
- 该子阶段涉及的 `/skills/*` 路由 stub→真实命令切换后 `cd frontend && npx vite build` + `npx tsc --noEmit` 无新增错误。
- `npm run tauri dev` 按下表局部联调：

| 子阶段 | 联调点 |
|---|---|
| 2.1 | 启动迁移无报错；`skills`/`skill_exports` 表已建；`config_json.skills.agents` 含 4 个种子 |
| 2.2 | 设置页 agent 路径增删改 → 持久化（重启仍在）；种子默认 agent 首启已存在 |
| 2.3 | 「导入已有」真实扫描各 agent 路径、读 SKILL.md、**只跳过指向库目录的符号链接**、`~` 展开；已导入禁用；选中导入 → 列表分页排序显示 |
| 2.4 | 安装/导出弹框真实软链接/文件拷贝；软链接→文件拷贝切换（先删后建）；同方式重装刷新内容；「查看」已安装 agent + 方式标签；`ls -l` 验证软链接指向库 |
| 2.5 | 卸载单 (skill, agent) 删目标 + DB 行；删除弹框软链接必删、文件拷贝可选删 |
| 2.6 | 📂 打开文件夹真实调系统 opener；文件夹选择器弹原生对话框；重新「导入已有」时软链接（导出产生）被扫描跳过、原生实体目录可见但「已导入」禁用 |

最后：更新 `doc/agent_20260724.md` §3.8.3 子阶段勾选完成项 + §7 待办勾选（按 AGENTS.md 约束：较大改动须更新文档）。

## 备注
- 软链接用**真符号链接**(支持跨卷 + 自定义目录)。mac/linux 直连;Windows 先试无提权 `symlink_dir`,权限不足时经主 exe 隐藏 flag `--symlink-helper` + `ShellExecuteExW("runas")` 提权创建(UAC 显示应用自身签名身份);拒绝/失败 → 提示改选文件拷贝(不静默回退复制)。不用 junction(不可跨卷 + Rust std 检测有坑)。
- 删除链接绝不 `remove_dir_all`(会顺着链接删库内真实文件),统一 `remove_link`(`symlink_metadata` 判断:链接 `remove_file`/`remove_dir`,实体目录才 `remove_dir_all`)。

## 联调迭代修正（实施记录，2026-07-29）

> Phase 2 代码完成后联调中按反馈逐项修正。**核心原则:检测一律走文件系统实际比对,不依赖 DB**(外部 agent 文件夹不可控)。详见 `doc/agent_20260724.md` §3.8.3「2.1–2.6 之后:联调迭代修正」。

| 项 | 改动 | 文件 |
|---|---|---|
| 已知 agent 数据源 | `runtimes/skills/install.json`(56 agent)经 `include_str!` 编译进二进制;`default_agents()`(pub(crate))解析;`migrate_v14`(TARGET=14)未改动配置整列替换、已自定义 backfill | `skill_service.rs`、`migration.rs`、`runtimes/skills/install.json` |
| 扫描符号链接 | 只跳过指向库目录的符号链接;其余符号链接收录 `is_symlink=true` 展示禁用 + 🔗徽标;真实目录可选 | `skill_service.scan_for_import`、`models/skill.rs`(ScannedSkill.is_symlink)、`SkillsPage.tsx`(ImportDialog) |
| SKILL.md 解析 | 支持折叠 `>`/字面 `\|` 块标量 + chomp/indent 指示;`is_block_indicator` + `collect_indented_block` | `skill_service.parse_skill_md` |
| 导入来源记录 | 导入成功后 `INSERT OR IGNORE skill_exports (来源agent,'copy','ok')` | `skill_service.import_skills` |
| **检测走 FS** | 已导入=`<lib>/<dir>` exists;已安装 agent=`scan_installs_by_dirname()` 扫各 agent 路径(symlink/copy);主列表/查看 FS reconcile;启动 reconcile 删库副本已没的 DB 行;DB skill_exports 退为状态机+事务清理 | `skill_service.rs`(scan_for_import/import_skills/list_library/get_skill/scan_installs_by_dirname/reconcile_pending)、`commands/skills.rs`(get_skill 加 app)、`SkillsPage.tsx`(用 s.alreadyImported,删 existingDirNames) |
| 外键关闭+事务清理 | `db/mod.rs` 不开 foreign_keys;`delete_skill` 事务删 skill_exports+skills;`reconcile_pending` 清孤儿 exports + 库副本已没的行 | `db/mod.rs`、`skill_service.delete_skill`/`reconcile_pending` |
| 来源记录保护 | delete/uninstall 跳过 `agent_id==source_agent` 的文件删除;删除弹框 copyExports 排除来源 agent;查看/安装弹框来源记录隐藏卸载按钮 | `skill_service.delete_skill`/`uninstall_skill`、`SkillsPage.tsx`(DeleteSkillDialog/ViewDialog/InstallDialog) |
| 列表打开文件夹按钮 | `open_skill_library_dir` 命令打开 `<lib>/<dir>` | `skill_service.open_skill_library`、`commands/skills.rs`、`tauriClient`、`skillService.ts`、`SkillsPage.tsx` |
| 导入弹框展示全部 agent | 全部已配置 agent(含 0 skill)默认折叠;有 skill 的 accent 徽标;路径截断+悬浮;📂 打开按钮 | `SkillsPage.tsx`(ImportDialog) |
| 查看弹框空状态 | 0 导出只显示「尚未导出」(不显示标题) | `SkillsPage.tsx`(ViewDialog) |
| 编译修复 | from_str turbofish、HashSet owned、Manifest* + serde `#[cfg(windows)]`、filter_map `.ok()?`、`library_dir` lib 变量、FilePath.into_path()、open 用 `std::process::Command`、push 闭包 `&str` | `migration.rs`、`skill_service.rs`、`commands/skills.rs`、`db/mod.rs` |
| 手动选择文件夹导入 | 导入弹框左下「手动选择skill(s)」按钮;`scan_folder_for_skills`(2 层 SKILL.md 检测,agent_id=`__manual__`,is_symlink 真实检测,already_imported FS 查库);`import_skills` 分支(path Some→手动无来源记录,None→agent 流程写来源记录);手动分组置顶+自动展开+可导入默认选中;`ImportItem` 加 `path: Option<String>` | `skill_service.rs`、`commands/skills.rs`、`models/skill.rs`、`tauriClient.ts`、`skillService.ts`、`useSkillData.ts`、`SkillsPage.tsx` |
| 切换安装方式 Bug 修复 | `export_to_agents` 原 `INSERT` 触发 `UNIQUE(skill_id, agent_id)` 冲突(切换/重装同一对)→ 旧安装保留;改 `INSERT OR REPLACE` 替换旧行 → remove_link+create 生效 | `skill_service.export_to_agents` |
| 来源记录保护移除 | `uninstall_skill`/`delete_skill` 不再跳过来源记录(agent_id==source_agent);删除弹框 copyExports 不排除来源 agent;查看/安装弹框来源记录显示卸载按钮;来源 agent 可删除+切换方式 | `skill_service.uninstall_skill`/`delete_skill`、`SkillsPage.tsx`(DeleteSkillDialog/ViewDialog/InstallDialog) |
| 已安装检测 DB+FS 校验 | `scan_installs_by_dirname`(纯 FS,过度报告 agent 自带 skill)→ `verified_exports_by_skill`(DB skill_exports + FS 校验 `<agent>/<dir>` 存在);`reconcile_pending` 清 stale exports | `skill_service.rs` |
| 弹框宽度+截断 | 弹框放宽(导入 4xl/查看导出安装 3xl/删除 2xl);agent 名字截断(列表标签 30 字符 JS,其余 truncate+maxWidth 翻倍 320~400);skill 名字/目录名截断(truncate+maxWidth+title) | `SkillsPage.tsx` |

**当前状态:** 6 子阶段代码完成 + 上述迭代修正全部入码;IDE 分析(rust-analyzer)+ `vite build` + `tsc` 均 0 新错;`cargo check`(mac)需用户运行(本会话 cargo 被 hook 拦截),Windows 提权代码需 Windows 构建定稿。

- `pick_directory` / `open_path_in_explorer` Phase 2 需 capability 补 `dialog:allow-open`（保险），见后端 service 说明。
- 不修改 `mcphub-origin/`（核心约束）。
