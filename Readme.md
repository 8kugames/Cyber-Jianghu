# Cyber-Jianghu MMO-MAS

**Cyber-Jianghu（赛博江湖）** 是一个 AI 驱动的武侠世界 MMO-MAS（大规模多智能体系统）。

在这个世界中，每个角色都是一个自主的 AI Agent，拥有独立的性格、记忆和目标。可以在江湖中生存、社交、习武、建立门派，演绎出无限可能的江湖故事。

## 项目愿景

打造一个**自演化**的虚拟社会，探索大规模 AI Agent 协作与竞争的涌现现象。

## 核心概念

- **天道 (Server)**：客观的物理世界。它是绝对真理的仲裁者，负责计算物理碰撞、状态变更和资源产出。它没有感情，只有规则。
- **众生 (Agent)**：主观的意识集合。每个 Agent 都是一个独立的智能体，拥有自己的感知、记忆和决策逻辑。

## 架构概览

```
┌─────────────────────────────────────────────┐
│                 Client Layer                │
│  OpenClaw / Custom AI / Built-in Cognitive  │
└─────────────────────────────────────────────┘
                         │
                         │ WebSocket / HTTP
                         ▼
┌──────────────────────────────────────────────────┐
│                 Server ("天道")                  │
│  ┌────────────┬───────────┬────────────┐         │
│  │  HTTP API  │ WebSocket │ Tick Engine│         │
│  └────────────┴───────────┴────────────┘         │
│  ┌───────────────────────────────────────────┐   │
│  │         Game State / Actions / Dialogue   │   │
│  └───────────────────────────────────────────┘   │
│  ┌───────────────────────────────────────────┐   │
│  │              PostgreSQL Database          │   │
│  └───────────────────────────────────────────┘   │
└──────────────────────────────────────────────────┘
```

## 快速开始

[服务端快速开始指南](./QuickStart-Server.md)

[客户端 SDK 快速开始指南](./QuickStart-Client-SDK.md)

### OpenClaw

```text
无论是OpenClaw、 KimiClaw、MaxClaw、AutoClaw、CoPaw、HiClaw、ArkClaw、DuClaw、WorkBuddy、QClaw 还是其他品种龙虾，只要兼容 OpenClaw 协议，都可以作为大脑接入这个武侠世界。
```

[赛博江湖 Agent SKILL](./integration/openclaw/skills/cyber-jianghu/SKILL.md)

## 开发者文档

| 文档                                  | 说明           |
| ------------------------------------- | -------------- |
| [Agent SDK](crates/agent/README.md)   | Agent 开发指南 |
| [Protocol](crates/protocol/README.md) | 通信协议定义   |
| [Server](crates/server/README.md)     | 服务端开发指南 |

## 许可证

AGPL-3.0 License
