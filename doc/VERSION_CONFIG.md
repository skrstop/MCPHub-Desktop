# 📋 版本号配置说明

## 🎯 版本号来源

### 改进前

版本号从 GitHub tag 中提取：
```python
tag = os.environ['RELEASE_TAG']  # e.g. v1.0.17
version = tag.lstrip('v')        # e.g. 1.0.17
```

**问题**：
- 版本号依赖于 tag 名称格式
- 如果 tag 格式不正确，可能导致版本号错误
- 无法验证 tag 和版本号是否一致

### 改进后

版本号从 `src-tauri/tauri.conf.json` 中读取：
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

## 🔧 配置文件

### 版本号配置位置

**文件**：`src-tauri/tauri.conf.json`

```json
{
  "$schema": "https://schema.tauri.app/config/2",
  "productName": "MCPHub Desktop",
  "version": "1.0.18001",  // ← 版本号在这里
  "identifier": "app.mcphub.desktop",
  ...
}
```

### 版本号格式

- 使用语义化版本号：`主版本.次版本.修订版本`
- 示例：`1.0.18001`、`1.1.0`、`2.0.0`

## 🚀 使用流程

### 1. 更新版本号

在发布新版本前，更新 `src-tauri/tauri.conf.json` 中的版本号：

```json
{
  "version": "1.0.17"  // ← 更新为新版本号
}
```

### 2. 创建 tag

创建与版本号对应的 tag：

```bash
# 版本号是 1.0.17，tag 应该是 v1.0.17
git tag v1.0.17
git push origin v1.0.17
```

### 3. 自动验证

GitHub Actions 会自动验证：
- ✅ 从 `tauri.conf.json` 读取版本号
- ✅ 验证 tag 和版本号是否一致
- ✅ 如果不一致，显示警告信息

## 🔍 验证逻辑

### 版本号读取流程

```python
# 1. 尝试从 tauri.conf.json 读取版本号
try:
    with open('src-tauri/tauri.conf.json', 'r') as f:
        tauri_conf = json.load(f)
    version = tauri_conf.get('version', '')
except:
    # 2. 如果读取失败，回退到 tag 名称
    version = tag.lstrip('v')

# 3. 验证 tag 和版本号是否一致
expected_tag = f'v{version}'
if tag != expected_tag:
    print(f'WARNING: Tag "{tag}" does not match version "v{version}"')
```

### 验证输出示例

**一致的情况**：
```
✅ Version from tauri.conf.json: 1.0.17
```

**不一致的情况**：
```
✅ Version from tauri.conf.json: 1.0.17
WARNING: Tag "v1.0.18001" does not match version "v1.0.17"
         Expected tag: v1.0.17
```

## 📋 版本号管理最佳实践

### 1. 版本号更新

- ✅ 在发布新版本前更新 `tauri.conf.json` 中的版本号
- ✅ 使用语义化版本号格式
- ✅ 确保版本号递增

### 2. Tag 创建

- ✅ 创建与版本号对应的 tag
- ✅ 使用 `v` 前缀（如 `v1.0.17`）
- ✅ 推送到远程仓库

### 3. 验证

- ✅ 检查 GitHub Actions 日志，确认版本号正确
- ✅ 验证 release 中的版本号与 `tauri.conf.json` 一致
- ✅ 测试自动更新功能

## 🛠️ 故障排除

### 问题：版本号不一致

**症状**：GitHub Actions 显示警告
```
WARNING: Tag "v1.0.18001" does not match version "v1.0.17"
```

**原因**：tag 和 `tauri.conf.json` 中的版本号不一致

**解决**：
1. 检查 `src-tauri/tauri.conf.json` 中的版本号
2. 删除错误的 tag：`git tag -d v1.0.18001 && git push origin :refs/tags/v1.0.18001`
3. 创建正确的 tag：`git tag v1.0.17 && git push origin v1.0.17`

### 问题：版本号读取失败

**症状**：GitHub Actions 显示警告
```
WARNING: Failed to read src-tauri/tauri.conf.json: [Errno 2] No such file or directory
```

**原因**：`tauri.conf.json` 文件不存在或无法读取

**解决**：
1. 检查文件是否存在：`ls -la src-tauri/tauri.conf.json`
2. 检查文件权限
3. 确保文件格式正确

## 📚 相关文档

- [src-tauri/tauri.conf.json](../src-tauri/tauri.conf.json) - Tauri 配置文件
- [.github/workflows/release.yml](../.github/workflows/release.yml) - GitHub Actions 配置
- [CONFIGURATION_FINAL.md](./CONFIGURATION_FINAL.md) - 完整配置说明

## 🎯 总结

版本号配置已改进，现在从 `tauri.conf.json` 中读取版本号，更加可靠和一致。使用流程：

1. 更新 `tauri.conf.json` 中的版本号
2. 创建对应的 tag
3. GitHub Actions 自动验证并构建

---

💡 **提示**：确保 tag 和版本号一致，避免构建错误。
