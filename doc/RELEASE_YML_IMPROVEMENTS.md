# 🚀 release.yml 配置改进总结

## ✅ 已完成的改进

### 1. 版本号配置改进

**改进前**：从 GitHub tag 中提取版本号
```python
tag = os.environ['RELEASE_TAG']
version = tag.lstrip('v')
```

**改进后**：从 `tauri.conf.json` 中读取版本号
```python
# 从 tauri.conf.json 读取版本号
with open('src-tauri/tauri.conf.json', 'r') as f:
    tauri_conf = json.load(f)
version = tauri_conf.get('version', '')
```

**优势**：
- ✅ 版本号来自项目配置文件，更加可靠
- ✅ 自动验证 tag 和版本号是否一致
- ✅ 如果配置文件读取失败，回退到 tag 名称

### 2. 签名配置改进

**改进前**：依赖 GitHub Secrets
```yaml
TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}
```

**改进后**：优先使用仓库中的私钥文件
```yaml
# 读取签名私钥
- name: Read signing key
  id: signing-key
  run: |
    if [ -f "src-tauri/updater/mcphub.key" ]; then
      echo "key=$(cat src-tauri/updater/mcphub.key)" >> $GITHUB_OUTPUT
    else
      echo "key=${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}" >> $GITHUB_OUTPUT
    fi

# 使用读取的私钥
TAURI_SIGNING_PRIVATE_KEY: ${{ steps.signing-key.outputs.key }}
```

**优势**：
- ✅ 开源项目无需配置 GitHub Secrets
- ✅ 私钥文件已提交到仓库
- ✅ 如果仓库中没有私钥，回退到 GitHub Secrets

### 3. 文档配置改进

**新增文档**：
- `doc/VERSION_CONFIG.md` - 版本号配置说明
- `doc/RELEASE_YML_IMPROVEMENTS.md` - release.yml 改进总结

**更新文档**：
- `doc/README.md` - 添加版本号配置文档链接

## 📁 配置文件结构

```
mcphub-desktop/
├── .github/workflows/
│   └── release.yml              # GitHub Actions 配置（已改进）
├── src-tauri/
│   ├── tauri.conf.json          # Tauri 配置（版本号来源）
│   └── updater/
│       ├── mcphub.key           # 签名私钥
│       └── mcphub.key.pub       # 签名公钥
└── doc/
    ├── README.md                # 文档索引
    ├── VERSION_CONFIG.md        # 版本号配置说明
    └── RELEASE_YML_IMPROVEMENTS.md  # release.yml 改进总结
```

## 🔧 配置详解

### 版本号配置

**来源**：`src-tauri/tauri.conf.json`
```json
{
  "version": "1.0.18001"
}
```

**使用流程**：
1. 更新 `tauri.conf.json` 中的版本号
2. 创建对应的 tag（如 `v1.0.17`）
3. GitHub Actions 自动验证并构建

**验证逻辑**：
- 从 `tauri.conf.json` 读取版本号
- 验证 tag 和版本号是否一致
- 如果不一致，显示警告

### 签名配置

**来源**：`src-tauri/updater/mcphub.key`

**使用流程**：
1. 检查仓库中的私钥文件
2. 如果存在，使用仓库中的私钥
3. 如果不存在，回退到 GitHub Secrets

**优势**：
- 开源项目无需配置 GitHub Secrets
- 私钥文件已提交到仓库
- 简化配置流程

## 🚀 使用流程

### 1. 更新版本号

```bash
# 更新 tauri.conf.json 中的版本号
vim src-tauri/tauri.conf.json
# 修改 "version": "1.0.17"

# 提交更改
git add src-tauri/tauri.conf.json
git commit -m "chore: bump version to 1.0.17"
git push origin main
```

### 2. 创建 Release

```bash
# 创建 tag
git tag v1.0.17
git push origin v1.0.17
```

### 3. 监控构建

1. 访问 GitHub 仓库的 **Actions** 页面
2. 查看 "Release" workflow 的运行状态
3. 检查版本号是否正确读取
4. 等待所有平台构建完成

### 4. 发布 Release

1. 访问 GitHub 仓库的 **Releases** 页面
2. 找到新创建的 draft release
3. 添加发布说明
4. 点击 **Publish release**

## 🔍 验证清单

- [x] 版本号从 `tauri.conf.json` 读取
- [x] 签名私钥从仓库文件读取
- [x] 自动验证 tag 和版本号一致性
- [x] 文档已更新
- [ ] 测试本地构建
- [ ] 创建 release
- [ ] 验证版本号正确
- [ ] 测试自动更新

## 📚 相关文档

- [VERSION_CONFIG.md](./VERSION_CONFIG.md) - 版本号配置说明
- [CONFIGURATION_FINAL.md](./CONFIGURATION_FINAL.md) - 完整配置说明
- [src-tauri/tauri.conf.json](../src-tauri/tauri.conf.json) - Tauri 配置文件
- [.github/workflows/release.yml](../.github/workflows/release.yml) - GitHub Actions 配置

## 🎯 总结

release.yml 配置已改进，主要改进：

1. **版本号配置**：从 `tauri.conf.json` 读取，更加可靠
2. **签名配置**：优先使用仓库中的私钥文件，简化配置
3. **文档配置**：新增版本号配置说明文档

使用流程：
1. 更新 `tauri.conf.json` 中的版本号
2. 创建对应的 tag
3. GitHub Actions 自动验证并构建

---

💡 **提示**：确保 tag 和版本号一致，避免构建错误。
