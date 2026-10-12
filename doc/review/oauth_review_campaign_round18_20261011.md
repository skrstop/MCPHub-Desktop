# 基线同步复核（第十八轮，R18xx）报告（2026-10-11）

> 战役已于 R17 收敛；本轮为**新功能定点复核**模式首轮——对象 = 本次基线同步引入的新代码（SSRF 守卫 + ui:// 镜像 + 前端合并）。

## 代理分工

| 代理 | 切片 | 结果 |
| --- | --- | --- |
| r18-a1 | SSRF 守卫逐行对照 origin + ui:// 六文件 | **1 High + 1 Medium**（均当日修复）+ 1 Info + 旁路猎杀全排除 |
| r18-a2 | 前端合并对照 origin HEAD | 前端合并**零回归**；1 Medium（origin 继承设计，桌面侧加固） |
| r18-a3 | 套件 r1800 + 回归 | 21/21（+5 有据 SKIP）+ apps_ui 复跑 7/7 + 5 版本公网 IP 绿 |

## 修复 3 项

| # | 级别 | 内容 |
| --- | --- | --- |
| 1 | **[H]** | **SSRF 重定向绕过**：reqwest 默认跟随 10 跳 redirect，公共 spec URL 302 到 `http://169.254.169.254/` 时重定向目标重新走系统 DNS、无校验无 pin，完全绕过预连接检查（origin 用逐跳重验证 + 5 跳帽关闭此洞）→ 桌面采取更严的 **`Policy::none()`**（fail-closed 姿态） |
| 2 | **[M]** | 守卫路径 `builder.build()` 失败静默回退**无 pin 无超时**的默认客户端（fail-open）→ 改硬失败 |
| 3 | **[M·前端]** | `disablePasswordLogin` 开启且无 SSO 时 LoginPage 渲染**空卡片**（无提示无恢复路径，origin 继承设计；桌面客户端开关在 `!isTauri()` 后不可达，暴露面 = hub web UI/手改配置）→ LoginPage 增「账密登录已禁用且未配置 SSO」提示 + 禁用时隐藏默认密码提示；locales +1 键 ×4 |

**Info 记录**：`64:ff9b:1::/48` 本地 NAT64 检查是**死代码**——忠实移植了 origin 自身的 bug（常量位数错），待上游修复后再移植。

## 旁路猎杀（全部排除，a1 亲核）

十进制/十六进制 IP 字面量（`http://2130706433/`）→ url crate WHATWG 解析器归一为 `127.0.0.1` 走字面量路径被拦；即使漏到 DNS 路径 getaddrinfo 也解析回环被拦。IPv4-mapped/废弃 v4 兼容形式全捕获。CIDR 掩码数学无溢出 panic 路径。配置 round-trip 全链路核对（前端 PUT → 深合并 → 守卫逐请求读取，无缓存）。resolve_to_addrs 域名括号处理正确。

## ui:// 镜像验证全过

on_demand 重构语义保持（create-lock 双查/连接锁序/post-connect 重检/InFlightGuard/ptr_eq/Box::pin 自愈——对照旧流程逐项）；run_read 忙碌覆盖与 run_call 镜像；bridge bearer 先于 scope 捷径（R9 F-3 parity）、$smart 早退、root 按 DB 序（已记录偏差）、禁用/内建跳过、逐候选错误仅 debug、全败落回同一 not-found；2026 TTL 包装；pool 锁纪律（无跨 await 占池锁）；mark_conn_loss 仅传输类错误（应用层 not-found 不驱逐健康客户端——R58 类 bug 未复现，有单测）。

## 套件与验证

- **rmcp_round_r1800.py 21/21 + 5 有据 SKIP**：SSRF 各变体预期行为文档化（十进制 IP 中和、file:// scheme 门）、allowedCidrs carve-out 持久化、**5 版本 initialize 回显精确 + 2025-06 会话/2026 stateless 双通道真实公网 IP**、allowedCidrs/disablePasswordLogin round-trip、apps_ui 复跑无回归
- SKIP 根因（已记录）：SSRF 守卫唯一触发点（fetch_spec）与 DB 服务器 pickup 都只在启动/重启后可达——**重启后重跑 r1800/apps_ui 即自动补验**
- tsc 0 错误；契约脚本 OK；locales 语义 diff 0 删 0 改仅 +5 键；cargo check 0 错 0 警；**lib 152 passed**
- 基线还原：config 字节级一致、0 临时服务器行、health 200

## 收敛回答

本轮验证了「收敛后定点复核」模式的有效性：3 个发现全部位于**当日新代码**（SSRF 守卫是今天写的，重定向绕过今天就抓到修掉）——已覆盖的存量切片零发现。新功能 → 定点复核 → 补套件钉死，这就是收敛后的常态循环。
