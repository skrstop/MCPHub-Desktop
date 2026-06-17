# Tauri Updater 签名密钥配置指南

本文档说明如何为 MCPHub Desktop 配置 Tauri 自动更新功能的签名密钥。

## 概述

Tauri 的自动更新功能需要一对签名密钥：
- **私钥**：用于在 GitHub Actions 构建时签名更新包
- **公钥**：嵌入到应用中，用于验证更新包的完整性

## 快速开始

### 1. 生成签名密钥

运行以下命令生成密钥对：

```bash
bash scripts/generate-signing-key.sh
```

或者手动执行：

```bash
npx tauri signer generate -w ~/.tauri/mcphub.key
```

这将生成：
- `~/.tauri/mcphub.key` - 私钥文件
- `~/.tauri/mcphub.key.pub` - 公钥文件

### 2. 配置公钥

查看生成的公钥：

```bash
cat ~/.tauri/mcphub.key.pub
```

将公钥内容复制到 `src-tauri/tauri.conf.json` 的 `plugins.updater.pubkey` 字段：

```json
{
  "plugins": {
    "updater": {
      "endpoints": [
        "https://github.com/skrstop/MCPHub-Desktop/releases/latest/download/latest.json"
      ],
      "pubkey": "你的公钥内容"
    }
  }
}
```

### 3. 配置 GitHub Secrets

在 GitHub 仓库中添加以下 Secrets：

1. **TAURI_SIGNING_PRIVATE_KEY**
   - 内容：私钥文件的完整内容
   - 获取方式：`cat ~/.tauri/mcphub.key`

2. **TAURI_SIGNING_PRIVATE_KEY_PASSWORD**
   - 内容：生成密钥时设置的密码（如果有的话）
   - 如果没有设置密码，可以留空或使用任意值

## 验证配置

### 本地验证

```bash
# 构建应用
npm run build

# 检查签名文件是否生成
ls -la src-tauri/target/release/bundle/*.{sig,zip}
```

### CI 验证

1. 推送一个 `v*` 格式的 tag：
   ```bash
   git tag v1.0.17
   git push origin v1.0.17
   ```

2. 在 GitHub Actions 中查看构建日志，确认：
   - 签名文件已生成
   - `latest.json` 已创建
   - Release 已发布（draft 模式）

## 更新流程

当发布新版本时：

1. GitHub Actions 构建所有平台的安装包
2. 使用私钥签名更新包
3. 生成 `latest.json` 文件（包含版本信息、下载链接和签名）
4. 创建 draft Release 并上传所有文件

用户的应用会：
1. 定期检查 `latest.json` 端点
2. 比较本地版本与远程版本
3. 如果有新版本，下载并验证签名
4. 提示用户安装更新

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

## 安全注意事项

- **私钥必须保密**：不要将私钥提交到代码仓库
- **定期轮换密钥**：建议每年轮换一次签名密钥
- **备份密钥**：将私钥安全备份，丢失后无法恢复

## 相关文档

- [Tauri Updater 官方文档](https://tauri.app/plugin/updater/)
- [Tauri 签名文档](https://tauri.app/distribute/signing/)
- [GitHub Secrets 配置](https://docs.github.com/en/actions/security-guides/encrypted-secrets)
