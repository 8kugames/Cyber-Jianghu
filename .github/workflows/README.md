# GitHub Workflows

## Workflows

| Workflow | 用途 | 触发条件 |
|----------|------|---------|
| **ci.yml** | 版本更新 + 5平台构建 + Docker + Release | PR 合并、Tag push、手动触发 |
| **pr-check.yml** | 快速验证 (构建 + 测试) | PR 创建/更新 |

## 构建矩阵

| 平台 | Target | 方式 |
|------|--------|------|
| Linux x86_64 | `x86_64-unknown-linux-musl` | cross 交叉编译 |
| Linux arm64 | `aarch64-unknown-linux-musl` | cross 交叉编译 |
| macOS x86_64 | `x86_64-apple-darwin` | 原生 (macos-13) |
| macOS arm64 | `aarch64-apple-darwin` | 原生 (macos-latest) |
| Windows x86_64 | `x86_64-pc-windows-msvc` | 原生 |

## 使用方法

### 日常开发

```bash
# 1. 创建分支并提交
git checkout -b feature/new-feature
git commit -m "feat: 新功能"
git push origin feature/new-feature

# 2. 创建 PR
# pr-check.yml 自动运行: cargo build + cargo test

# 3. 合并 PR
# ci.yml 自动:
#   - 更新版本号 (0.1.0 → 0.1.1)
#   - 5 平台构建
#   - Docker 镜像发布
```

### 发布版本

```bash
# 1. 手动升级版本号 (如需 MINOR/MAJOR)
vim crates/server/Cargo.toml  # version = "0.2.0"
cargo update --workspace
git commit -m "chore: release v0.2.0"

# 2. 打 tag 并推送
git tag v0.2.0
git push origin main --tags

# 3. CI 自动创建 GitHub Release (含 10 个二进制文件)
```

## CI 执行流程

```
PR 合并/Tag push
       ↓
[1] version-bump  → 更新 Cargo.toml 版本号
       ↓
[2] build         → 5 平台并行构建 (cross + 原生)
       ↓
[3] docker-build  → 推送 ghcr.io 镜像
       ↓
[4] release       → GitHub Release (仅 tag 触发)
```

## 版本管理

| 类型 | 触发方式 | 示例 |
|------|----------|------|
| PATCH | PR 合并自动 | 0.1.0 → 0.1.1 |
| MINOR | 手动修改 | 0.1.0 → 0.2.0 |
| MAJOR | 手动修改 | 0.1.0 → 1.0.0 |

## 监控的 Crates

- `crates/server/`
- `crates/protocol/`

## 产物命名

```
cyber-jianghu-server-linux-x86_64
cyber-jianghu-server-linux-arm64
cyber-jianghu-server-macos-x86_64
cyber-jianghu-server-macos-arm64
cyber-jianghu-server-windows-x86_64.exe
cyber-jianghu-agent-* (同上)
```
