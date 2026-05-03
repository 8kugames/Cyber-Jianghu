# Cyber-Jianghu Protocol

核心通信协议库，定义服务端与客户端之间所有共享的数据结构、消息格式和错误类型。本协议层采用无状态、数据驱动的设计理念，为游戏引擎和 AI Agent 提供统一的类型边界。

## 使用方式

```toml
[dependencies]
cyber-jianghu-protocol = { path = "crates/protocol" }
cyber-jianghu-protocol = { path = "crates/protocol", features = ["sqlx-support"] }
```

## 架构文档

详见 docs/architecture/

### P0 核心

| 文档 | 说明 |
|------|------|
| [action_type.md](docs/architecture/p0_core/action_type.md) | 数据驱动的动作类型系统 |
| [game_error.md](docs/architecture/p0_core/game_error.md) | 统一错误码体系 |
| [websocket_pipeline.md](docs/architecture/p0_core/websocket_pipeline.md) | WebSocket 全双工通信管道 |

### P1 重要特性

| 文档 | 说明 |
|------|------|
| [attribute_component.md](docs/architecture/p1_major/attribute_component.md) | COI 属性组件 |
| [dialogue_session.md](docs/architecture/p1_major/dialogue_session.md) | Agent 对话会话 |
| [hierarchical_map.md](docs/architecture/p1_major/hierarchical_map.md) | 层级位置图系统 |
| [soul_cycle_report.md](docs/architecture/p1_major/soul_cycle_report.md) | 三魂认知流转报告 |
| [subsequent_intents.md](docs/architecture/p1_major/subsequent_intents.md) | 多意图管道 |

### P2 体验增强

| 文档 | 说明 |
|------|------|
| [graded_llm_validation.md](docs/architecture/p2_enhancement/graded_llm_validation.md) | 分级 LLM 验证机制 |
| [immediate_event.md](docs/architecture/p2_enhancement/immediate_event.md) | 即时事件广播 |
| [nl_state_mapping.md](docs/architecture/p2_enhancement/nl_state_mapping.md) | 自然语言状态映射 |
| [numeric_leak_guard.md](docs/architecture/p2_enhancement/numeric_leak_guard.md) | 数值泄漏防护 |
| [world_building.md](docs/architecture/p2_enhancement/world_building.md) | 世界观设定边界 |

## 许可证

MIT OR Apache-2.0
