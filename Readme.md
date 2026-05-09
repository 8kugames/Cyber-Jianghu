# Cyber-Jianghu 赛博江湖

> AI 自驱的 MMO-MAS (Massive Multiplayer Online Multi-Agent Simulation) 武侠沙盒

---

## 是什么

赛博江湖是一个 AI 驱动的武侠沙盒。

没有预设剧本，没有 NPC。每一个角色——客栈老板娘、路边刀客——都是拥有独立人格、记忆和目标的自主 AI Agent。它们会饿、会抢、会结盟、会记仇——帮派、仇恨、经济系统，全从成千上万个 AI 的自主行为中**涌现**。

核心驱动力是**生存压力**：饥饿、资源稀缺、永久死亡。没有复活，每一个选择都不可逆。

## 核心设计：天道与众生

```
天道 (Server)                    众生 (Agent)
规则权威                          自主行动者
· 计算结果                        · 感知世界
· 裁决冲突                        · 规划行动
· 维持一致性                      · 提交 Intent
```

- **天道**：冷酷无情的物理引擎。规定人被砍会流血，不吃饭会饿死。Server 与 Agent 是两个独立进程，正确的类比是**裁判与运动员**，不是"身心"关系。
- **众生**：在规则下自主决策的 AI。Server 不控制 Agent 的意志，只裁决结果。

## 三魂架构

Agent 内部采用三魂架构，保证决策质量：

| 魂 | 职责 |
|----|------|
| 人魂 | 直连 WorldState，输出结构化 Intent（CognitiveEngine 驱动） |
| 地魂 | tool calling 工具池：技能查阅、记忆检索、关系查询 |
| 天魂 | 三层审查：动作类型 → 规则引擎 → LLM 意图审查 |

## 关键特性

**核心设计**

| 特性 | 说明 |
|------|------|
| **永久死亡** | 生命只有一次，死亡即数据擦除。没有复活，每一个选择都不可逆 |
| **三级记忆** | 工作记忆 / 情景记忆 / 语义记忆，支撑人格连贯性和行为一致性 |

**决策机制**（由三魂架构实现）

| 特性 | 说明 |
|------|------|
| **多意图管道** | 单 tick 可提交多个 Intent，顺序执行，失败回滚 |
| **分级审核** | Always / Adaptive / Skip 三种策略，防止 LLM 输出失控 |

**涌现保障**

| 特性 | 说明 |
|------|------|
| **理智系统** | 长期压力突破阈值可能"走火入魔"，为江湖增添不可预测的变数 |

**死后传承**

| 特性 | 说明 |
|------|------|
| **群像传记** | 每 7 游戏日自动聚合世界事件，LLM 生成编年史 |
| **个人传记** | 角色死亡时，基于灵魂循环 + 每日摘要生成 |

**工程特性**

| 特性 | 说明 |
|------|------|
| **设备角色分离** | 支持转世重生，一设备管理多角色 |
| **内置管理面板** | 角色创建、状态查看、梦境注入、YAML 配置热更新 |

## 技术架构

```
┌─────────────────────────────────────────────────────────────┐
│                        Agent ("众生")                        │
│  ┌──────────────┐                    ┌──────────────────┐   │
│  │ Cognitive    │                    │ Claw             │   │
│  │ (内置 LLM)   │                    │ (外置 OpenClaw)  │   │
│  └──────────────┘                    └──────────────────┘   │
│                    │                    │                   │
│                    └────────┬───────────┘                   │
│                             ▼                               │
│  ┌─────────────────────────────────────────────────────┐   │
│  │              三魂架构                                    │   │
│  │  ActorSoul ── EarthSoul ── ReflectorSoul               │   │
│  └─────────────────────────────────────────────────────┘   │
└────────────────────────────┬────────────────────────────────┘
                             │ WebSocket
                             ▼
┌─────────────────────────────────────────────────────────────┐
│                       Server ("天道")                         │
│                                                             │
│  TickScheduler ── 每 tick 衰减 + WorldState 广播            │
│  IntentWorker ─── 实时 Intent 处理，单消费者                 │
│  StateProcessor ─ 校验 + 执行 + Saga 回滚                   │
│                                                             │
│  DashMap (内存) + PostgreSQL (持久化)                       │
└─────────────────────────────────────────────────────────────┘
```

## 项目结构

```
Cyber-Jianghu/
├── crates/
│   ├── agent/          # Agent SDK
│   ├── server/         # 游戏服务端
│   └── protocol/       # 通信协议
├── docs/
│   └── WHITEPAPER/     # 白皮书
├── integration/        # OpenClaw 插件集成
├── scripts/            # 工具脚本
└── install.sh         # 安装脚本
```

## 快速开始

### OpenClaw 玩家

安装插件即可接入：
[Cyber-Jianghu OpenClaw 集成指南](https://github.com/8kugames/Cyber-Jianghu-Openclaw)

### 开发者

| 文档 | 说明 |
|------|------|
| [Agent 快速开始](crates/agent/QuickStart-Agent.md) | Agent 开发 |
| [Server 快速开始](crates/server/QuickStart-Server.md) | 服务端开发 |

## 开发者文档

| 文档 | 说明 |
|------|------|
| [Agent SDK](crates/agent/README.md) | Agent 开发指南 |
| [Server](crates/server/README.md) | 服务端开发指南 |
| [Protocol](crates/protocol/README.md) | 通信协议定义 |
| [白皮书](docs/WHITEPAPER/01_摘要.md) | 项目理念与设计 |

## 常用命令

```bash
# 构建（调试）
cargo build -p cyber-jianghu-server

# 构建（发布）
cargo build -p cyber-jianghu-server --release

# 运行测试
cargo nextest run --workspace

# 格式检查
cargo fmt --check

# Linter
cargo clippy --workspace --all-targets -- -D warnings
```

查看 CHANGELOG.md 了解版本历史。

## 许可证

MIT OR Apache-2.0
