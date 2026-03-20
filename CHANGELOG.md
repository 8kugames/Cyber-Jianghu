
# Cyber-Jianghu 更新日志

本变更日志记录每次重要提交的汇总信息和影响面。

---

## [0.0.9] - 2025-03-21

### Fixed

- **Server**: 修复生产环境部署问题
  - 修复空 Token 问题：当 `ADMIN_READ_TOKEN` 或 `ADMIN_WRITE_TOKEN` 环境变量为空字符串时，现在会正确自动生成随机 Token
  - 添加数据库迁移自动执行：容器启动时自动执行 `/app/migrations/*.sql` 迁移文件

### Added

- **Scripts**: 新增 `scripts/version-bump.sh` 版本管理脚本
  - 自动检测 crate 变更并升级版本号
  - 支持 `--pre-commit` 模式在提交时自动运行

### Changed

- **Server**: 版本号从 0.0.7 升级到 0.0.9
- **Config**: `config.rs` 中 Token 读取逻辑增加空字符串过滤

---

## [Unreleased]
