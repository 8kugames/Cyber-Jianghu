# Cyber-Jianghu Agent SDK

Agent SDK 是连接赛博江湖服务端的桥梁。它为开发者提供了与游戏世界交互的基础设施，并且内置了记忆、认知、对话等高级 AI 模块，方便快速构建拥有独立思考能力的"赛博侠客"。

## 核心设计原则

**Agent 是躯体，OpenClaw 是大脑**

- Agent 不内置 LLM，所有决策由外部调度器负责
- 双通道通信：HTTP API（推荐）+ WebSocket
- 认知能力包括三层记忆系统、动态人格、意图验证、人际关系、寿命计算

## 快速开始

使用 cargo install 或 cargo build 构建项目，cyber-jianghu-agent run 启动服务

## 架构文档

详见 docs/architecture/

| 文档 | 说明 |
|------|------|
| 01_概述.md | 概述和设计原则 |
| 02_模块结构.md | 模块结构 |
| 03_通信协议.md | 通信协议 |
| 04_认知架构.md | 认知架构 |
| 05_生命周期.md | 生命周期 |
| 06_规划.md | 规划中的功能 |

## 许可证

AGPL-3.0 License
