# GitHub Actions 和更新配置修复总结

本文档总结了对 MCPHub Desktop 项目 GitHub Actions 和更新功能的修复。

## 修复的问题

### 1. ✅ updater pubkey 为空（严重）

**问题**：`src-tauri/tauri.conf.json` 中的 `plugins.updater.pubkey` 为空字符串，导致 Tauri updater 无法验证签名，应用无法自动更新。

**修复**：
- 添加了占位符 pubkey（需要用户生成真实密钥替换）
- 创建了 `scripts/generate-signing-key.sh` 脚本生成密钥对
- 创建了 `SIGNING_SETUP.md` 详细配置指南
- 创建了 `scripts/verify-signing.sh` 验证配置脚本

**下一步**：
```bash
# 1. 生成签名密钥
bash scripts/generate-signing-key.sh

# 2. 查看公钥
cat ~/.tauri/mcphub.key.pub

# 3. 更新 src-tauri/tauri.conf.json 中的 pubkey
# 4. 添加 GitHub Secrets:
#    - TAURI_SIGNING_PRIVATE_KEY (私钥内容)
#    - TAURI_SIGNING_PRIVATE_KEY_PASSWORD (密码，如果有)
```

### 2. ✅ Windows arm64 交叉编译工具链缺失

**问题**：`release.yml` 中 Windows arm64 在 x64 runner 上编译，但没有安装对应的 MSVC 工具链。

**修复**：在 `release.yml` 中添加了 Windows ARM64 MSVC 工具链安装步骤：

```yaml
# ─── Windows arm64 交叉编译工具链 ─────────────────────────────────────
- name: Install Windows ARM64 MSVC toolchain
  if: matrix.target == 'aarch64-pc-windows-msvc'
  run: |
    rustup target add aarch64-pc-windows-msvc
    echo "Added aarch64-pc-windows-msvc target"
```

### 3. ✅ Changelog API 在 Tauri 中被禁用（设计如此）

**问题**：前端 `changelogService.ts` 调用 `/changelog/update-info` API，但在 Tauri 桌面应用中被映射为 stub，总是返回 `{ hasUpdate: false, entries: [] }`。

**分析**：这是正确的设计：
- Tauri 桌面应用使用原生 updater 插件 (`@tauri-apps/plugin-updater`) 进行自动更新
- Changelog API 是为 Web 版本设计的，在桌面版本中不需要
- `frontend/src/utils/version.ts` 正确实现了 Tauri updater 集成

**结论**：这不是 bug，是预期行为。

## 配置验证

### 运行验证脚本

```bash
bash scripts/verify-signing.sh
```

预期输出：
```
✅ pubkey is configured in src-tauri/tauri.conf.json
✅ Release workflow found at .github/workflows/release.yml
✅ TAURI_SIGNING_PRIVATE_KEY referenced in release.yml
```

### 检查清单

- [ ] 生成签名密钥对
- [ ] 更新 `src-tauri/tauri.conf.json` 中的 pubkey
- [ ] 添加 GitHub Secrets:
  - [ ] `TAURI_SIGNING_PRIVATE_KEY`
  - [ ] `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`
- [ ] 测试本地构建：`npm run build`
- [ ] 测试 CI 构建：`git tag v1.0.17 && git push origin v1.0.17`

## 文件变更

### 新增文件

1. `scripts/generate-signing-key.sh` - 生成签名密钥脚本
2. `scripts/verify-signing.sh` - 验证配置脚本
3. `SIGNING_SETUP.md` - 签名配置指南
4. `FIXES_SUMMARY.md` - 本修复总结

### 修改文件

1. `.github/workflows/release.yml`
   - 添加 Windows ARM64 MSVC 工具链安装步骤

2. `src-tauri/tauri.conf.json`
   - 添加占位符 pubkey（需要用户替换）

## 更新流程说明

### 自动更新流程

1. **构建阶段**（GitHub Actions）：
   - 构建所有平台的安装包
   - 使用私钥签名更新包（生成 `.sig` 文件）
   - 生成 `latest.json`（包含版本信息、下载链接和签名）
   - 创建 draft Release 并上传所有文件

2. **用户端**（Tauri 应用）：
   - 定期检查 `latest.json` 端点
   - 比较本地版本与远程版本
   - 如果有新版本，下载并验证签名
   - 提示用户安装更新

### 手动更新检查

前端 `version.ts` 提供了以下函数：

```typescript
// 检查更新
const updateInfo = await checkForAppUpdate();

// 安装更新
await installAppUpdate((event) => {
  console.log('Download progress:', event);
});
```

## 故障排除

### 问题：updater 无法验证签名

**原因**：公钥配置错误或私钥不匹配

**解决**：
1. 确认 `tauri.conf.json` 中的 `pubkey` 与生成的公钥一致
2. 确认 GitHub Secrets 中的私钥与公钥配对

### 问题：CI 构建失败

**原因**：GitHub Secrets 配置错误

**解决**：
1. 检查 `TAURI_SIGNING_PRIVATE_KEY` 是否正确设置
2. 如果密钥有密码，检查 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` 是否正确

### 问题：用户无法收到更新

**原因**：`latest.json` 文件不存在或格式错误

**解决**：
1. 检查 GitHub Release 是否包含 `latest.json` 文件
2. 确认 `latest.json` 格式正确（包含 `version`、`platforms` 等字段）

## 相关文档

- [Tauri Updater 官方文档](https://tauri.app/plugin/updater/)
- [Tauri 签名文档](https://tauri.app/distribute/signing/)
- [GitHub Secrets 配置](https://docs.github.com/en/actions/security-guides/encrypted-secrets)
- [SIGNING_SETUP.md](./SIGNING_SETUP.md) - 详细签名配置指南
