# 虚境：江湖 - 通信协议层

这是游戏的核心通信协议库，定义了服务端（天道）与智能体（众生）之间所有共享的数据结构、消息交互格式以及错误类型。

本协议层采用无状态、纯数据驱动的设计理念，为游戏物理引擎和 AI 智能体之间划定了一道清晰、统一的边界。

## 使用方式

在 Rust 项目中引入：

```toml
[dependencies]
cyber-jianghu-protocol = { path = "crates/protocol" }
# 如果需要数据库支持
cyber-jianghu-protocol = { path = "crates/protocol", features = ["sqlx-support"] }
```

## 架构说明文档

更多细节请查阅 `docs/architecture/` 目录。

### 核心系统

| 文档 | 说明 |
|------|------|
| [action_type.md](docs/architecture/p0_core/action_type.md) | 完全由数据驱动的动作类型系统 |
| [game_error.md](docs/architecture/p0_core/game_error.md) | 统一规范的错误码体系 |
| [websocket_pipeline.md](docs/architecture/p0_core/websocket_pipeline.md) | 实时全双工通信管道设计 |

### 重要特性

| 文档 | 说明 |
|------|------|
| [attribute_component.md](docs/architecture/p1_major/attribute_component.md) | 模块化的角色属性组件 |
| [dialogue_session.md](docs/architecture/p1_major/dialogue_session.md) | 智能体之间的对话会话管理 |
| [hierarchical_map.md](docs/architecture/p1_major/hierarchical_map.md) | 层级化的世界地图位置系统 |
| [soul_cycle_report.md](docs/architecture/p1_major/soul_cycle_report.md) | 三魂认知流转报告与追踪 |
| [subsequent_intents.md](docs/architecture/p1_major/subsequent_intents.md) | 连续的原子行动意图队列 |

### 体验增强

| 文档 | 说明 |
|------|------|
| [graded_llm_validation.md](docs/architecture/p2_enhancement/graded_llm_validation.md) | 分级的大模型行为合规验证机制 |
| [immediate_event.md](docs/architecture/p2_enhancement/immediate_event.md) | 突发事件的即时广播机制 |
| [nl_state_mapping.md](docs/architecture/p2_enhancement/nl_state_mapping.md) | 机器状态到自然语言的自动映射 |
| [numeric_leak_guard.md](docs/architecture/p2_enhancement/numeric_leak_guard.md) | 防止大模型输出暴漏底层数值的防护机制 |
| [world_building.md](docs/architecture/p2_enhancement/world_building.md) | 游戏世界观与时代设定的边界限制 |

## 许可证

MIT OR Apache-2.0