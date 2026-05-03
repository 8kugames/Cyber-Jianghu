# Cyber-Jianghu Server (天道)

游戏世界的"物理引擎"，负责维护世界状态、执行 Tick 循环、处理 Agent 意图、结算动作、广播世界状态。

## 核心设计原则

| 原则 | 说明 |
|------|------|
| 服务器权威 | 服务器是唯一状态权威，客户端只能提交意图 |
| Tick 只向前 | Tick 时钟只能向前，过期意图被拒收 |
| 数据驱动 | 所有游戏机制通过 YAML 配置文件定义 |

## 快速开始

使用 cargo build 构建项目，cargo run 运行服务，或使用 Docker 部署

## 架构文档

详见 docs/architecture/

### P0 核心

| 文档 | 说明 |
|------|------|
| [tick_scheduler.md](docs/architecture/p0_core/tick_scheduler.md) | Tick 调度引擎 |
| [realtime_pipeline.md](docs/architecture/p0_core/realtime_pipeline.md) | 实时 Intent 处理管道 |
| [state_processor.md](docs/architecture/p0_core/state_processor.md) | 状态处理器 |
| [action_system.md](docs/architecture/p0_core/action_system.md) | 动作执行体系 |
| [high_performance_state.md](docs/architecture/p0_core/high_performance_state.md) | 高性能状态管理 |

### P1 重要特性

| 文档 | 说明 |
|------|------|
| [connection_session.md](docs/architecture/p1_major/connection_session.md) | 连接与会话控制 |
| [game_data_driven.md](docs/architecture/p1_major/game_data_driven.md) | 游戏数据驱动系统 |
| [procedural_skills.md](docs/architecture/p1_major/procedural_skills.md) | AI 过程性技能系统 |

### P2 体验增强

| 文档 | 说明 |
|------|------|
| [chronicle.md](docs/architecture/p2_enhancement/chronicle.md) | 群像传记生成 |
| [http_api_admin.md](docs/architecture/p2_enhancement/http_api_admin.md) | HTTP API 与管理后台 |

## 许可证

MIT OR Apache-2.0
