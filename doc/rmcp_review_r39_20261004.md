# rmcp 复核第三十九轮（R39，2026-10-04）：session_rebuild 重连缺重检 + 空密码改密 + 最后管理员无守卫

> 复核范围：http_server.rs 头部 2250 行（中间件/bearer/leniency/loopback/activity 辅助——R34 只扫了尾部）；
> user_service / auth / mcp_manager / bearer_key_service / db 重读。
> 方法：侵蚀审计全在 → 2 个并行逐行代理 → 亲核验证 3 项 → 修复 → v39 套件 → 全量回归。

## 修复（3 项：Medium×2，Low×1）

### U1 [M] session_rebuild 重连缺 post-connect 重检——第三个同族位点
toggle enable/disable 两路径都有「连接完成后重读 DB 仍启用？」fail-closed 重检（R31/R32
修的），**rebuild 的 reconnect spawn 是同族第三条路径却没有**：tick 读配置 → 用户禁用 →
spawn 重连 → 已禁用服务器永久在线（scope filter 不查 DB，直到重启无人回收）。**修复**：
connect 后镜像 toggle 的重检（行缺/Err/disabled → disconnect，fail-closed）。

### U2 [M] update_by_username 接受空密码
create/update_password 都拒空，update_by_username 直接 `hash("")`——账号变成空密码可
登录（经 admin API 或 settings_import 喂入）。**修复**：同一 trim 空守卫。

### U3 [L] 删除最后管理员无守卫
无条件 DELETE——最后一个 admin 删掉自己后用户管理不可达，唯一恢复是重启重播种**已知默
认凭据** admin/admin（可能暴露 HTTP 的机器上）。**修复**：目标为 admin 时先数剩余
admin，≤1 拒绝。

## 清洁（重要负面结论）
- **http_server.rs 头部 2250 行零发现**：中间件层序（bearer 最外→leniency→cleanup→handler）
  全组合追踪正确；leniency 六种门组合逐一落对路径；strict 模式零 body 变异；
  `&auth[7..]` 切片被双守卫保证不可达；loopback stale-stop 守卫全在——**协议核心面正式
  扫清**
- auth（HS256-only/随机 secret/guest fail-closed）、bearer_key_service（常数时间比较单
  调性）、db/mod（WAL/busy_timeout/初始化时序）——零缺陷

## 新套件 rmcp_supplement_v39.py（8 项）
- 4 legacy 回显 + 公网 IP ×2（2024-11-05 + 2026 stateless，IPv4 Body 断言）
- 2026 discover（ttlMs 3600000）+ /health

## 验证
- `cargo check --lib` 0 错 0 警；`cargo test --lib` 102 passed；cargo build ✓
- 43 套件 **1270/1270 全绿**（v39 8/8 新计入；重建二进制热跑）

## 教训
- **同族修复点必须穷举到「零」而不是到「修完报告的那些」**：U1 是 post-connect 重检家
  族的第三条路径（toggle enable/disable 修了、rebuild 漏了）——修一个状态机问题时把全
  部 spawn/connect 位点列出来逐一问「这条也要重检吗」。
- 校验守卫按「函数对」审计而非单函数：create/update_password 有守卫而 update_by_username
  没有 = 三条写密码路径只封了两条（与门控数据面审计同方法）。
