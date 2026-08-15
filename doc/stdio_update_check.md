# 服务器页面新增「stdio 更新检测」按钮

## Context

当前 stdio 服务器（npx/uvx）的更新检测**只在每次连接成功后被动触发一次**（`pool.rs` → `progress::spawn_update_check`），用户无法主动触发检测。本次需求两处主动入口：

1. **页面级**：在服务器页面「+市场」按钮**左边**增加「stdio 更新检测」按钮，**前提是当前列表中至少有一个 npx/uvx stdio 服务器**（否则不显示）。点击后对所有 npx/uvx stdio 服务器**批量复用现有检测逻辑**。
2. **单卡级**：每个 npx/uvx stdio 服务器的「更多」菜单中增加「检查更新」项，可**单个**触发检测。

两者结果都通过现有 `server://update-available` 事件回到前端，badge 的显示/清除与现有机制完全一致。命名采用「stdio 更新检测」（页面按钮文案）/「检查更新」（卡片菜单项）。

触发时机不变（连接成功后的被动检测保留），本次新增的是**额外的主动触发入口**。

此外配套修复：**允许对未启动（disabled）的服务器安装更新**——原先 `reinstall_server` 仅在 `cfg.enabled` 时才重连拉取新版本，导致禁用服务器无法更新包。现已移除该限制。

## 实现方案

### 1. 后端：新增检测命令（批量 + 单个）

**文件**：`src-tauri/src/commands/servers.rs`

新增两个命令，**共用一个内部异步函数**完成实际检测，避免重复逻辑：

- `check_stdio_updates`（批量）：读 `server_service::list_all().await`（`server_service.rs:41`）→ 过滤 `server_type == Stdio` 且 `is_package_manager(&cfg.command)`（`progress.rs:145`）→ 对每个命中服务器触发检测。
- `check_server_update`（单个，参数 `name`）：`server_service::get_by_name(&name).await`（`server_service.rs:51`）→ 校验是 npx/uvx stdio → 触发检测。

两者都调用同一个私有 async helper：
```rust
async fn run_update_check(cfg: &ServerConfig) {
    let running_version = pool::get_entry_info(&cfg.name).await
        .map(|(s, _)| s.server_version).unwrap_or(None);
    progress::spawn_update_check(
        cfg.name.clone(),
        cfg.command.clone().unwrap_or_default(),
        cfg.args.clone().unwrap_or_default(),
        running_version,
    );
}
```
- `running_version` 取该服务器在 pool 中的 `server_version`（`pool::get_entry_info`，`pool.rs:540`），可能为 `None`，与连接时检测一致——自报版本仅用于日志，不影响判定。
- 两个命令**立即返回**（`spawn_update_check` 本身是 fire-and-forget 的 `tauri::async_runtime::spawn`）。批量返回受检数量；单个返回 `{ checked: true }`（便于前端 toast）。

> 关键：**完全复用** `spawn_update_check` 内部逻辑（取包名→拉 registry→对比 recorded 版本→emit `server://update-available`）。不复制比较/通知规则，所以 `just_reinstalled`、首次记录、`is_newer` 等行为与连接时检测**完全一致**。这对 on-demand（睡眠中）的 stdio 服务器同样有效：`spawn_update_check` 不依赖进程在运行，只读 config 的 `packageVersions` 和拉 registry。

**注册命令**：`src-tauri/src/lib.rs` 的 `invoke_handler!` 列表，在 `reinstall_server` 附近（`lib.rs:366`）加 `commands::servers::check_stdio_updates,` 与 `commands::servers::check_server_update,`。

### 1b. 后端：允许禁用服务器重装更新

**文件**：`src-tauri/src/commands/servers.rs`（`reinstall_server`）

原先：
```rust
if cfg.enabled {
    progress::mark_reinstalled(&cfg.name);
    tauri::async_runtime::spawn(async move { pool::connect_server(&cfg_clone).await; });
}
```
改为**无条件**重连拉取新版本：
```rust
progress::mark_reinstalled(&cfg.name);
tauri::async_runtime::spawn(async move { pool::connect_server(&cfg_clone).await; });
```
- `connect_server` 本身不检查 `enabled`（该标志只控制启动时自动连接，不影响显式重连），所以禁用服务器也能重连拉取新包。
- DB 中的 `enabled` 标志保持不变（重装流程从不改写它），重连成功后该服务器在 pool 中转为 connected；若用户希望恢复禁用态，可在卡片重新禁用。`mark_reinstalled` 保证连接后更新检查把新版本记为已安装并清 badge。

### 2. 前端：REST→Tauri 路由映射

**文件**：`frontend/src/utils/tauriClient.ts`

在 `mapRestToCommand` 中（`reinstall` 路由附近，`tauriClient.ts:96`）增加：
```ts
if (p === 'servers/check-stdio-updates' && m === 'POST')
  return { command: 'check_stdio_updates', args: {} };
if (segs[0] === 'servers' && segs.length === 3 && segs[2] === 'check-update' && m === 'POST')
  return { command: 'check_server_update', args: { name: decodeURIComponent(segs[1]) } };
```
在 `transformTauriResponse`（`tauriClient.ts:725`）中无需特殊处理——两者返回简单对象，走末尾的 `{ success: true, data: result }` 通用分支即可。

### 3a. 前端：页面按钮（批量）

**文件**：`frontend/src/pages/ServersPage.tsx`

在 header 右侧按钮组中，把新按钮放在「+市场」按钮**左边**（按钮组最左）：
```tsx
{hasStdioServer && (
  <button className="hub-btn" onClick={handleCheckStdioUpdates} disabled={isCheckingUpdates}>
    <RefreshCw size={13} className={isCheckingUpdates ? 'animate-spin' : ''} />
    {t('server.checkStdioUpdates')}
  </button>
)}
```
- `hasStdioServer`：`useMemo`，`allServers.some(s => s.config?.command === 'npx' || s.config?.command === 'uvx')`（与 `ServerCard` 的 `supportsReinstall` 判定一致，与后端 `is_package_manager` 口径一致）。
- `isCheckingUpdates` state；`handleCheckStdioUpdates` 调 `apiPost('/servers/check-stdio-updates', {})`，读返回的 `checked` 数 toast「已开始检测 N 个 stdio 服务器的更新」，并保持 spinner ~400ms。
- 命令立即返回，`checked` 为 0 时 toast 显示「检测 0 个」——即「列表中暂无 npx/uvx stdio 服务器可检测」。

### 3b. 前端：单个服务器「更多」菜单项

**文件**：`frontend/src/components/ServerCard.tsx`

在「更多」菜单中新增「检查更新」项，条件与 reinstall 相同（`supportsReinstall`，npx/uvx），放在「重载」之后：
```tsx
{supportsReinstall && (
  <button onClick={handleCheckUpdate} disabled={isCheckingUpdate} ...>
    <RefreshCw size={13} className={isCheckingUpdate ? 'animate-spin' : ''} />
    {t('server.checkForUpdates') || '检查更新'}
  </button>
)}
```
- 新增 state `isCheckingUpdate`；`handleCheckUpdate` 调 `apiPost('/servers/${encodeURIComponent(server.name)}/check-update', {})`，toast「正在检查 {{name}} 的更新」，spinner ~400ms。
- 检测结果经 `server://update-available` 事件回流到本卡 `updateInfo`/`hasUpdate`，自动驱动红点与「更新到 X」菜单项——无需额外状态。

**配套调整（允许禁用服务器更新）**：「更新到 X」与「重新安装」两个菜单项原先 `disabled={isReinstalling || isToggling || !enabled}`，现移除 `!enabled` 条件（保留 `isReinstalling || isToggling`），使禁用服务器也能点更新。`handleReinstall` 的 guard 同样去掉 `!enabled`（其前置 `canManage` 仍保留）。

### 4. 文案（i18n）

四个语言文件的 `server` 块内新增 key（zh/en 在 `updateTo` 附近；fr/tr 在 `reinstall` 块附近）：

| key | zh | en | fr | tr |
|---|---|---|---|---|
| `server.checkStdioUpdates`（页面按钮） | stdio 更新检测 | stdio update check | Vérif. mises à jour stdio | stdio güncelleme kontrolü |
| `server.checkForUpdates`（卡片菜单项） | 检查更新 | Check for updates | Vérifier les mises à jour | Güncellemeleri kontrol et |
| `server.checkStdioUpdatesStarted`（toast, count） | 已开始检测 {{count}} 个 stdio 服务器的更新 | Checking {{count}} stdio server(s) for updates | Vérification des mises à jour pour {{count}} serveur(s) stdio | {{count}} stdio sunucusu için güncellemeler kontrol ediliyor |
| `server.checkStdioUpdatesError` | 更新检测启动失败 | Failed to start update check | Échec du lancement de la vérification | Güncelleme kontrolü başlatılamadı |
| `server.checkServerUpdateStarted`（toast, name） | 正在检查 {{name}} 的更新 | Checking {{name}} for updates | Vérification des mises à jour pour {{name}} | {{name}} için güncellemeler kontrol ediliyor |
| `server.checkForUpdatesError` | 检查更新失败 | Failed to check for updates | Échec de la vérification des mises à jour | Güncellemeler kontrol edilemedi |

### 5. 复用的现有函数（不新写）

- `progress::spawn_update_check`（`progress.rs:331`）——检测主体，复用全部通知规则。
- `progress::is_package_manager`（`progress.rs:145`）——后端过滤 npx/uvx。
- `progress::mark_reinstalled`（`progress.rs:130`）——重装后清 badge。
- `server_service::list_all`（`server_service.rs:41`）——取全量配置。
- `pool::get_entry_info`（`pool.rs:540`）——取运行中 `server_version`（仅用于日志）。
- 前端 `ServerInstallProgressContext`（已监听 `server://update-available`，`ServerInstallProgressContext.tsx:82`）——结果自动流入 `updates` 状态，`ServerCard` 的 badge 现成生效。
- `ServerCard` 的 `hasUpdate` badge（`ServerCard.tsx:213`）与「更新到 X」菜单项——检测后自动出现。

## 关键文件清单

修改：
- `src-tauri/src/commands/servers.rs` — 新增 `check_stdio_updates`、`check_server_update` + 共用 `run_update_check`；`reinstall_server` 去掉 `cfg.enabled` 限制
- `src-tauri/src/lib.rs` — 注册两个命令
- `frontend/src/utils/tauriClient.ts` — 两条路由映射
- `frontend/src/pages/ServersPage.tsx` — 页面按钮 + handler + `hasStdioServer`
- `frontend/src/components/ServerCard.tsx` — 卡片菜单项 + handler + state；「更新到 X」/「重新安装」去掉 `!enabled` 限制
- `locales/zh.json` / `en.json` / `fr.json` / `tr.json` — 文案（6 个 key）

不修改：`progress.rs`（逻辑零改动）、`ServerInstallProgressContext.tsx`、`ServerContext.tsx`。

## 验证

1. **构建**：`cargo check`（带本地代理 `127.0.0.1:7890`，cargo 经 asdf shim，见 memory）；前端 `tsc --noEmit`（改动文件错误数与 baseline 一致，未引入新类型错误）。
2. **页面按钮（无 stdio）**：服务器页面不显示该按钮。
3. **页面按钮（有 npx/uvx）**：按钮出现；点击后 spinner 短暂旋转，toast 显示「已开始检测 N 个…」（N 为 npx/uvx 服务器数，可能为 0）；几秒后对应卡片「…」菜单出现红点 +「更新到 X」（若 registry 有新版本）；已是最新则红点不出现/清除旧 badge。
4. **卡片单检**：某 npx/uvx 服务器「更多」→「检查更新」，spinner 旋转 + toast；结果回流到该卡 badge。
5. **禁用服务器更新**：禁用某 npx/uvx 服务器，「更新到 X」/「重新安装」按钮可点；点击后清缓存重连拉新版本，连接后 badge 清除。
6. **on-demand 睡眠中的 stdio 服务器**：两种检测入口都能检测（不依赖进程运行）。
7. **回归**：连接成功后的被动检测仍正常工作（未改动 `pool.rs`/`progress.rs`）。
