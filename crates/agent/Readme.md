# Cyber-Jianghu Agent SDK

Agent SDK 是连接赛博江湖服务端的桥梁。它为开发者提供了与游戏世界交互的基础设施，并且内置了记忆、认知、对话等高级 AI 模块，支持两种运行模式：

- **Cognitive 模式（默认）**：内置 LLM，Agent 自主决策
- **Claw 模式**：等待外部调度器（OpenClaw）提交 Intent

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

- **ActorSoul (行动之魂)**：生成意图，执行行动，纯叙事输出
- **IntentTranslator (天魂)**：LLM 翻译叙事为格式化 Intent（精确 ID 映射）
- **ReflectorSoul (反思之魂)**：分级审查，世界观一致性审查
- **NarrativeGenerator (叙事生成器)**：LLM 生成叙事上下文，语义缓存，泄露检测
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

| 特性 | Cognitive 模式 | Claw 模式 |
|------|---------------|-----------|
| LLM 位置 | **内置** (Agent 内部) | **外置** (OpenClaw) |
| ReflectorSoul | ✅ 默认启用 | ✅ 默认启用 |
| HTTP API | ✅ 完整支持 | ✅ 完整支持 |
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

| 文档 | 说明 |
|------|------|
| 01_概述.md | 概述和设计原则 |
| 02_模块结构.md | 模块结构 |
| 03_通信协议.md | 通信协议 |
| 04_认知架构.md | 认知架构、ActorSoul + ReflectorSoul |
| 05_生命周期.md | 生命周期 |
| 06_规划.md | 规划中的功能 |

## API 端口

### WebSocket (主通道)

- `ws://localhost:23340/ws` - OpenClaw 连接（Claw 模式）

### HTTP API (辅助功能)

- `GET /api/v1/state` - 获取当前世界状态
- `GET /api/v1/context` - 获取 LLM 上下文
- `GET /api/v1/memory/recent` - 获取近期记忆
- `POST /api/v1/character/dream` - 托梦注入
- `GET /api/v1/review/pending` - 查看待审查意图
- `POST /api/v1/review/{id}` - 提交审查结果

## 许可证

MIT OR Apache-2.0
