
# Cyber-Jianghu 更新日志

本变更日志记录每次重要提交的汇总信息和影响面。

---

## [0.0.20] - 2026-03-22

### ⚠️ Breaking Changes

- **Agent**: 移除 `--mode` 命令行参数，现在只有 Claw 模式（默认）
  - 旧命令: `cyber-jianghu-agent --mode claw run`
  - 新命令: `cyber-jianghu-agent run`

- **Agent**: Intent API 响应格式变更
  - 旧格式: 纯文本 `"Intent submitted"`
  - 新格式: JSON `{"status": "submitted", "intent_id": "...", "tick_id": N, "action_type": "..."}`

### Fixed

- **Agent**: 修复 HTTP API 死锁问题
  - 问题: 注册回调中 RwLock 读锁未释放就尝试获取写锁，导致永久阻塞
  - 解决: 显式 `drop(old_id)` 释放读锁后再获取写锁
  - 影响: 修复后 HTTP API 正常响应

- **Server**: 修复生产环境部署失败问题
  - 修复空 Token 问题：环境变量为空字符串时自动生成随机 Token
  - 添加数据库迁移自动执行

- **Server**: 修复 `get_agent_by_device_id` 函数未导出问题
  - 添加到 `db/mod.rs` 导出列表

- **Agent**: 修复 Agent Docker 部署和数据库类型不匹配问题

### Added

- **Agent**: Cognitive Context API (`/api/v1/cognitive`)
  - 四阶段推理结构：Perception → Motivation → Planning → Decision
  - 引导 LLM 按认知流程进行决策

- **Agent**: 多角色管理系统
  - `GET /api/v1/characters` - 获取所有角色列表
  - `POST /api/v1/characters/switch` - 切换当前活跃角色
  - 支持已故和归隐角色的历史记录

- **Agent**: Web Panel 智能路由
  - 首页根据服务器连通性和角色状态自动跳转
  - 角色信息页支持多角色切换

- **Agent**: 服务器热切换 API
  - `POST /api/v1/config/server` - 动态切换服务器地址
  - 自动触发 WebSocket 重连

- **Server**: 设备认证系统
  - `POST /api/v1/agent/connect` - 设备注册获取 auth_token
  - WebSocket 连接需要 token 参数

- **Server**: Intent 全链路追踪
  - 每个 Intent 分配唯一 `intent_id`
  - 支持 `priority` 字段

### Changed

- **Agent**: 重构决策模式
  - 移除 `http` / `ws` / `cognitive` 模式区分
  - 统一为 Claw 模式（HTTP API + WebSocket 服务）

- **Agent**: 版本号 0.0.15 → 0.0.16 → 0.0.20

- **Config**: `CharacterConfig` 新增字段
  - `server_url`: 角色所属服务器
  - `status`: 角色状态 (alive/dead/retired)

### Removed

- **Agent**: 移除过时的 OpenClaw 内联模式代码
- **Agent**: 移除 `--mode` 命令行参数

---

## [0.0.16] - 2026-03-22

### Added

- **Agent**: 多角色管理系统
  - 支持在同一设备上管理多个角色（包括已故和归隐角色）
  - 每个角色关联到特定服务器，记录角色来源
  - 新增 `CharacterStatus` 枚举（Alive/Dead/Retired）跟踪角色状态
  - `GET /api/v1/characters` - 获取所有角色列表
  - `POST /api/v1/characters/switch` - 切换当前活跃角色

- **Agent**: Web Panel 智能路由
  - 首页根据服务器连通性和角色状态自动跳转
  - 无角色或服务器不可达时优先显示管理页
  - 有存活角色且服务器可达时显示角色信息页

- **Agent**: 角色信息页增强
  - 多角色选择器，支持在存活角色间切换
  - 显示角色所属服务器
  - 支持查看已故和归隐角色

- **Agent**: 服务器切换改进
  - 切换服务器时正确检测设备注册状态
  - 返回 `needs_device_registration` 和 `needs_character_creation` 标志
  - 显示该服务器上的历史角色列表

### Changed

- **Agent**: 版本号从 0.0.15 升级到 0.0.16
- **Config**: `CharacterConfig` 新增 `server_url` 和 `status` 字段
- **Config**: `Config` 新增 `characters` 数组存储角色历史

### Fixed

- **Agent**: 修复服务器切换时的 RwLock 使用错误（identity 不是 RwLock）

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
