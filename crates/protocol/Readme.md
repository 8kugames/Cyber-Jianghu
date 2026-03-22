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

| 文档 | 说明 |
|------|------|
| 01_概述.md | 概述 |
| 02_消息类型.md | 消息类型 |
| 03_类型定义.md | 类型定义 |
| 04_审核.md | Review System |

## 许可证

AGPL-3.0 License
