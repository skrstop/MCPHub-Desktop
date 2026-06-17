# Tauri 自动更新功能配置指南

本指南将帮助您完成 MCPHub Desktop 的自动更新功能配置。

## 快速开始（3 步完成）

### 步骤 1：生成签名密钥

在终端中运行以下命令：

```bash
# 创建密钥目录
mkdir -p ~/.tauri

# 生成密钥对（会提示输入密码，可以直接回车留空）
npx tauri signer generate -w ~/.tauri/mcphub.key
```

执行后会显示类似：
```
Your keypair was generated successfully
Public key: dW50cnVzdGVkIGNvbW1lbnQ6...
Secret key saved to: /Users/your-username/.tauri/mcphub.key
```

### 步骤 2：更新配置文件

运行脚本自动更新配置：

```bash
bash scripts/update-pubkey.sh
```

或者手动更新：

```bash
# 查看公钥
cat ~/.tauri/mcphub.key.pub

# 将输出的公钥复制到 src-tauri/tauri.conf.json 的 plugins.updater.pubkey 字段
```

### 步骤 3：配置 GitHub Secrets

在 GitHub 仓库的 **Settings -> Secrets and variables -> Actions** 中添加：

1. **TAURI_SIGNING_PRIVATE_KEY**
   - 值：运行 `cat ~/.tauri/mcphub.key` 获取私钥内容

2. **TAURI_SIGNING_PRIVATE_KEY_PASSWORD**
   - 值：生成密钥时设置的密码（如果直接回车留空，可以设置任意值或留空）

## 验证配置

### 验证本地配置

```bash
bash scripts/verify-signing.sh
```

预期输出：
```
✅ pubkey is configured in src-tauri/tauri.conf.json
✅ Private key found at ~/.tauri/mcphub.key
✅ Keys match: tauri.conf.json pubkey matches ~/.tauri/mcphub.key.pub
✅ Release workflow found at .github/workflows/release.yml
✅ TAURI_SIGNING_PRIVATE_KEY referenced in release.yml
```

### 测试本地构建

```bash
# 构建应用
npm run build

# 检查签名文件是否生成
ls -la src-tauri/target/release/bundle/*.sig
```

应该看到类似文件：
- `MCPHub Desktop.app.tar.gz.sig` (macOS)
- `mcphub-desktop_1.0.16_amd64.AppImage.tar.gz.sig` (Linux)
- `mcphub-desktop_1.0.16_x64-setup.nsis.zip.sig` (Windows)

## 发布新版本

### 创建 Release

```bash
# 更新版本号（可选）
# 编辑 src-tauri/tauri.conf.json 中的 version 字段

# 创建并推送 tag
git tag v1.0.17
git push origin v1.0.17
```

### 监控构建

1. 访问 GitHub 仓库的 **Actions** 页面
2. 查看 "Release" workflow 的运行状态
3. 等待所有平台构建完成

### 检查 Release

1. 访问 GitHub 仓库的 **Releases** 页面
2. 应该看到一个新的 draft release
3. 检查是否包含以下文件：
   - 安装包（.dmg, .deb, .exe 等）
   - 更新包（.app.tar.gz, .AppImage.tar.gz, .nsis.zip）
   - 签名文件（.sig）
   - `latest.json` 文件

### 发布 Release

1. 在 draft release 页面点击 **Edit**
2. 添加发布说明
3. 点击 **Publish release**

## 测试自动更新

### 测试场景 1：检查更新

1. 安装当前版本的应用
2. 打开应用，点击"关于"对话框
3. 点击"检查更新"按钮
4. 应该显示"正在检查更新..."
5. 如果有新版本，应该显示"安装更新"按钮

### 测试场景 2：安装更新

1. 在"关于"对话框中，如果有新版本
2. 点击"安装更新"按钮
3. 应该显示"正在安装更新..."
4. 下载完成后，应用会自动重启
5. 重启后应该运行新版本

### 测试场景 3：查看更新日志

1. 在"关于"对话框中
2. 点击"查看更新日志"按钮
3. 应该跳转到 GitHub releases 页面
4. 可以查看所有版本的更新日志

## 故障排除

### 问题：updater 无法验证签名

**症状**：检查更新时显示错误，或无法安装更新

**原因**：公钥配置错误或私钥不匹配

**解决**：
1. 确认 `tauri.conf.json` 中的 `pubkey` 与生成的公钥一致
2. 确认 GitHub Secrets 中的私钥与公钥配对
3. 重新生成密钥对并更新配置

### 问题：CI 构建失败

**症状**：GitHub Actions 构建失败

**原因**：GitHub Secrets 配置错误

**解决**：
1. 检查 `TAURI_SIGNING_PRIVATE_KEY` 是否正确设置
2. 如果密钥有密码，检查 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` 是否正确
3. 确保私钥内容完整（包括开头和结尾的注释）

### 问题：用户无法收到更新

**症状**：应用显示"已是最新版本"，但实际上有新版本

**原因**：`latest.json` 文件不存在或格式错误

**解决**：
1. 检查 GitHub Release 是否包含 `latest.json` 文件
2. 确认 `latest.json` 格式正确（包含 `version`、`platforms` 等字段）
3. 检查 `latest.json` 中的下载链接是否正确

### 问题：下载更新失败

**症状**：点击"安装更新"后显示错误

**原因**：网络问题或文件损坏

**解决**：
1. 检查网络连接
2. 尝试重新检查更新
3. 如果问题持续，手动下载安装包更新

## 安全注意事项

- **私钥必须保密**：不要将私钥提交到代码仓库
- **定期轮换密钥**：建议每年轮换一次签名密钥
- **备份密钥**：将私钥安全备份，丢失后无法恢复
- **限制访问**：只有授权人员才能访问 GitHub Secrets

## 相关文档

- [Tauri Updater 官方文档](https://tauri.app/plugin/updater/)
- [Tauri 签名文档](https://tauri.app/distribute/signing/)
- [GitHub Secrets 配置](https://docs.github.com/en/actions/security-guides/encrypted-secrets)
- [GENERATE_KEYS.md](./GENERATE_KEYS.md) - 详细的密钥生成说明
- [SIGNING_SETUP.md](./SIGNING_SETUP.md) - 签名配置详细指南

## 下一步

完成配置后，您可以：

1. **发布新版本**：创建新的 tag 并推送到 GitHub
2. **监控更新**：查看 GitHub Actions 的构建状态
3. **测试更新**：在测试环境中验证自动更新功能
4. **收集反馈**：从用户那里收集更新体验的反馈

如果您遇到任何问题，请查看故障排除部分或联系开发团队。
