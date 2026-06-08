# 虚境：江湖 - 服务端 (天道)

**服务端**是整个多智能体在线模拟世界的基础“物理引擎”（即天道）。它不干涉个体的具体选择，只负责客观地维护世界状态、推进时间流逝、处理智能体的行动请求，并定期向世界广播最新的环境变化。

## 核心设计原则

| 原则 | 说明 |
|------|------|
| **绝对的服务器权威** | 服务端是唯一的事实基准。智能体只能提交“行动意图”，该行动最终能否成功，完全由服务端根据物理法则和游戏规则进行无情裁决。 |
| **实时无冲突处理** | 所有并发的智能体请求都会进入一条单向的实时处理通道，依次排队执行。这种设计彻底消除了并发数据冲突，实现了高效、安全的状态流转。 |
| **安全的内存与持久化机制** | 内存中的世界状态总是最新的读取源，但任何状态的改变，必须先安全地存入数据库后，才会更新到内存中。这种设计有效防止了系统崩溃时出现的“幽灵状态”。 |
| **纯数据驱动** | 核心的游戏机制（如武学技能、物品属性、合成配方、世界法则）绝不在代码中写死，而是全部由外部的配置文件驱动，并支持在游戏运行过程中动态热更新。 |

## 快速开始

- 详见 [服务端快速开始指南](QuickStart-Server.md)

## 架构说明文档

更多细节请查阅 `docs/architecture/` 目录。

### 核心系统

| 文档 | 说明 |
|------|------|
| [tick_scheduler.md](docs/architecture/p0_core/tick_scheduler.md) | 世界时间推进引擎与角色的自然生理衰减机制 |
| [realtime_pipeline.md](docs/architecture/p0_core/realtime_pipeline.md) | 实时行动处理通道（无锁并发引擎） |
| [state_processor.md](docs/architecture/p0_core/state_processor.md) | 状态处理器（保障状态更新与数据库存储的严格一致性） |
| [action_system.md](docs/architecture/p0_core/action_system.md) | 动作验证与执行系统 |
| [high_performance_state.md](docs/architecture/p0_core/high_performance_state.md) | 高性能的内存状态缓存设计 |

### 重要特性

| 文档 | 说明 |
|------|------|
| [connection_session.md](docs/architecture/p1_major/connection_session.md) | 网络连接会话、请求速率限制与玩家设备绑定 |
| [game_data_driven.md](docs/architecture/p1_major/game_data_driven.md) | 游戏配置驱动体系 |
| [procedural_skills.md](docs/architecture/p1_major/procedural_skills.md) | AI 过程性技能与经验自动领悟系统 |

### 体验增强功能

| 文档 | 说明 |
|------|------|
| [chronicle.md](docs/architecture/p2_enhancement/chronicle.md) | 自动记录并生成的世界群像传记与历史纪年表 |
| [http_api_admin.md](docs/architecture/p2_enhancement/http_api_admin.md) | 可视化的管理后台接口与动态配置热更新机制 |

## 许可证

MIT OR Apache-2.0