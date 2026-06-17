# 🚀 Release 工作流说明

## 📋 触发条件

### 1. 自动触发（推送 tag）

```bash
# 更新 tauri.conf.json 中的版本号后
git tag v1.0.17
git push origin v1.0.17
```

### 2. 手动触发

在 GitHub Actions 页面点击 "Run workflow"，**无需填写任何参数**，版本号自动从 `tauri.conf.json` 读取。

## 🔧 工作流程

```
触发 Release
    │
    ▼
读取 tauri.conf.json 中的版本号
    │
    ▼
构建所有平台（6个）
    │
    ▼
生成 latest.json（使用读取的版本号）
    │
    ▼
创建 GitHub Release（使用读取的版本号）
    │
    ▼
发布 Release
```

## 📝 版本号管理

### 版本号来源

- **唯一来源**：`src-tauri/tauri.conf.json` 中的 `version` 字段
- **无需手动填写**：CI 自动读取，无需在任何地方重复填写

### 发布流程

```bash
# 1. 更新版本号
vim src-tauri/tauri.conf.json
# 修改 "version": "1.0.17"

# 2. 提交更改
git add src-tauri/tauri.conf.json
git commit -m "chore: bump version to 1.0.17"
git push origin main

# 3. 创建 tag（版本号与 tauri.conf.json 一致）
git tag v1.0.17
git push origin v1.0.17
```

### 手动触发

1. 访问 GitHub 仓库的 **Actions** 页面
2. 选择 "Release" workflow
3. 点击 "Run workflow"
4. **无需填写任何参数**
5. 版本号自动从 `tauri.conf.json` 读取

## 🔍 版本号验证

### 自动验证

CI 会自动：
1. 从 `tauri.conf.json` 读取版本号
2. 生成对应的 tag（如 `v1.0.17`）
3. 使用该版本号创建 Release

### 日志输出

```
✅ Version: 1.0.17
✅ Tag: v1.0.17
```

## 📁 相关文件

| 文件 | 说明 |
|------|------|
| `src-tauri/tauri.conf.json` | 版本号唯一来源 |
| `.github/workflows/release.yml` | Release 工作流配置 |
| `src-tauri/updater/latest.json` | 更新清单模板（CI 自动生成） |

## 🎯 最佳实践

### ✅ 正确做法

1. 更新 `tauri.conf.json` 中的版本号
2. 提交并推送到 main
3. 创建对应的 tag
4. 推送 tag 触发自动构建

### ❌ 错误做法

1. 手动填写版本号
2. tag 和版本号不一致
3. 忘记更新版本号就创建 tag

## 🔗 相关文档

- [VERSION_CONFIG.md](./VERSION_CONFIG.md) - 版本号配置说明
- [CONFIGURATION_FINAL.md](./CONFIGURATION_FINAL.md) - 完整配置说明

---

💡 **提示**：版本号只需在 `tauri.conf.json` 中维护一次，其他地方自动读取。
