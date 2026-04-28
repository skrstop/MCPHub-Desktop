# Tauri Updater — 发布清单 `latest.json` 使用说明

[Tauri Updater 插件](https://v2.tauri.app/plugin/updater/) 通过拉取一份 `latest.json` 清单来判断是否有新版本。本目录的 [latest.json](latest.json) 是模板，发版时按下面流程替换字段后上传到 GitHub Release。

## 1. 生成签名密钥（仅首次）

```bash
npx @tauri-apps/cli signer generate -w ~/.tauri/mcphub.key
```

- 把 **公钥** 填到 [src-tauri/tauri.conf.json](../tauri.conf.json) 的 `plugins.updater.pubkey`
- **私钥** 与 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` 仅保存在 CI Secrets 中，用于签名构建产物

## 2. 构建并签名

```bash
export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/mcphub.key)"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="<your-password>"
npm run tauri build
```

构建完成后会在 `src-tauri/target/release/bundle/<platform>/` 下生成：

| 平台 | 升级包 | 签名文件 |
|---|---|---|
| macOS | `MCPHub Desktop_<ver>_x64.app.tar.gz` / `_aarch64.app.tar.gz` | 同名 `.sig` |
| Linux | `mcphub-desktop_<ver>_amd64.AppImage.tar.gz` | 同名 `.sig` |
| Windows | `MCPHub Desktop_<ver>_x64-setup.nsis.zip` | 同名 `.sig` |

## 3. 填写 `latest.json`

把每个 `.sig` 文件的 **整段 base64 内容** 填到对应平台的 `signature` 字段。`url` 指向 GitHub Release 资产的下载链接。

示例（macOS aarch64 单平台）：

```json
{
  "version": "0.12.13",
  "notes": "Release notes here",
  "pub_date": "2026-04-23T00:00:00Z",
  "platforms": {
    "darwin-aarch64": {
      "signature": "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVRcm...",
      "url": "https://github.com/samanhappy/mcphub-desktop/releases/download/v0.12.13/MCPHub.Desktop_0.12.13_aarch64.app.tar.gz"
    }
  }
}
```

## 4. 上传到 GitHub Release

把以下文件作为 Release 资产上传到 `v0.12.13` Release：

- `latest.json`
- 所有平台的升级包（`.tar.gz` / `.nsis.zip`）

客户端配置（[tauri.conf.json](../tauri.conf.json)）的 endpoint 已指向：
`https://github.com/samanhappy/mcphub-desktop/releases/latest/download/latest.json`

GitHub 会自动把 `latest` 别名解析到最新 Release，因此后续每次发版只需上传新的 `latest.json` 与升级包即可，无需修改客户端。

## 5. （推荐）使用 GitHub Action 自动化

`tauri-apps/tauri-action` 支持自动构建、签名、生成 `latest.json` 并上传到 Release。最小示例：

```yaml
- uses: tauri-apps/tauri-action@v0
  env:
    GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
    TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}
  with:
    tagName: v__VERSION__
    releaseName: 'MCPHub Desktop v__VERSION__'
    includeUpdaterJson: true
```

`includeUpdaterJson: true` 会自动生成并上传 `latest.json`，无需手动维护。
