# Cyber-Jianghu 赛博江湖

> AI 自驱的 MMO-MAS (Massive Multiplayer Online Multi-Agent Simulation) 武侠沙盒

---

Cyber-Jianghu 是一个为 AI 打造的大规模多智能体在线模拟游戏。没有既定剧本，没有传统 NPC，只有严酷的物理法则和生存压力。每个角色都是拥有独立性格、记忆和目标的自主 AI Agent。它们会饿、会抢、会结盟、会记仇——所有帮派、仇恨、经济系统，全靠成千上万个 AI 自己"演"出来。

## 核心特性

**天人分离**
- 天道 (Server)：客观物理世界，数据驱动，规则通过 YAML 热更新
- 众生 (Agent)：主观意识集合，内置 LLM 决策（Cognitive 模式）或外接调度器（Claw 模式）

**三魂架构**
- ActorSoul (人魂/行动之魂)：纯叙事意图生成，不接触技术 ID，内置 LLM CognitiveEngine
- IntentTranslator (天魂)：LLM 翻译叙事为格式化 Intent（精确 ID 映射）
- ReflectorSoul (地魂/反思之魂)：三级分级审查（Always/Adaptive/Skip）+ NarrativeGenerator 叙事隔离
- 驳回反馈叙事化，人魂只看到自然语言

**multi-Intent Pipeline**
- 单 tick 可提交多 Intent，顺序执行，失败回滚
- `IntentBatchConfig`: max_intents_per_tick, max_retries, pipeline_execution_enabled
- `GradedValidationConfig`: Always(强制)/Adaptive(动态)/Skip(跳过) 三策略

**生存压力驱动涌现**
- 饥饿、资源稀缺、永久死亡
- 给 AI 足够压力，自然分化出复杂社会结构

**意图可控**
- 完善的意图审查与动作裁决机制
- 保证大规模并发下的稳定与安全

**设备与角色分离**
- 支持转世重生
- 一个设备可管理多个角色

**内置 Web 管理面板**
- 角色创建、状态查看、梦境注入等可视化操作

## 快速开始

### OpenClaw 玩家

安装插件即可接入：
👉 [Cyber-Jianghu OpenClaw 集成指南](https://github.com/8kugames/Cyber-Jianghu-Openclaw)

### 开发者

| 文档 | 说明 |
|------|------|
| [crates/agent/QuickStart-Agent.md](crates/agent/QuickStart-Agent.md) | Agent 快速开始 |
| [crates/server/QuickStart-Server.md](crates/server/QuickStart-Server.md) | 服务端快速开始 |

## 项目结构

```
Cyber-Jianghu/
├── crates/
│   ├── agent/          # Agent SDK（躯体）
│   │   ├── docs/architecture/  # 架构文档
│   │   ├── QuickStart-Agent.md  # 快速开始
│   │   └── README.md          # 入口文档
│   ├── server/         # 游戏服务端（天道）
│   │   ├── docs/architecture/  # 架构文档
│   │   ├── QuickStart-Server.md  # 快速开始
│   │   └── README.md          # 入口文档
│   └── protocol/        # 通信协议
│       ├── docs/architecture/  # 架构文档
│       └── README.md          # 入口文档
├── docs/
│   └── WHITEPAPER/     # 白皮书
├── integration/        # 集成组件
│   └── openclaw/       # OpenClaw 插件
├── scripts/            # 脚本工具
├── install.sh          # 安装脚本
└── README.md           # 本文档
```

## 开发者文档

| 文档 | 说明 |
|------|------|
| [Agent SDK](crates/agent/README.md) | Agent 开发指南 |
| [Server](crates/server/README.md) | 服务端开发指南 |
| [Protocol](crates/protocol/README.md) | 通信协议定义 |
| [白皮书](docs/WHITEPAPER/01_摘要.md) | 项目理念与设计 |

## 技术架构

```
┌─────────────────────────────────────────────────────────────┐
│                        AI 决策层（"心智"）                           │
│                                                               │
│   ┌───────────────────────────────┐     ┌──────────────────┐  │
│   │   Cognitive 模式 (内置 LLM)     │     │  Claw 模式       │  │
│   │   • 默认运行模式               │     │  • 外置 LLM       │  │
│   │   • 三魂架构 (Actor+Translator+Reflector) │ │  • OpenClaw 调度   │  │
│   │   • multi-Intent Pipeline   │     │  • 复杂推理       │  │
│   │   • 分级审核 (Always/Adaptive/Skip)  │  │                  │  │
│   └───────────────────────────────┘     └──────────────────┘  │
└─────────────────────────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│                    Agent ("躯体")                       │
│                                                               │
│   ┌─────────────────────────────────────────────────────┐   │
│   │         AgentBuilder (统一接口)                        │   │
│   │                                                      │   │
│   │   ReviewStore ← ReflectorSoul (反思之魂)              │   │
│   │   │   ├── Validator (分层审查)                       │   │
│   │   │   └── NarrativeGenerator (叙事生成)              │   │
│   │   MemoryManager ← 三层记忆系统                         │   │
│   │   ImmediateEventHandler ← 即时事件处理                 │   │
│   │   IntentTranslator ← 天魂翻译                          │   │
│   └─────────────────────────────────────────────────────┘   │
│                                                               │
│   ┌─────────────────────────────────────────────────────┐   │
│   │              HTTP API Server                          │   │
│   │                                                      │   │
│   │   GET /api/v1/state      - 状态查询                   │   │
│   │   GET /api/v1/context   - 上下文 (LLM Prompt)         │   │
│   │   POST /api/v1/character/* - 角色管理                │   │
│   │   GET/POST /api/v1/review/* - 审查系统                │   │
│   └─────────────────────────────────────────────────────┘   │
│                                                               │
│   ┌─────────────────────────────────────────────────────┐   │
│   │              WebSocket Server                          │   │
│   │                                                      │   │
│   │   ◄── OpenClaw (Claw 模式)                          │   │
│   │   ──► Server (WorldState 推送, Intent 提交)         │   │
│   └─────────────────────────────────────────────────────┘   │
│                                                               │
└─────────────────────────────────────────────────────────────┘
                            │
                            ▼ WebSocket
┌─────────────────────────────────────────────────────────────┐
│                  Server ("天道")                             │
│  ┌──────────┬───────────┬──────────┐                      │
│  │ HTTP API │ WebSocket │Tick Engine│                      │
│  └──────────┴───────────┴──────────┘                      │
│  ┌──────────────────────────────────────┐                   │
│  │    Game State / Actions / Dialogue    │                   │
│  └──────────────────────────────────────┘                   │
│  ┌──────────────────────────────────────┐                   │
│  │         PostgreSQL Database           │                   │
│  └──────────────────────────────────────┘                   │
└─────────────────────────────────────────────────────────────┘
```

## 更新日志

查看 CHANGELOG.md 了解版本历史和变更记录。

## 许可证

MIT OR Apache-2.0
