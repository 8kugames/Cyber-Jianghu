
# Cyber-Jianghu 更新日志

本变更日志记录每次重要提交的汇总信息和影响面。

---

## [Unreleased]

### ⚠️ Breaking Changes

- **Agent**: CLI 移除 `--role` 和 `--target-endpoint` 参数
  - 移除远程 Observer 模式（HTTP 轮询其他 Agent）
  - ReflectorSoul 现在作为进程内双 Soul 架构默认启用
  - 原因：简化架构，统一使用 AgentBuilder 接口

- **Agent**: HTTP Intent API 禁用
  - 移除 `POST /api/v1/intent` 路由
  - 强制使用 WebSocket 提交 Intent（确保 Tick 同步）
  - 原因：HTTP 轮询无法保证 tick_id 实时同步，会导致意图被拒绝

### Added

- **Agent**: ActorSoul 和 ReflectorSoul LLM 独立配置
  - 新增 `llm_reflector` 配置字段，支持独立配置 ReflectorSoul LLM
  - 新增 GET /api/v1/config/llm/providers 端点
  - 新增 GET /api/v1/config/llm 端点获取当前配置
  - 新增 POST /api/v1/config/llm 端点更新配置
  - Web 面板新增 LLM 配置界面
  - 配置变更通过文件监听自动热重载
  - API Key 格式验证和内存安全（zeroize）
  - 配置更新原子替换 + 备份回滚机制

- **Agent**: ActorSoul + ReflectorSoul 双 Soul 架构
  - 新增 `ReviewStore` 共享内存用于进程内审查通信
  - ActorSoul (行动之魂)：生成意图，执行行动
  - ReflectorSoul (反思之魂)：审查意图，道德判断（默认启用）
  - AgentBuilder 新增 `with_review_store()` 和 `with_reconnect_rx()` 方法

- **Agent**: 审查系统默认启用
  - Cognitive 和 Claw 模式均默认启用 ReflectorSoul
  - 支持三种审查结果：Approved、Rejected、TimeoutApproved
  - 审查超时自动批准（默认 30 秒）

- **Agent**: 架构统一（COI 原则）
  - Cognitive 和 Claw 模式统一使用 AgentBuilder
  - 移除 `Agent::new()` 的使用（改用 Builder）
  - 确保两种模式功能完全一致

- **Server**: agent_id → device_id 反向映射系统
  - 新增 `AgentToDeviceMap` 类型维护角色到设备的映射
  - 在 `agent_register` 和 WebSocket 连接时自动更新映射
  - 解决设备与角色分离后，WorldState 广播找不到正确连接的问题

- **Agent**: WebSocket Tick 消息集成四阶段认知上下文
  - `DownstreamMessage::Tick` 新增 `cognitive_context` 字段
  - 结构化四阶段推理引导：Perception → Motivation → Planning → Decision
  - OpenClaw 可直接使用认知上下文进行推理，无需额外 API 调用

### Changed

- **Agent**: 配置文件新增 `config_path` 字段

- **Server**: WebSocket 连接管理改用 device_id 作为 key
  - 连接管理器现在以 device_id 而非 agent_id 存储连接
  - 支持同一设备管理多角色的场景

### Removed

- **Agent**: 移除远程 Observer 模式相关代码
  - 删除 `run_observer_mode()` 函数
  - 删除 `fetch_pending_reviews()` 和 `process_review_remote()` 函数
  - 删除 `--role observer` 和 `--target-endpoint` CLI 参数
  - 保留 HTTP API 端点供外部监控工具使用

- 删除过时的设计文档：
  - `docs/openclaw-cognitive-integration.md`
  - `docs/superpowers/plans/2026-03-23-agent-death-notification.md`
  - `docs/superpowers/specs/2026-03-22-agent-openclaw-error-forwarding-design.md`
  - `docs/superpowers/specs/2026-03-23-agent-death-notification-design.md`
  - `联调测试.md`

---

## [0.0.33] - 2026-03-23

### Added

- **Agent**: Server → OpenClaw 消息透传机制
  - Agent 实时转发 Server 下行消息给 OpenClaw（WebSocket）
  - 支持：错误消息、对话消息、游戏规则更新、世界观规则更新
  - 新增 `ServerErrorCode` 结构化错误码枚举
  - 新增 `DownstreamMessage` 变体：`ServerError`、`ServerDialogue`、`ServerGameRulesUpdate`、`ServerWorldBuildingRulesUpdate`、`MissedMessages`

- **Agent**: WebSocket Server 安全限制
  - 仅允许 localhost 连接（拒绝远程连接）
  - 单连接限制（同一时间只允许一个 OpenClaw 连接）
  - 连接断开时自动释放 slot

- **Agent**: WebSocket Client 回调机制
  - 新增 `set_server_msg_callback()` 方法
  - 收到 Server 消息时触发回调，实现消息透传

### Fixed

- **Agent**: 修复单连接限制的竞态条件
  - 问题：拒绝第二个连接时错误地释放了第一个连接的 slot
  - 解决：拒绝连接时不调用 `store(false)`，slot 由已建立连接在断开时释放

### Changed

- **Agent**: 版本号 0.0.29 → 0.0.33

### Technical Details

消息流转路径：
```
Game Server → WebSocket Client → server_msg_callback → broadcast::Sender
           → WebSocket Server → OpenClaw
```

新增 API：
- `Agent::set_server_msg_callback(callback)` - 设置 Server 消息透传回调
- `AgentClient::set_server_msg_callback(callback)` - 同上
- `WebSocketClient::set_server_msg_callback(callback)` - 同上

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
