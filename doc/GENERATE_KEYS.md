# 生成 Tauri 更新签名密钥

由于密钥生成需要交互式输入密码，请在终端中手动执行以下步骤：

## 步骤 1：生成密钥对

在终端中运行：

```bash
# 创建密钥目录
mkdir -p ~/.tauri

# 生成密钥对（会提示输入密码，可以直接回车留空）
npx tauri signer generate -w ~/.tauri/mcphub.key
```

执行后会显示：
- 私钥位置：`~/.tauri/mcphub.key`
- 公钥位置：`~/.tauri/mcphub.key.pub`
- 公钥内容（base64 编码的字符串）

## 步骤 2：查看公钥

```bash
cat ~/.tauri/mcphub.key.pub
```

输出类似：
```
dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHVwZGF0ZSBzaWduaW5nIGtleQpSV1JrTTNad0ZJek1nSGFXVjVwWTBKaElBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE9PQ==
```

## 步骤 3：更新配置文件

将公钥内容复制到 `src-tauri/tauri.conf.json` 的 `plugins.updater.pubkey` 字段：

```json
{
  "plugins": {
    "updater": {
      "endpoints": [
        "https://github.com/skrstop/MCPHub-Desktop/releases/latest/download/latest.json"
      ],
      "pubkey": "这里粘贴你的公钥内容"
    }
  }
}
```

## 步骤 4：查看私钥（用于 GitHub Secrets）

```bash
cat ~/.tauri/mcphub.key
```

## 步骤 5：配置 GitHub Secrets

在 GitHub 仓库的 Settings -> Secrets and variables -> Actions 中添加：

1. **TAURI_SIGNING_PRIVATE_KEY**
   - 值：`~/.tauri/mcphub.key` 文件的完整内容

2. **TAURI_SIGNING_PRIVATE_KEY_PASSWORD**
   - 值：生成密钥时设置的密码（如果直接回车留空，可以设置任意值或留空）

## 验证配置

运行验证脚本：

```bash
bash scripts/verify-signing.sh
```

## 测试构建

```bash
# 本地构建测试
npm run build

# 检查签名文件是否生成
ls -la src-tauri/target/release/bundle/*.sig
```

## 注意事项

- 私钥必须保密，不要提交到代码仓库
- 公钥可以公开，会嵌入到应用中
- 如果丢失私钥，将无法发布更新，用户需要重新下载安装
- 建议定期备份私钥到安全位置
