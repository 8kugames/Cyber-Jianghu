# Cyber-Jianghu 更新日志

本变更日志记录每次重要提交的汇总信息和影响面。

---

## [Unreleased]

### 2025-03-20 - Agent 注册系统重构（设备身份与角色身份分离）

**提交摘要**: 重构 Agent 注册系统，将设备身份（device）与角色身份（agent）完全分离，支持一个设备创建多个角色（转世机制）。

#### 数据库变更

| 文件 | 变更 |
|------|------|
| `migrations/001_initial_schema.sql` | agents 表移除 `auth_token`，添加 `device_id` 外键 |
| `migrations/005_devices.sql` | 新增 devices 表（设备身份存储） |
| `migrations/006_agents_device_link.sql` | 迁移脚本：添加 device_id 列，移除 auth_token |

**影响面**: ⚠️ **不向后兼容** - 需要重建数据库或运行迁移

#### Server 变更

| 文件 | 变更 |
|------|------|
| `src/models/agent.rs` | Agent 结构体移除 `auth_token`，添加 `device_id: Uuid` |
| `src/db/agent_ops.rs` | 新增设备相关操作，更新 `register_agent_transactional` 关联 device_id |
| `src/db/mod.rs` | 更新导出列表 |
| `src/handlers/agent.rs` | 新增 `/api/v1/agent/connect` 端点，更新角色注册流程 |
| `src/websocket/types.rs` | WebSocketQuery 添加 `device_id` 和 `agent_id` 字段 |
| `src/websocket/handler.rs` | 使用 `verify_device_token` 验证设备身份 |

#### Agent 变更

| 文件 | 变更 |
|------|------|
| `src/config.rs` | 分离 `IdentityConfig`（设备）和 `CharacterConfig`（角色） |
| `src/bin/cyber-jianghu-agent.rs` | 新增 `ensure_identity()` 自动注册设备 |
| `src/runtime/decision/http/handlers.rs` | 新增 `/api/v1/character/register` 转发端点 |
| `static/panel/*` | Web 角色创建面板 |

#### Docker 变更

| 文件 | 变更 |
|------|------|
| `crates/agent/docker-compose.yml` | 更新环境变量，移除废弃配置 |
| `crates/agent/docker-compose.prod.yml` | 同上 |
| `crates/agent/Dockerfile` | 添加 static 目录复制 |
| `crates/server/docker-compose.yml` | 添加 005、006 迁移挂载 |

#### 认证流程变更

```
旧流程:
  注册 → 获取 agent.auth_token → WebSocket 连接

新流程:
  设备注册 → 获取 device.auth_token → WebSocket 连接 → 创建角色
```

#### 环境变量变更

**废弃**:
- `CYBER_JIANGHU_AGENT_NAME`
- `CYBER_JIANGHU_SYSTEM_PROMPT`
- `CYBER_JIANGHU_AUTH_TOKEN`

**新增**:
- `CYBER_JIANGHU_SERVER_WS_URL`
- `CYBER_JIANGHU_SERVER_HTTP_URL`
- `CYBER_JIANGHU_RUNTIME_MODE`
- `CYBER_JIANGHU_PORT`

---

## [0.0.7] - 2025-03-19

### 新增
- 仪表板显示代理状态最大值和先天属性
- 服务端可配置时区和平滑游戏时间显示

### 修复
- 修正 agent 依赖中的重复 version 字段

---

## 更新日志格式说明

每个条目应包含：

1. **提交摘要**: 简要描述本次变更的目的
2. **数据库变更**: Schema 变更、迁移脚本
3. **Server 变更**: 服务端代码变更
4. **Agent 变更**: Agent SDK 代码变更
5. **Docker 变更**: 部署配置变更
6. **认证流程变更**: 如有涉及认证机制的变更
7. **环境变量变更**: 废弃/新增的环境变量
8. **影响面**: 变更的影响范围（兼容性警告）

---

*本文件由开发团队维护，记录所有重要变更。*
