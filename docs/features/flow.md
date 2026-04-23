# Cyber-Jianghu 架构与数据流

**日期**: 2026-04-23
**版本**: v0.5

---

## 一、系统概览

```mermaid
graph TB
    subgraph Client["客户端"]
        OpenClaw["OpenClaw<br/>外部调度器"]
        Agent["Agent SDK"]
    end

    subgraph Protocol["协议层 cyber_jianghu_protocol"]
        SM["ServerMessage"]
        CM["ClientMessage"]
    end

    subgraph Server["服务端 天道"]
        TickEngine["Tick Engine<br/>(scheduler + IntentWorker)"]
        WS["WebSocket Handler"]
        HTTP["HTTP API"]
        DB["PostgreSQL"]
    end

    OpenClaw <-->|"WS/HTTP"| WS
    Agent <-->|"WS/HTTP"| WS
    SM --> Agent
    CM --> Server
    TickEngine <--> DB
```

---

## 二、Server 内部架构

```mermaid
graph TB
    subgraph Scheduler["Scheduler (纯时钟驱动)"]
        Tick["interval.tick()"]
        Calc["calculate_tick_id()"]
        Boundary["发送 TickBoundary<br/>到 IntentWorker"]
        Broadcast["广播 WorldState<br/>(持续开单)"]
        Chronicle["群像传记<br/>(每168 tick)"]
    end

    subgraph IntentWorker["IntentWorker (实时处理 Intent)"]
        Process["process_intent()<br/>验证+执行+状态变更"]
        Decay["Decay + Death Check"]
        Persist["Persist to DB"]
        UpdateCache["Update DashMap"]
        SendResult["Send ExecutionResult"]
    end

    subgraph StateProc["StateProcessor"]
        Validate["IntentValidator"]
        Resolve["IntentResolver"]
        Execute["ActionExecutor"]
        Mutate["Mutators"]
    end

    subgraph DataLayer["Data Layer"]
        PG["PostgreSQL"]
        GameData["GameData Cache"]
    end

    Tick --> Calc
    Calc --> Boundary
    Boundary --> Process
    Process --> Validate
    Validate --> Resolve
    Resolve --> Execute
    Execute --> Mutate
    Tick --> Broadcast
    Broadcast --> Chronicle

    Decay --> Persist
    Persist --> UpdateCache
    UpdateCache --> SendResult
```

---

## 三、Agent 三魂架构

```mermaid
graph TB
    subgraph Actor["ActorSoul 人魂"]
        WorldState["WorldState<br/>直连"]
        Cognitive["CognitiveEngine"]
        Intent["结构化 Intent"]
    end

    subgraph Earth["EarthSoul 地魂 (soul/earth/)"]
        Tools["skill_view (已实现)<br/>search_memory / recall_archived (预留)"]
    end

    subgraph Reflector["ReflectorSoul 天魂"]
        Layer1["Layer1: action_type"]
        Layer2["Layer2: RuleEngine"]
        Layer3["Layer3: LLM"]
    end

    WorldState --> Cognitive
    Cognitive --> Intent
    Cognitive -->|"use_tool_calling"| Earth
    Earth -->|"tool result"| Cognitive
    Intent --> Layer1
    Layer1 --> Layer2
    Layer2 --> Layer3
    Layer3 -->|"approved/rejected"| Intent
```

---

## 四、Tick 完整生命周期

```mermaid
sequenceDiagram
    participant SCH as Scheduler
    participant IW as IntentWorker
    participant BC as Broadcaster
    participant AG as Agent
    participant AS as ActorSoul + EarthSoul
    participant RS as ReflectorSoul
    participant DB as PostgreSQL

    SCH->>SCH: interval.tick()
    SCH->>SCH: calculate_tick_id()
    SCH->>IW: TickBoundary { tick_id }
    IW->>IW: apply_decay() + death check
    IW->>DB: batch persist states
    IW->>IW: update DashMap
    IW->>AG: ExecutionResult (via WS)
    SCH->>BC: broadcast WorldState
    BC->>AG: ServerMessage::WorldState

    Note over AG: Agent 决策循环 (异步，与 Server Tick 并行)
    AG->>AS: think(tick_id, context)
    AS->>AS: CognitiveEngine 4-stage
    AS->>Earth: tool calling (skill_view 等)
    Earth-->>AS: tool result
    AS->>RS: validate_with_reflector()
    RS-->>AS: approved/rejected
    AG->>IW: send Intent via WS

    Note over IW: Intent 实时处理
    IW->>IW: process_intent()
    IW->>DB: persist state (write-through)
    IW->>IW: update DashMap
    IW->>AG: ExecutionResult
```

---

## 五、Intent 提交流程

```mermaid
graph LR
    A["ActorSoul + EarthSoul<br/>决策 + tool calling"] --> B["Intent 结构化"]
    B --> C["ReflectorSoul<br/>分级审核"]
    C --> D{"通过?"}
    D -->|是| E["WebSocket<br/>发送"]
    D -->|否| F["重试 + feedback"]
    F --> A
    E --> G["handler.rs"]
    G --> H["IntentValidator"]
    H --> I{"合法?"}
    I -->|是| J["IntentWorker<br/>实时处理"]
    I -->|否| K["ServerMessage::Error"]
```

---

## 六、即时事件 (Speak) 流程

```mermaid
sequenceDiagram
    participant A as Agent A
    participant S as Server
    participant B as Agent B<br/>(同场景)

    A->>S: ClientMessage::Intent<br/>action_type: speak
    S->>S: handle_intent() 检测即时动作
    S->>S: tokio::spawn 独立任务
    S->>B: ServerMessage::ImmediateEvent
    Note over B: 同场景在线 Agent 立即收到
    Note over S: intent 标记 already_broadcast=true
    Note over S: tick 结算时不再重复处理
```

---

## 七、对话 (Whisper) 流程

```mermaid
sequenceDiagram
    participant A as Agent A
    participant S as Server
    participant B as Agent B

    A->>S: whisper intent<br/>target_agent_id + content
    S->>S: dialogue_manager.create_session()
    S-->>A: DialogueMessage::Accept<br/>{ session_id }
    A->>S: DialogueMessage::Content
    S->>B: forward dialogue
    B->>S: DialogueMessage::Content<br/>(reply)
    S->>S: close_all_sessions()<br/>(tick 结算时)
    S-->>A: ServerMessage::PrivateDialogueRecord
```

---

## 八、死亡通知流程

```mermaid
graph TB
    A["decay::apply_decay()"] --> B{"HP <= 0?"}
    B -->|否| Z[结束]
    B -->|是| C["构建 DeathNotification"]
    C --> D["send_agent_died_notification()"]
    D --> E["ConnectionManager.remove()"]
    E --> F["清空背包 + 死亡掉落"]
    F --> G["UPDATE agents<br/>status = 'dead'"]
    G --> H["ServerMessage::AgentDied<br/>(推送给 Agent)"]
```

---

## 九、Agent 记忆系统

```mermaid
graph TB
    subgraph WM["Working Memory"]
        FIFO["VecDeque<MemoryEntry><br/>FIFO 队列"]
    end

    subgraph EM["Episodic Memory (SQLite)"]
        Events["事件序列<br/>+ decay_strength"]
    end

    subgraph Forgetting["ForgettingScheduler"]
        EBB["Ebbinghaus<br/>遗忘曲线"]
    end

    subgraph SM["Semantic Memory (HNSW)"]
        HNSW["instant-distance<br/>add() 为空操作"]
    end

    FIFO -->|"记忆固化"| Events
    Events --> EBB
    Events -->|"归档"| HNSW
```

---

## 十、Server 模块依赖

```mermaid
graph TB
    main["main.rs"] --> state["state.rs"]
    main --> lib["lib.rs"]

    state --> handlers["handlers/"]
    state --> tick["tick/"]
    state --> ws["websocket/"]
    state --> db["db/"]
    state --> game_data["game_data/"]

    handlers --> agent["agent.rs"]
    handlers --> auth["auth.rs"]
    handlers --> dashboard["dashboard.rs"]
    handlers --> context["context.rs"]
    handlers --> chronicle["chronicle.rs"]
    handlers --> config["config_*.rs"]
    handlers --> validation["validation.rs"]
    handlers --> vendor["vendor.rs"]

    tick --> scheduler["scheduler.rs"]
    tick --> broadcaster["broadcaster.rs"]
    tick --> processor["processor/"]
    tick --> decay["decay.rs"]
    tick --> persistence["persistence.rs"]
    tick --> realtime["realtime.rs"]

    processor --> executor["executor/"]
    processor --> validator["validator.rs"]
    processor --> mutator["mutator.rs"]

    game_data --> loader["loader.rs"]
    game_data --> cache["cache.rs"]
    game_data --> registry["registry/"]
    game_data --> formula["formula_engine/"]
    game_data --> types["types/"]
```

---

## 十一、Agent 模块依赖

```mermaid
graph TB
    lib["lib.rs"] --> core["core/"]
    lib --> soul["soul/"]
    lib --> component["component/"]
    lib --> runtime["runtime/"]

    core --> agent["agent.rs"]
    core --> builder["builder.rs"]
    core --> lifecycle["lifecycle.rs"]

    soul --> actor["actor/"]
    soul --> reflector["reflector/"]
    soul --> earth["earth/"]

    actor --> engine["engine.rs"]
    actor --> chain["chain.rs"]
    actor --> stages["stages.rs"]
    actor --> tools["tools.rs"]
    actor --> prompts["prompt_cache.rs<br/>engine_prompts.rs"]
    actor --> summary["summary_window.rs"]
    actor --> chaos["chaos.rs"]
    actor --> translation["translation.rs"]

    reflector --> validator["validator.rs"]
    reflector --> rule_eng["rule_engine/"]
    reflector --> store["store.rs"]
    reflector --> prompt["prompt.rs"]
    reflector --> cog_val["cognitive_validator.rs"]

    component --> memory["memory/"]
    component --> persona["persona/"]
    component --> social["social/"]
    component --> llm["llm/"]

    runtime --> cognitive["cognitive/"]
    runtime --> claw["claw/"]
```

---

## 十二、数据库 Schema

```mermaid
erDiagram
    devices {
        uuid id PK
        uuid device_id UK
        string auth_token
        timestamp created_at
        timestamp last_seen_at
    }

    agents {
        uuid agent_id PK
        uuid device_id FK
        string name
        text system_prompt
        string status
        timestamp created_at
        timestamp retired_at
    }

    agent_states {
        bigint id PK
        uuid agent_id FK
        bigint tick_id
        jsonb attributes
        uuid node_id
        boolean is_alive
        timestamp created_at
    }

    agent_inventory {
        bigint id PK
        uuid agent_id FK
        uuid item_id FK
        int quantity
        boolean is_equipped
    }

    tick_logs {
        bigint id PK
        bigint tick_id UK
        string status
        timestamp started_at
        timestamp completed_at
    }

    devices ||--o| agents : device_id
    agents ||--o{ agent_states : agent_id
    agents ||--o{ agent_inventory : agent_id
```

---

## 十三、配置热重载

```mermaid
graph TB
    A["scheduler.run()"] --> B{"每 Tick<br/>check_and_reload?"}
    B -->|actions.yaml<br/>已修改| C["load_actions()"]
    C --> D["game_data_cache<br/>.update_actions()"]
    D --> E["init_registry()"]
    E --> F["broadcast_action_update()"]
    F --> G["ServerMessage<br/>::ActionUpdate"]
    B -->|无变化| H[继续下一 Tick]
```

---

## 十四、地魂 Tool Calling 流程

```mermaid
sequenceDiagram
    participant CE as CognitiveEngine
    participant ET as EarthToolExecutor
    participant LLM as LLM Client
    participant FS as File System<br/>(SKILL.md)

    CE->>LLM: complete_json_with_tools(tools)
    LLM-->>CE: tool_call: skill_view { skill_id }
    CE->>ET: execute("skill_view", args)
    ET->>FS: 读取 $CONFIG_DIR/skills/{category}/{skill_id}/SKILL.md
    FS-->>ET: SKILL.md content
    ET-->>CE: { success: true, content: "..." }
    CE->>LLM: complete_json_with_tools(tools, tool_result)
    LLM-->>CE: 最终 Intent
```

---

*文档生成时间: 2026-04-23*
