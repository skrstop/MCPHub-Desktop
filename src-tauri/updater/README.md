# Tauri Updater — 自动更新配置指南

[Tauri Updater 插件](https://v2.tauri.app/plugin/updater/) 通过拉取一份 `latest.json` 清单来判断是否有新版本。本目录包含签名密钥和配置文件，用于实现应用的自动更新功能。

## 📁 目录结构

```
src-tauri/updater/
├── README.md              # 本文档
├── mcphub.key             # 签名私钥（已提交到仓库）
├── mcphub.key.pub         # 签名公钥（已提交到仓库）
└── latest.json            # 更新清单模板
```

## 🔑 签名密钥配置

### 密钥说明

- **私钥** (`mcphub.key`)：用于签名更新包，已提交到仓库（开源项目）
- **公钥** (`mcphub.key.pub`)：用于验证更新包，已配置到 `tauri.conf.json`
- **证书密码**：无（空密码，CI 直接使用）

### 配置状态

- ✅ 公钥已配置到 `src-tauri/tauri.conf.json`
- ✅ 私钥已存在于 `src-tauri/updater/mcphub.key`
- ✅ GitHub Actions 已配置使用仓库中的私钥

## 🚀 快速开始

### 1. 本地构建

```bash
# 安装依赖
npm install
cd frontend && npm install && cd ..

# 构建应用（会自动签名）
npm run build

# 检查签名文件
ls -la src-tauri/target/release/bundle/*.sig
```

### 2. 创建 Release

```bash
# 创建并推送 tag
git tag v1.0.17
git push origin v1.0.17
```

### 3. 测试自动更新

1. 安装当前版本的应用
2. 打开"关于"对话框
3. 点击"检查更新"
4. 如果有新版本，点击"安装更新"
5. 应用会自动下载、安装并重启

## 🔧 GitHub Actions 配置

### 签名流程

1. **读取签名私钥**
   - 从仓库中的 `src-tauri/updater/mcphub.key` 文件读取（无需 GitHub Secrets）

2. **构建应用**
   - 使用私钥签名更新包（无密码）
   - 生成 `.sig` 签名文件

3. **生成 latest.json**
   - 解析 `.sig` 文件生成更新元数据
   - 包含版本信息、下载链接和签名

4. **创建 Release**
   - 创建 draft release
   - 上传所有平台的安装包和签名文件

### 环境变量

| 变量名 | 值 | 说明 |
|--------|-----|------|
| `TAURI_SIGNING_PRIVATE_KEY` | 从仓库文件 `mcphub.key` 读取 | 签名私钥 |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | `""`（空字符串） | 私钥无密码 |

## 📋 构建产物

构建完成后会在 `src-tauri/target/release/bundle/<platform>/` 下生成：

| 平台 | 安装包 | 更新包 | 签名文件 |
|------|--------|--------|----------|
| macOS | `.dmg` | `.app.tar.gz` | `.app.tar.gz.sig` |
| Linux | `.deb`, `.rpm` | — | — |
| Windows | `.exe`, `.msi` | `.nsis.zip` | `.nsis.zip.sig` |

## 📝 `latest.json` 格式

把每个 `.sig` 文件的 **整段 base64 内容** 填到对应平台的 `signature` 字段。`url` 指向 GitHub Release 资产的下载链接。

```json
{
  "version": "1.0.18001",
  "notes": "MCPHub Desktop 1.0.18001\n\nSee release page for full changelog.",
  "pub_date": "2026-06-17T12:00:00Z",
  "platforms": {
    "darwin-aarch64": {
      "signature": "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVRcm...",
      "url": "https://github.com/skrstop/MCPHub-Desktop/releases/download/v1.0.18001/MCPHub.Desktop_1.0.18001_aarch64.app.tar.gz"
    },
    "darwin-x86_64": {
      "signature": "...",
      "url": "..."
    },
    "linux-x86_64": {
      "signature": "...",
      "url": "..."
    },
    "windows-x86_64": {
      "signature": "...",
      "url": "..."
    }
  }
}
```

### 平台标识

- `darwin-aarch64` — macOS ARM64 (Apple Silicon)
- `darwin-x86_64` — macOS x64 (Intel)
- `linux-aarch64` — Linux ARM64
- `linux-x86_64` — Linux x64
- `windows-aarch64` — Windows ARM64
- `windows-x86_64` — Windows x64

## 🔄 自动更新流程

### 检查更新

1. 用户打开"关于"对话框
2. 点击"检查更新"按钮
3. 应用使用 Tauri updater 插件检查 `latest.json` 端点
4. 如果有新版本，显示"安装更新"按钮

### 安装更新

1. 用户点击"安装更新"按钮
2. 应用下载新版本（使用 `.sig` 文件验证完整性）
3. 下载完成后自动安装并重启应用

### 查看更新日志

- 点击"查看更新日志"按钮跳转到 GitHub releases 页面
- 用户可以查看所有版本的更新日志

## 🛠️ 故障排除

### 问题：updater 无法验证签名

**症状**：检查更新时显示错误，或无法安装更新

**原因**：公钥配置错误或私钥不匹配

**解决**：
1. 确认 `tauri.conf.json` 中的 `pubkey` 与 `mcphub.key.pub` 一致
2. 确认使用正确的私钥进行签名
3. 重新生成密钥对并更新配置

### 问题：CI 构建失败

**症状**：GitHub Actions 构建失败

**原因**：签名配置错误

**解决**：
1. 检查 `src-tauri/updater/mcphub.key` 文件是否存在
2. 确认私钥内容完整
3. 检查 GitHub Actions 日志获取详细错误信息

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

## 🔐 安全注意事项

1. **私钥公开**：由于是开源项目，私钥已提交到代码仓库，这是预期行为
2. **签名目的**：签名主要用于验证**完整性**，而不是**安全性**
3. **官方版本**：只有通过 GitHub Actions 构建的版本才会被推送到更新端点
4. **本地构建**：本地构建的版本不会自动更新，除非手动配置更新端点
5. **密钥轮换**：如需轮换密钥，需要重新生成并更新 `src-tauri/updater/` 目录中的文件以及 `tauri.conf.json` 中的 `pubkey`

### 密钥历史

| 时间 | Key ID | 说明 |
|------|--------|------|
| 2026-06 初始 | `65BF7C572D68354A` | 初始密钥（密码加密，已废弃） |
| 2026-06-18 | `DC1E8C89B9CCAD9C` | 新密钥（无密码，供 CI 直接使用） |

## 📚 相关文档

- [Tauri Updater 官方文档](https://tauri.app/plugin/updater/)
- [Tauri 签名文档](https://tauri.app/distribute/signing/)
- [GitHub Actions 配置](../../.github/workflows/release.yml)

## 🎯 下一步

1. 测试本地构建
2. 创建第一个 release
3. 测试自动更新功能
4. 收集用户反馈

---

💡 **提示**：运行 `bash scripts/verify-signing.sh` 验证配置状态。
