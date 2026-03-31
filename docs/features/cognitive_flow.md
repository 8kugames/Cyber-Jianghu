# Cognitive 模式数据流向架构

> **文档版本**: 2026-03-26
> **状态**: 已复核

本文档梳理 Agent Cognitive 模式的完整数据流向架构，包括输入输出、WebSocket 通信、存储系统等关键节点。

## 实现状态摘要

### 核心结论

- **双 Soul 架构已落地**：ActorSoul（生成/发送 Intent）+ ReflectorSoul（审查 Intent），单进程内通过 `ReviewStore` 共享内存通信。
- **远程 Observer 模式已移除**：只保留进程内双 Soul，减少部署复杂度与一致性风险。
- **主通道明确**：Intent 提交 **只能** 走 WebSocket；HTTP API 仅做辅助查询/管理/审查/面板。
- **硬约束**：`POST /api/v1/intent` **已禁用**（强制 WebSocket，避免 tick 同步问题）。

### 运行时模式对比

| 特性               | Cognitive 模式              | Claw 模式                   |
| ---------------- | ------------------------- | ------------------------- |
| LLM 位置           | 内置（Agent 内）               | 外置（OpenClaw）              |
| WebSocket        | ✅ 必须（tick 同步 + Intent 提交） | ✅ 必须（tick 同步 + Intent 提交） |
| HTTP API         | ✅ 辅助功能                    | ✅ 辅助功能                    |
| 托梦注入             | ✅                         | ✅                         |
| ReflectorSoul 审查 | ✅ 默认启用                    | ✅ 默认启用                    |
| 适用场景             | 独立运行、低延迟                  | 外部大脑编排、复杂推理               |

### 最近一次验证（截至 2026-03-26）

| 验证项    | 命令                                                      | 结果          |
| ------ | ------------------------------------------------------- | ----------- |
| Build  | `cargo build -p cyber-jianghu-agent`                    | ✅           |
| Tests  | `cargo test --workspace`                                | ✅           |
| Clippy | `cargo clippy --workspace --all-targets -- -D warnings` | ✅（二进制无新增问题） |

### Web 控制语义（强约束）

- 禁止：通过 HTTP 提交 Intent（无法保证 tick 同步，必然引入“过期 tick\_id”类脏数据）
- 允许：通过“托梦”注入语义（DreamState → 注入 LLM Context → 由 ActorSoul 自主决策）

### HTTP API 与 Web 面板（关键点）

- HTTP API（辅助）：`GET /api/v1`、`GET /api/v1/state`、`GET /api/v1/context`、`POST /api/v1/character/dream`、`GET/POST /api/v1/review/*`、`POST /api/v1/validate`
- HTTP Intent：`POST /api/v1/intent` 已禁用（强制 WebSocket）
- Web 面板：`/index.html`（创建角色）、`/character.html`（角色信息）、`/manage.html`（托梦/转生），资源目录 `crates/agent/src/static/panel/`

## 架构总览

```
┌─────────────────────────────────────────────────────────────────────────────────────────┐
│                                    AGENT (Cognitive Mode)                               │
│                                         "众生" / 意识层                                  │
├─────────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                         │
│  ┌─────────────────────────────────────────────────────────────────────────────────┐   │
│  │                              CONFIG LAYER (配置层)                               │   │
│  │  ~/.cyber-jianghu/agent.yaml                                                    │   │
│  │  ┌──────────────┐ ┌──────────────┐ ┌──────────────┐ ┌──────────────┐            │   │
│  │  │ Identity     │ │ ServerConfig │ │ LlmConfig    │ │ CharacterCfg │            │   │
│  │  │ (device_id)  │ │ (ws/http URL)│ │ (provider)   │ │ (name/persona)│           │   │
│  │  └──────────────┘ └──────────────┘ └──────────────┘ └──────────────┘            │   │
│  └─────────────────────────────────────────────────────────────────────────────────┘   │
│                                          │                                              │
│                                          ▼                                              │
│  ┌─────────────────────────────────────────────────────────────────────────────────┐   │
│  │                           TRANSPORT LAYER (传输层)                               │   │
│  │  infra/transport/websocket.rs - WebSocketClient                                       │   │
│  │  ┌──────────────────────────────────────────────────────────────────────────┐   │   │
│  │  │  WebSocket ──────────────────────────────────────────────────────────▶   │   │   │
│  │  │  ┌────────────────┐                      ┌────────────────┐              │   │   │
│  │  │  │ ServerMessage  │                      │ ClientMessage  │              │   │   │
│  │  │  │ • WorldState   │                      │ • Intent       │              │   │   │
│  │  │  │ • Registered   │                      │ • Dialogue     │              │   │   │
│  │  │  │ • GameRules    │                      └────────────────┘              │   │   │
│  │  │  │ • Dialogue     │                                                      │   │   │
│  │  │  └────────────────┘                                                      │   │   │
│  │  └──────────────────────────────────────────────────────────────────────────┘   │   │
│  └─────────────────────────────────────────────────────────────────────────────────┘   │
│                                          │                                              │
│                     WorldState ◀─────────┼──────────────▶ Intent                       │
│                                          ▼                                              │
│  ┌─────────────────────────────────────────────────────────────────────────────────┐   │
│  │                            CORE LAYER (核心层)                                   │   │
│  │  core/agent.rs + core/lifecycle.rs                                              │   │
│  │                                                                                  │   │
│  │  ┌─────────────────────────────────────────────────────────────────────────┐    │   │
│  │  │                          MAIN LOOP (主循环)                              │    │   │
│  │  │                                                                          │    │   │
│  │  │  1.  receive_world_state() ──▶ WorldState                               │    │   │
│  │  │  1.5 [死亡检查] death_reported 检测死亡事件                              │    │   │
│  │  │  2.  process_events() ───────▶ MemoryManager.process_events()           │    │   │
│  │  │  3.  run_forgetting() ───────▶ MemoryManager.run_forgetting() [每84tick]│    │   │
│  │  │  4.  get_memory_context() ───▶ MemoryManager.build_llm_context()        │    │   │
│  │  │  5.  decide_with_validation() ───────────────────────────────┐          │    │   │
│  │  │  │                                                           ▼          │    │   │
│  │  │  │                                            ┌───────────────────┐   │    │   │
│  │  │  │                                            │ decision_callback │   │    │   │
│  │  │  │                                            │ (Cognitive)       │   │    │   │
│  │  │  │                                            └───────────────────┘   │    │   │
│  │  │  5.5 [审查] submit_for_review() ──▶ ReviewStore (ReflectorSoul)        │    │   │
│  │  │  6.  send_intent() ───────────────────────────────────────────▶ Intent │    │   │
│  │  │  6.5 [寿命] LifespanCalculator.process_tick() 检查寿命状态              │    │   │
│  │  └─────────────────────────────────────────────────────────────────────────┘    │   │
│  └─────────────────────────────────────────────────────────────────────────────────┘   │
│                                          │                                              │
│                                          ▼                                              │
│  ┌─────────────────────────────────────────────────────────────────────────────────┐   │
│  │                         COGNITIVE ENGINE (认知引擎)                              │   │
│  │  soul/actor/                                                                 │   │
│  │  ├── engine.rs      - MultiStageCognitiveEngine (主引擎)                        │   │
│  │  ├── chain.rs       - CognitiveChain (认知链)                                   │   │
│  │  ├── stages.rs      - StageOutput, 各阶段响应类型                               │   │
│  │  └── pipeline.rs    - CognitivePipeline (流程编排)                              │   │
│  │                                                                                  │   │
│  │  ┌─────────────────────────────────────────────────────────────────────────┐    │   │
│  │  │                    think(WorldState) -> CognitiveChain                   │    │   │
│  │  │                                                                          │    │   │
│  │  │  ┌──────────────┐   ┌──────────────┐   ┌──────────────┐   ┌──────────┐ │    │   │
│  │  │  │  Perception  │──▶│  Motivation  │──▶│   Planning   │──▶│ Decision │ │    │   │
│  │  │  │   (感知)     │   │   (动机)     │   │   (规划)     │   │  (决策)  │ │    │   │
│  │  │  └──────────────┘   └──────────────┘   └──────────────┘   └──────────┘ │    │   │
│  │  │        │                  │                  │                 │       │    │   │
│  │  │        ▼                  ▼                  ▼                 ▼       │    │   │
│  │  │  ┌──────────────────────────────────────────────────────────────────┐ │    │   │
│  │  │  │                    LLM Prompt Building                            │ │    │   │
│  │  │  │  • NarrativeEngine (叙事化属性描述)                               │ │    │   │
│  │  │  │  • DynamicPersona (人设生成描述)                                  │ │    │   │
│  │  │  │  • Memory Context (记忆上下文注入)                               │ │    │   │
│  │  │  └──────────────────────────────────────────────────────────────────┘ │    │   │
│  │  │                                    │                                   │    │   │
│  │  │                                    ▼                                   │    │   │
│  │  │                            ┌───────────────┐                          │    │   │
│  │  │                            │   LlmClient   │                          │    │   │
│  │  │                            │ complete_json │                          │    │   │
│  │  │                            └───────────────┘                          │    │   │
│  │  └─────────────────────────────────────────────────────────────────────────┘    │   │
│  └─────────────────────────────────────────────────────────────────────────────────┘   │
│                                                                                         │
├─────────────────────────────────────────────────────────────────────────────────────────┤
│                                    AI COMPONENTS                                        │
├─────────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                         │
│  ┌──────────────────┐  ┌──────────────────┐  ┌──────────────────┐  ┌────────────────┐  │
│  │    LlmClient     │  │  DynamicPersona  │  │  MemoryManager   │  │ NarrativeEngine│  │
│  │  (component/llm/)       │  │  (component/persona/)   │  │  (component/memory/)    │  │ (soul/actor/)│  │
│  │                  │  │                  │  │                  │  │                │  │
│  │ • DirectLlmClient│  │ • traits         │  │ • WorkingMemory  │  │ • Perception   │  │
│  │ • LlmProvider    │  │ • current_state  │  │ • EpisodicMemory │  │   Narrative    │  │
│  │   - ollama       │  │ • version        │  │ • SemanticMemory │  │                │  │
│  │   - openclaw     │  │                  │  │ • ArchiveMemory  │  │                │  │
│  │   - openai_      │  │ generate_        │  │                  │  │ from_attributes│  │
│  │     compatible   │  │   description()  │  │ build_llm_       │  │ _with_engine() │  │
│  │                  │  │                  │  │   context()      │  │                │  │
│  └──────────────────┘  └──────────────────┘  └──────────────────┘  └────────────────┘  │
│           │                     │                     │                    │           │
│           └─────────────────────┴─────────────────────┴────────────────────┘           │
│                                         │                                               │
│                                         ▼                                               │
│  ┌─────────────────────────────────────────────────────────────────────────────────┐   │
│  │                           STORAGE LAYER (存储层)                                 │   │
│  │                                                                                  │   │
│  │  ┌──────────────────────────────────────────────────────────────────────────┐   │   │
│  │  │  ~/.cyber-jianghu/data/                                                   │   │   │
│  │  │  ├── episodic.db     (情景记忆 SQLite)                                    │   │   │
│  │  │  ├── semantic.db     (语义记忆 SQLite + HNSW 向量索引)                    │   │   │
│  │  │  └── archive.db      (归档记忆 SQLite)                                    │   │   │
│  │  └──────────────────────────────────────────────────────────────────────────┘   │   │
│  │                                                                                  │   │
│  │  ┌──────────────────────────────────────────────────────────────────────────┐   │   │
│  │  │  ~/.cyber-jianghu/config/                                                 │   │   │
│  │  │  └── narrative_config.yaml   (属性叙事配置，从 Server 获取)               │   │   │
│  │  └──────────────────────────────────────────────────────────────────────────┘   │   │
│  │                                                                                  │   │
│  │  ┌──────────────────────────────────────────────────────────────────────────┐   │   │
│  │  │  ~/.openclaw/                                                             │   │   │
│  │  │  └── openclaw.json   (OpenClaw Provider 配置，仅 openclaw provider 使用) │   │   │
│  │  └──────────────────────────────────────────────────────────────────────────┘   │   │
│  └─────────────────────────────────────────────────────────────────────────────────┘   │
│                                                                                         │
└─────────────────────────────────────────────────────────────────────────────────────────┘
                                          │
                                          │ WebSocket (开发环境: ws://, 生产环境: wss://)
                                          ▼
┌─────────────────────────────────────────────────────────────────────────────────────────┐
│                                    GAME SERVER                                          │
│                                         "天道" / 物理引擎                               │
│                                                                                         │
│  ws://localhost:23333/ws (开发默认)                                                     │
│                                                                                         │
│  Tick Loop (60s): 意图收集 → 验证 → 冲突解析 → 执行 → 状态更新 → 广播                   │
│                                                                                         │
└─────────────────────────────────────────────────────────────────────────────────────────┘
```

***

## 核心数据流

### 1. 输入数据流 (Input Flow)

| 来源                   | 数据结构                 | 处理模块                     | 用途     |
| -------------------- | -------------------- | ------------------------ | ------ |
| **Server WebSocket** | `WorldState`         | `infra/transport/websocket.rs` | 世界快照   |
| **Server WebSocket** | `GameRules`          | `infra/transport/websocket.rs` | 游戏规则更新 |
| **Server WebSocket** | `DialogueMessage`    | `component/social/dialogue.rs`           | 对话消息   |
| **Server WebSocket** | `WorldBuildingRules` | `soul/reflector/`          | 世界观约束  |
| **本地事件**             | `WorldEvent[]`       | `component/memory/manager.rs`   | 记忆系统输入 |

### 2. 认知处理流 (Cognitive Pipeline)

```
WorldState ──┬──▶ NarrativeEngine.from_attributes() ──▶ 叙事化状态描述
             │
             ├──▶ DynamicPersona.generate_description() ──▶ 人设描述
             │
             └──▶ MemoryManager.build_llm_context() ──▶ 记忆上下文
                              │
                              ▼
                    ┌─────────────────────┐
                    │  Stage 1: Perception │  LLM: "我看到了什么？"
                    └─────────────────────┘
                              │
                              ▼
                    ┌─────────────────────┐
                    │ Stage 2: Motivation │  LLM: "我想要什么？"
                    └─────────────────────┘
                              │
                              ▼
                    ┌─────────────────────┐
                    │  Stage 3: Planning  │  LLM: "我该怎么做？"
                    └─────────────────────┘
                              │
                              ▼
                    ┌─────────────────────┐
                    │ Stage 4: Decision   │  LLM: "最终决定？"
                    └─────────────────────┘
                              │
                              ▼
                         Intent
```

### 3. 输出数据流 (Output Flow)

| 目标                   | 数据结构                   | 来源模块                             | 用途   |
| -------------------- | ---------------------- | -------------------------------- | ---- |
| **Server WebSocket** | `Intent`               | `infra/transport/websocket.rs`         | 提交决策 |
| **Episodic Memory**  | `MemoryEntry`          | `component/memory/backends/episodic.rs` | 事件记忆 |
| **Semantic Memory**  | `MemoryEntry + Vector` | `component/memory/backends/semantic/`   | 语义索引 |
| **Archive Memory**   | `MemoryEntry`          | `component/memory/backends/archive.rs`  | 遗忘归档 |

***

## 主循环详细流程

### 完整步骤

```
loop {
    tokio::select! {
        // 重连请求 (热切换)
        Some(req) = reconnect_rx.recv() => {
            更新服务器 URL
            触发重连
        }

        // 配置热重载
        _ = config_reload_rx.recv() => {
            重新加载 LLM 配置
            更新 ActorSoul LLM Client 容器（RwLock）
            决策回调自动使用新配置
        }

        // 主流程
        result = client.receive_world_state() => {
            1.  接收 WorldState
            1.5 [死亡检查] 检测 events_log 中的死亡事件
                - 若死亡且未报告: 记录日志, 设置 death_reported = true
            2.  处理事件: process_events(&world_state.events_log)
            3.  每 84 tick 运行遗忘机制: run_forgetting(tick_id)
            4.  构建记忆上下文: get_memory_context()
            5.  决策 (带验证器):
                - 若有 validator: decide_with_validation()
                - 若有 memory_callback: decision_with_memory_callback()
                - 否则: decision_callback()
            5.5 [审查] 若有 review_store: submit_for_review()
                - 等待 ReflectorSoul 审查结果
                - 超时则使用原始 Intent
            6.  发送 Intent: send_intent()
            6.5 [寿命] LifespanCalculator.process_tick()
                - 若已故: 发送最后的 idle Intent 并退出
        }
    }
}
```

### 步骤说明

| 步骤  | 说明                        | 状态          |
| --- | ------------------------- | ----------- |
| 1   | 接收世界状态                    | 已实现         |
| 1.5 | 死亡检查 (death\_reported 标志) | 已实现         |
| 2   | 事件处理并更新记忆                 | 已实现         |
| 3   | 遗忘机制 (每 84 tick)          | 已实现         |
| 4   | 构建记忆上下文                   | 已实现         |
| 5   | 认知决策 (带验证器)               | 已实现         |
| 5.5 | ReflectorSoul 审查          | 已实现         |
| 6   | 发送 Intent                 | 已实现         |
| 6.5 | 寿命处理                      | 已实现         |
| -   | 配置热重载                     | **已实现** 使用 `RwLock` 容器包装 LLM Client，决策回调每次从容器读取最新 Client |

***

## 错误处理与重连机制

### WebSocket 重连策略

**位置**: `core/lifecycle.rs` → `reconnect()`

```
重连策略: 指数退避 (Exponential Backoff)

初始延迟: 1 秒
延迟计算: min(1s * 2^backoff, tick_duration / 2)
最大延迟: tick_duration 的一半 (确保每 tick 至少尝试 2 次)

日志采样策略:
- 前 5 次: 每次都记录
- 第 6 次后: 仅当重试次数为完全平方数时记录 (9, 16, 25, 36...)
```

### 连接失败处理

| 场景      | 处理方式             |
| ------- | ---------------- |
| 初始连接失败  | 无限重试，5 秒间隔       |
| 运行中断开   | 触发 `reconnect()` |
| 重连后注册失败 | 继续重试，增加退避计数      |
| 重连成功    | 重置退避计数器          |

### 日志采样

为避免日志爆炸，采用完全平方数采样策略：

```rust
fn should_log_retry(attempt: u32) -> bool {
    if attempt <= 5 { return true; }
    let sqrt = (attempt as f64).sqrt() as u32;
    sqrt * sqrt == attempt  // 9, 16, 25, 36...
}
```

***

## Intent 验证流程

### Validator 组件

**位置**: `soul/reflector/`

| 组件                   | 说明                                     |
| -------------------- | -------------------------------------- |
| `Validator` Trait    | 验证器接口                                  |
| `CognitiveValidator` | 基于 LLM 的认知验证器                          |
| `RuleEngine`         | 规则引擎验证                                 |
| `ValidationRequest`  | 验证请求 (intent, persona, world\_context) |
| `ValidationResult`   | 验证结果 (Approved/Rejected)               |

### 验证配置

```yaml
# agent.yaml
validator:
  max_retry_attempts: 5           # 最大重试次数
  min_retry_time_secs: 10         # 最小重试时间
  consecutive_rejection_threshold: 3  # 连续驳回阈值
```

### 验证流程

```
Intent 生成
    │
    ▼
┌─────────────────┐
│   Validator     │
│   .validate()   │
└────────┬────────┘
         │
    ┌────┴────┐
    ▼         ▼
Approved   Rejected
    │         │
    │         ▼
    │    记录驳回原因
    │         │
    │         ▼
    │    decision_with_feedback_callback()
    │         │
    │         ▼
    │    重新生成 Intent (最多 max_retry_attempts 次)
    │         │
    │    ┌────┴────┐
    │    ▼         ▼
    │ Approved   连续驳回 >= threshold
    │    │         │
    │    │         ▼
    │    │    强制 idle
    │    │         │
    └────┴─────────┘
              │
              ▼
         最终 Intent
```

***

## ReflectorSoul 审查系统

### 架构

```
┌─────────────────┐                    ┌─────────────────┐
│   ActorSoul     │                    │  ReflectorSoul  │
│   (玩家 Agent)   │                    │  (观察者 Agent)  │
├─────────────────┤                    ├─────────────────┤
│                 │                    │                 │
│ 1. 生成 Intent  │                    │ 1. 轮询待审查队列│
│                 │                    │                 │
│ 2. 提交审查     │──▶ ReviewStore ──▶│ 2. LLM 审查     │
│    add_pending()│                    │    (世界观一致性)│
│                 │                    │                 │
│ 3. 等待结果     │◀── ReviewStore ◀──│ 3. 提交结果     │
│    (带超时)     │                    │    (Approved/   │
│                 │                    │     Rejected)   │
└─────────────────┘                    └─────────────────┘
```

### 审查配置

```yaml
# agent.yaml
review:
  enabled: true
  timeout_seconds: 30      # 审查超时
  auth_token: "xxx"        # 审查认证 Token
```

### 审查结果处理

| 结果                | 处理                    |
| ----------------- | --------------------- |
| `Approved`        | 使用原始 Intent           |
| `Rejected`        | 返回 idle Intent + 驳回原因 |
| `TimeoutApproved` | 超时后自动通过               |
| `Pending`         | 继续等待 (每 100ms 检查一次)   |

***

## 关键节点说明

### 1. 配置层 (`config.rs`)

**路径**: `crates/agent/src/config.rs`

| 组件                | YAML Key              | 说明     | 默认值                       |
| ----------------- | --------------------- | ------ | ------------------------- |
| `IdentityConfig`  | `identity`            | 设备身份   | 首次启动生成                    |
| `ServerConfig`    | `server`              | 服务器连接  | `ws://localhost:23333/ws` |
| `LlmConfig`       | `llm`                 | LLM 配置 | `provider: ollama`        |
| `CharacterConfig` | `agent` / `character` | 当前角色   | 无                         |
| `MemoryConfig`    | `memory`              | 记忆系统   | `enabled: true`           |
| `ReviewConfig`    | `review`              | 审查配置   | `enabled: true`           |
| `RuntimeConfig`   | `runtime`             | 运行时模式  | `mode: cognitive`         |

### 2. 传输层 (`infra/transport/websocket.rs`)

**路径**: `crates/agent/src/infra/transport/websocket.rs`

| 组件                | 说明                        |
| ----------------- | ------------------------- |
| `WebSocketClient` | 纯 I/O 层，负责 WebSocket 连接管理 |
| `AgentClient`     | 异步包装器，提供线程安全访问            |
| `ServerMessage`   | 服务端下行消息枚举                 |
| `ClientMessage`   | 客户端上行消息枚举                 |

**关键方法**:

- `connect()`: 建立 WebSocket 连接
- `receive_world_state()`: 阻塞等待世界状态
- `send_intent()`: 发送意图到服务端
- `wait_for_registration()`: 等待注册确认
- `set_game_rules_callback()`: 设置游戏规则更新回调
- `set_server_msg_callback()`: 设置消息透传回调 (OpenClaw 集成)

### 3. 核心层 (`core/agent.rs` + `core/lifecycle.rs`)

**路径**: `crates/agent/src/core/`

| 组件                         | 说明                    |
| -------------------------- | --------------------- |
| `Agent`                    | 运行时主结构，持有所有组件引用       |
| `run()`                    | 主循环：接收 → 处理 → 决策 → 发送 |
| `reconnect()`              | 重连机制，指数退避策略           |
| `decide_with_validation()` | 带验证器的决策流程             |
| `submit_for_review()`      | 提交 ReflectorSoul 审查   |

### 4. 认知引擎 (`soul/actor/`)

**路径**: `crates/agent/src/soul/actor/`

| 文件            | 组件                          | 说明          |
| ------------- | --------------------------- | ----------- |
| `engine.rs`   | `MultiStageCognitiveEngine` | 四阶段认知流程引擎   |
| `chain.rs`    | `CognitiveChain`            | 存储各阶段输出的认知链 |
| `stages.rs`   | `StageOutput`, `*Response`  | 各阶段响应类型定义   |
| `pipeline.rs` | `CognitivePipeline`         | 认知流程编排器     |

**四阶段流程**:

1. **Perception (感知)**: 理解当前世界状态
2. **Motivation (动机)**: 基于人设生成内在驱动力
3. **Planning (规划)**: 制定行动计划
4. **Decision (决策)**: 选择最终行动并输出 Intent

**关键方法**:

- `think(&WorldState) -> CognitiveChain`: 执行完整认知流程
- `think_with_memory(&WorldState, &str) -> CognitiveChain`: 带记忆上下文
- `create_decision_callback()`: 创建决策回调 (兼容 Agent 接口)

### 5. 记忆系统 (`component/memory/manager.rs`)

**路径**: `crates/agent/src/component/memory/manager.rs`

| 后端                      | 存储            | 用途   | 容量   | 配置项                   |
| ----------------------- | ------------- | ---- | ---- | --------------------- |
| `WorkingMemoryBackend`  | RAM FIFO      | 最近事件 | 20 条 | `working_memory_size` |
| `EpisodicMemoryBackend` | SQLite        | 情景记忆 | 无限   | `episodic_threshold`  |
| `SemanticMemoryBackend` | SQLite + HNSW | 语义检索 | 无限   | -                     |
| `ArchiveMemoryBackend`  | SQLite        | 遗忘归档 | 无限   | -                     |

**关键方法**:

- `process_events(&[WorldEvent])`: 处理事件到记忆系统
- `run_forgetting(tick_id)`: 执行遗忘机制 (艾宾浩斯曲线)
- `build_llm_context()`: 构建 LLM 上下文字符串
- `recall_archived(query, limit)`: 搜索归档记忆 ("努力回忆")

### 6. 人设系统 (`component/persona/dynamic_persona.rs`)

**路径**: `crates/agent/src/component/persona/dynamic_persona.rs`

| 组件                  | 说明              |
| ------------------- | --------------- |
| `DynamicPersona`    | 运行时可修改的人设系统     |
| `Trait`             | 单个特质 (值 + 历史变化) |
| `PersonaState`      | 当前状态 (情绪、目标、压力) |
| `ThreadSafePersona` | 线程安全包装器         |

**关键方法**:

- `generate_description() -> String`: 生成叙事化人设描述
- `apply_trait_change(name, delta, reason, tick_id)`: 应用特质变化
- `update_emotion(emotion)`: 更新情绪状态
- `set_goal(goal)`: 设置当前目标
- `apply_all_decay()`: 应用所有特质衰减

### 7. 叙事引擎 (`soul/actor/narrative.rs`)

**路径**: `crates/agent/src/soul/actor/narrative.rs`

| 组件                     | 说明           |
| ---------------------- | ------------ |
| `NarrativeEngine`      | 数据驱动的属性叙事化引擎 |
| `PerceptionNarrative`  | 感知阶段叙事结构     |
| `ThresholdDescription` | 阈值描述配置       |

**关键方法**:

- `from_attributes_with_engine()`: 将数值属性转换为叙事描述
- `to_prompt_section()`: 生成 LLM Prompt 片段

***

## 存储路径

| 路径                                              | 用途                                            |
| ----------------------------------------------- | --------------------------------------------- |
| `~/.cyber-jianghu/agent.yaml`                   | Agent 配置文件                                    |
| `~/.cyber-jianghu/data/episodic.db`             | 情景记忆                                          |
| `~/.cyber-jianghu/data/semantic.db`             | 语义记忆 + 向量索引                                   |
| `~/.cyber-jianghu/data/archive.db`              | 归档记忆                                          |
| `~/.cyber-jianghu/config/narrative_config.yaml` | 叙事配置 (从 Server 获取)                            |
| `~/.openclaw/openclaw.json`                     | OpenClaw Gateway 配置（仅读取 `base_url`/`model`，不含 API Key） |

***

## LLM Provider 支持

| Provider           | 配置值                 | 说明               | 配置要求                                      |
| ------------------ | ------------------- | ---------------- | ----------------------------------------- |
| `Ollama`           | `ollama`            | 本地部署 (默认)        | 无需 API Key，默认 `http://localhost:11434/v1` |
| `OpenClaw`         | `openclaw`          | OpenClaw Gateway | 仅读取 `~/.openclaw/openclaw.json` 的 `base_url`/`model`，**API Key 需手动输入** |
| `OpenAICompatible` | `openai_compatible` | 兼容 OpenAI API    | **必须**设置 `base_url` + `model` + `api_key` |

### OpenClaw Provider 说明

当用户选择 `openclaw` provider 时：

1. **配置文件检查**：Provider 列表接口仅检查 `~/.openclaw/openclaw.json` 是否存在
2. **自动禁选**：如果配置文件不存在，前端显示禁选状态并提示安装 OpenClaw Gateway
3. **延迟读取**：仅当用户选择 `openclaw` 时，前端调用 `GET /api/v1/config/llm/providers/openclaw/defaults` 读取配置
4. **仅读取 `gateway_url`**：从配置文件读取 `gateway.url` 字段
5. **不读取 `api_key`**：出于安全考虑，API Key 必须由用户手动在 `agent.yaml` 中配置
6. **只读操作**：不会修改 `~/.openclaw/openclaw.json` 文件
7. **配置落存**：读取的 `base_url` 会保存到 `~/.cyber-jianghu/agent.yaml`，下次启动无需重新配置

**API 调用流程**：
```
前端加载 Provider 列表
    ↓
GET /api/v1/config/llm/providers
    → 检查 ~/.openclaw/openclaw.json 是否存在
    → 如果不存在：{ value: "openclaw", disabled: true, disabled_reason: "OpenClaw 不存在" }
    → 如果存在：{ value: "openclaw", disabled: false }
    → 不读取配置内容
    ↓
用户选择 openclaw provider (仅当 disabled=false 时可选)
    ↓
GET /api/v1/config/llm/providers/openclaw/defaults
    → 此时才读取 ~/.openclaw/openclaw.json
    → 返回 { base_url: "...", model: null }
    ↓
用户手动输入 API Key
    ↓
POST /api/v1/config/llm 保存到 ~/.cyber-jianghu/agent.yaml
```

### 第三方服务配置示例

通过 `openai_compatible` 连接 OpenAI/DeepSeek/Anthropic 等：

```yaml
# DeepSeek 示例
llm:
  provider: "openai_compatible"
  base_url: "https://api.deepseek.com/v1"
  api_key: "sk-xxx"
  model: "deepseek-chat"
  temperature: 0.7
  max_tokens: 4096
```

### LLM 配置项

| YAML Key          | 类型     | 默认值      | 说明          |
| ----------------- | ------ | -------- | ----------- |
| `llm.provider`    | string | `ollama` | LLM 提供者     |
| `llm.base_url`    | string | -        | 自定义 API URL |
| `llm.api_key`     | string | -        | API 密钥      |
| `llm.model`       | string | -        | 模型名称        |
| `llm.temperature` | float  | `0.7`    | 温度参数        |
| `llm.max_tokens`  | int    | `4096`   | 最大 Token 数  |

***

## 与 Claw 模式的区别

| 特性        | Cognitive 模式                | Claw 模式                         |
| --------- | --------------------------- | ------------------------------- |
| LLM 调用位置  | Agent 内置                    | 外部调度器 (OpenClaw)                |
| 决策方式      | 多阶段认知引擎                     | WebSocket 推送 Tick，外部回调提交 Intent |
| WebSocket | **必须**（tick 同步 + Intent 提交） | **必须**（tick 同步 + Intent 提交）     |
| HTTP API  | 辅助查询/管理（状态、记忆、关系、托梦、审查、面板）  | 辅助查询/管理（同 Cognitive）            |
| 适用场景      | 独立运行，内置智能                   | 集成外部 LLM 调度系统                   |

> **重要**: 两种模式下，Intent 提交都**必须**通过 WebSocket；HTTP `POST /api/v1/intent` 已禁用。

***

## 相关文档

- [Agent Quick Start](../../crates/agent/QuickStart-Agent.md) - Agent 快速启动指南
- [Summary](./summary.md) - 功能概述
