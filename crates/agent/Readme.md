# Cyber-Jianghu Agent SDK

Agent SDK 是连接虚境：江湖服务端的桥梁。它为开发者提供了与游戏世界交互的基础设施，并且内置了记忆、认知、对话等高级 AI 模块，支持两种运行模式：

- **Cognitive 模式（默认）**：内置 LLM 客户端，使用 CognitiveEngine 自主决策
- **Claw 模式**：通过 OpenClawBridge 桥接外部 LLM，使用相同的 CognitiveEngine 架构

## 核心设计原则

### COI (Composition Over Inheritance)

Agent 采用组合优于继承的设计，通过 `AgentBuilder` 灵活组合各种功能：

```rust
let agent = AgentBuilder::new(config, decision)
    .with_review_store(review_store)      // ReflectorSoul 审查
    .with_memory_manager(memory_manager)    // 三层记忆系统
    .with_validator(validator)             // 意图验证
    .with_reconnect_rx(reconnect_rx)       // Claw 热切换
    .build();
```

### ActorSoul + ReflectorSoul 架构

- **ActorSoul (人魂/行动之魂)**：直连 WorldState，输出含精确 ID 的结构化 Intent
- **地魂 (能力之魂)**：提供 tool calling 工具池，行动落地层（嵌入 ActorSoul）
- **ReflectorSoul (天魂/守护之魂)**：三层审查，世界观一致性审查
- **共享内存通信**：通过 `ReviewStore` 进行进程内通信

### 分级审核策略

| 策略 | 说明 | 适用场景 |
|------|------|---------|
| Always | 完整三层审核 | speak/shout/whisper 等高优先级动作 |
| Adaptive | 动态判断是否需要 LLM | steal/trade/give/move 等风险动作 |
| Skip | 仅 RuleEngine 校验 | idle/wait 等低风险动作 |

### multi-Intent Pipeline

单 tick 可提交多 Intent，顺序执行，失败回滚：
- `max_intents_per_tick`: 每 tick 最大 Intent 数（默认 5）
- `max_retries`: 三魂循环最大重试次数（默认 3）

### 两种运行模式

> **架构统一**: 两种模式共享 CognitiveEngine、OutcomeMemory、ChaosGenerator、回调注册，仅 LLM 客户端实现不同。

| 特性 | Cognitive 模式 | Claw 模式 |
|------|---------------|-----------|
| LLM 位置 | **内置** (Agent 内部) | **外置** (OpenClaw) |
| 认知引擎 | CognitiveEngine（直连） | CognitiveEngine（via OpenClawBridge） |
| ReflectorSoul | ✅ 默认启用 | ✅ 默认启用 |
| OutcomeMemory | ✅ 已初始化 | ✅ 已初始化 |
| ChaosGenerator | ✅ 已初始化 | ✅ 已初始化 |
| HTTP API | ✅ 完整支持（含 /api/v1/context enrichment） | ✅ 完整支持（含 /api/v1/context enrichment） |
| 适用场景 | 独立运行、低延迟 | 复杂推理、外部大脑 |

## 快速开始

```bash
# 安装
cargo install --path crates/agent

# Cognitive 模式（默认，ReflectorSoul 内置启用）
cyber-jianghu-agent run

# Claw 模式
cyber-jianghu-agent run --mode claw
```

## 架构文档

详见 `docs/architecture/`

### P0 核心

| 文档 | 说明 |
|------|------|
| [three_soul.md](docs/architecture/p0_core/three_soul.md) | 三魂架构 |
| [cognitive_engine.md](docs/architecture/p0_core/cognitive_engine.md) | 认知流转引擎 |
| [memory_system.md](docs/architecture/p0_core/memory_system.md) | 三级记忆系统 |
| [dual_mode.md](docs/architecture/p0_core/dual_mode.md) | 双栖运行模式 |

### P1 重要特性

| 文档 | 说明 |
|------|------|
| [model_gateway.md](docs/architecture/p1_major/model_gateway.md) | 模型网关与调度 |
| [outcome_memory.md](docs/architecture/p1_major/outcome_memory.md) | 经验结果记忆 (Hermes) |
| [dynamic_persona.md](docs/architecture/p1_major/dynamic_persona.md) | 动态角色演化 |

### P2 体验增强

| 文档 | 说明 |
|------|------|
| [session_triage.md](docs/architecture/p2_enhancement/session_triage.md) | 异步即时事件引擎 |
| [relationship_store.md](docs/architecture/p2_enhancement/relationship_store.md) | 人际社交网络 |
| [agent_control_panel.md](docs/architecture/p2_enhancement/agent_control_panel.md) | 玩家控制台 |
| [cli.md](docs/architecture/p2_enhancement/cli.md) | 命令行工具 |

### WebSocket (主通道)

- `ws://localhost:23340/ws` - OpenClaw 连接（Claw 模式）

### HTTP API (辅助功能)

**核心**:
- `GET /api/v1/state` - 获取当前世界状态
- `GET /api/v1/context` - 获取叙事上下文 + DecisionContextSnapshot
- `GET /api/v1/cognitive` - 结构化认知上下文

**角色管理**:
- `GET /api/v1/character` - 角色信息
- `POST /api/v1/character/generate` - LLM 一键生成角色
- `POST /api/v1/character/register` - 注册新角色
- `POST /api/v1/character/rebirth` - 角色转世重生
- `GET /api/v1/character/soul-cycles` - 灵魂循环记录
- `GET /api/v1/character/biography` - 获取纪传体传记
- `POST /api/v1/character/biography` - LLM 生成传记
- `GET/POST /api/v1/character/dream` - 梦境注入（持续 N 轮思想注入）

**多角色**:
- `GET /api/v1/characters` - 列出所有角色
- `POST /api/v1/characters/switch` - 切换当前角色
- `GET /api/v1/characters/{agent_id}` - 按 ID 获取角色

**记忆与关系**:
- `GET /api/v1/memory/recent` - 近期记忆
- `GET /api/v1/memory/daily-summaries` - 每日摘要
- `POST /api/v1/memory/search` - 语义搜索记忆
- `GET /api/v1/relationship/list` - 所有人际关系

**属性与状态**:
- `GET /api/v1/attributes` - 属性值
- `GET /api/v1/tick` - Tick 状态
- `GET /api/v1/lifespan` - 寿命状态

**审查与验证**:
- `POST /api/v1/validate` - 验证意图
- `GET /api/v1/review/pending` - 待审查意图
- `POST /api/v1/review/{id}` - 提交审查结果

**配置**:
- `GET/POST /api/v1/config/llm` - LLM 配置
- `GET /api/v1/config/llm/providers` - 支持的 LLM 提供商
- `GET /api/v1/config/llm/usage` - Token 累计用量
- `GET/POST /api/v1/config/llm-disabled` - LLM 开关
- `GET/POST /api/v1/config/auto-rebirth` - 自动重生开关
- `POST /api/v1/config/reload` - 热重载配置

**事件流**:
- `GET /api/v1/events` - 死亡事件 SSE 流

## 许可证

MIT OR Apache-2.0
