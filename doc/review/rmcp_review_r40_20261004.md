# R40 复核报告（2026-10-04）

## 概览
R40 轮复核完成 6 项修复，全部验证通过。累计 44 套件 **1280/1280 全绿**（v40 新增 8 项计入）。

## 修复 6 项

### ①[H] sweep_stale_dirs 长度检查 64→32 hex（R28 自身引入的 bug）
`rag/git.rs` R28 修复时把 sweep 长度检查写错（42→64→应有 32）——`repo_hash` 是 **md5 = 32 hex**，写 64 导致 stale 目录永远不清理。修复：长度 32 + `is_ascii_hexdigit` 逐字符校验。
> **教训：修复自身引入 bug。** R28 轮 fix 写入时未对常量做源头核对（md5 长度是常识级事实），直到 R40 才被纠正。这就是"每轮都能跑出问题"的直接证据：**修复落盘后必须有一道 fix self-review**（对常量、对边界、对调用点逐项核对），不是跑绿测试就完事——sweep 是幂等清理，测试根本覆盖不到"长度写错导致永不清理"。

### ②[H] 凭证文件创建权限窗口
`store_credentials` 原先 write→chmod 两步，中间明文密码以进程 umask 权限暴露在磁盘上。修复：unix 走 `OpenOptions::new().mode(0o600)` 直接创建，非 unix 回退 write；chmod 失败向上传播错误（原 `.ok()` 静默吞掉）。
> 教训：**资源创建时的权限就是最终权限**——任何 write-then-chmod 序列都有 TOCTOU 暴露窗口，必须创建时携带 mode。

### ③[M] abort token 注册先于 PICK_LOCK
`clone_to_temp` 原先在获取 PICK_LOCK **之后**才 register_abort，cancel-while-queued 场景下取消请求看不到在排队的任务。修复：注册移到取锁之前。

### ④[M] quoteArg 补反斜杠转义（前端）
`ServerForm.tsx` quoteArg 只转义引号不转义反斜杠——含 `\` 的参数经 shell 解包后语义漂移。修复：首步 `replace(/\\/g,'\\\\')`。

### ⑤[M] splitArgs 反斜杠语义分裂（前端）
引号内 `\` 作转义前缀、引号外作字面量（Windows 路径 `C:\path` 存活）。node 脚本验证 roundtrip 含 `a\"b` / `p\q` / `C:\path` 全部 ok。
> 教训：**转义/反转义对必须成对设计且用混合用例做 roundtrip 验证**——单侧修、单侧测必漏。

### ⑥[L] idleTimeoutMs clamp + BearerKeyRow catch
`SettingsPage.tsx`：0/NaN 直接传 Rust 导致后端解析异常 → 非有限或 <10000 clamp 到 300000；BearerKeyRow.handleSave 补 catch（未处理 promise rejection）。

## 验证
- cargo check 0 错 0 警；cargo test --lib 102 passed；cargo build ✓
- tsc 0 错误；npm build ✓
- v40 套件 8/8；全量回归 44 套件
- **全量回归首轮 8 个套件失败（v19/v20/v21/v24/v26/round50f/round50g/round_r51），全部为公网 IP 真实调用类** → 单独复跑逐一全绿（87/87、26/26、12/12、36/36、12/12、12/12、35/35、14/14）。定性：**外部公网 IP 服务（ip.3322.net）瞬时抖动 + 串行压测限速**，非代码回归。判据：失败用例全部命中同一外部依赖，且复跑零改动即过。

## 累计账目（R40 止）
44 套件 1280/1280 全绿；cargo test --lib 102 passed；~230 项修复；单轮发现数 25 → 3-6 收敛。
