# Cyber-Jianghu 功能架构

**日期**: 2026-04-24

## 核心定位

**Cyber-Jianghu (赛博江湖)** — MMO-MAS (Massive Agent Simulation)，每个角色都是 AI Agent。

- **Server (天道)**: 权威物理引擎，Tick 驱动，状态广播
- **Agent (众生)**: 自主 AI 决策，三魂架构，三级记忆

---

## 一、通信协议 (Critical)

*Server 与 Agent 间的通信契约，协议层无业务逻辑*

- [ ] `ServerMessage` 下行消息 — WorldState / Error / DeathNotification / ImmediateEvent / PrivateDialogueRecord / ExecutionResult
- [ ] `ClientMessage` 上行消息 — Intent / Dialogue / SoulCycleReport
- [ ] `WorldState` 完整世界快照 — tick_id / world_time / location / self_state / entities / nearby_items / events_log
- [ ] `Intent` Agent决策结构 — intent_id / action_type / action_data / priority / subsequent_intents
- [ ] `DialogueMessage` 对话协议 — Request → Accept/Reject → Content → End 完整生命周期
- [ ] `GameError` 统一错误码 — ERROR_CODE_* 常量，机器可读错误分类
- [ ] Protocol Version 协商 — `PROTOCOL_VERSION` 常量

---

## 二、服务端核心 (Critical)

### 2.1 Tick 调度引擎

- [ ] 时钟驱动调度 — `TickScheduler::run()` 循环，不依赖 intent 触发
- [ ] `tick_id` 生成 — Unix 秒级时间戳 (`current_unix_secs - game_epoch`)
- [ ] `accepting_tick_id` 原子管理 — `AtomicI64`，实时模式持续开单
- [ ] Decay 计算 — `tick/decay.rs` 生理衰减 (HP/stamina/hunger/thirst)
- [ ] WorldState 广播 — `tick/broadcaster.rs` 周期性推送
- [ ] TickBoundary 事件 — 触发 chronicle 生成检查 (每7游戏日)

### 2.2 实时 Intent 处理

- [ ] IntentWorker (MPSC channel) — 单消费者，低延迟 intent 消费
- [ ] StateProcessor — 验证→执行→Saga回滚 管道
- [ ] Intent Resolver — `tick/processor/resolver.rs` 解析校验
- [ ] Action Executor — `tick/processor/executor.rs` 状态变更执行
- [ ] Mutator — `tick/processor/mutator.rs` item transfer / attribute change
- [ ] Event Generator — `tick/processor/events.rs` 游戏事件构建
- [ ] ExecutionResult 反馈 — 实时推送给 Agent

### 2.3 状态管理

- [ ] DashMap write-through 缓存 — `state.rs` `AgentStateCache`
- [ ] PostgreSQL 持久化 — await 确认后再更新 DashMap
- [ ] Persist failure 处理 — DashMap 不更新，Agent 收到 `success=false`
- [ ] `AgentToDeviceMap` 反向映射 — device_id → agent_id
- [ ] RateLimiter — per-agent 限速，`RwLock<HashMap<AgentId, Instant>>`

### 2.4 WebSocket 连接管理

- [ ] ConnectionManager — 在线 agent 列表维护
- [ ] WebSocket Upgrade Handler — `websocket/handler.rs` 凭证校验
- [ ] 消息广播 — `broadcast.rs` send_world_state / forward_dialogue / death notification
- [ ] Ping/Pong 心跳 — `tungstenite` 自动处理

---

## 三、Agent 核心 (Critical)

### 3.1 三魂架构

#### ActorSoul (人魂)
- [ ] 直连 WorldState — 无 translation 中转，最快路径
- [ ] 结构化 Intent 输出 — intent_id / action_type / action_data / priority
- [ ] CognitiveChain 因果推理链 — `chain.rs` 记录完整推理过程
- [ ] CognitiveEngine 四阶段 — Perception → Motivation → Planning → Decision
  - [ ] Perception — 数值属性→叙事描述
  - [ ] Motivation — 人设驱动动机生成
  - [ ] Planning — 行动计划制定
  - [ ] Decision — 最终决策输出
- [ ] deadline 感知 — 避免过期 Intent
- [ ] ChaosGenerator — 低sanity随机行为注入

#### 地魂 (EarthSoul)
- [ ] EarthToolExecutor — 复合工具执行器
- [ ] `skill_view` — SKILL.md 加载工具
- [ ] `search_memory` / `recall_archived` — 记忆检索工具
- [ ] Progressive disclosure 设计 — prompt 注入索引，LLM 自主决定何时加载

#### ReflectorSoul (天魂)
- [ ] 三层审查管道 — `validator.rs` 统一入口
  - [ ] Layer1: action_type 合法性校验
  - [ ] Layer2: RuleEngine 规则引擎 (`soul/reflector/rule_engine/`)
  - [ ] Layer3: LLM persona/worldview 最终审核
- [ ] Graded Validation — OOC风险分类 (always/adaptive/skip types)
- [ ] Numeric Leak Detection — 叙事生成时数字泄露检测
- [ ] Review Store — PendingReview / ReviewDecision 持久化

### 3.2 认知引擎

- [ ] `CognitiveEngine::decide()` — 主决策入口
- [ ] `NarrativeSummaryWindow` — action history sliding window
- [ ] `PromptTemplate` 加载 — `prompt_template.rs` YAML驱动
- [ ] `PromptCache` — persona + actions prompt 缓存
- [ ] `Translation` 层 — 中文 boundary translation (aliases → canonical)

### 3.3 运行时模式

- [ ] Cognitive 模式 — `FallbackLlmClient` 内置LLM
  - [ ] DirectLlmClient — 直连 API
  - [ ] FallbackLlmClient — 主备模型切换
  - [ ] TokenTracking — token 使用量跟踪
- [ ] Claw 模式 — `OpenClawBridge` 外部LLM
  - [ ] WebSocket Server — `runtime/claw/server.rs`
  - [ ] Protocol 转换 — `runtime/claw/protocol.rs`

---

## 四、游戏数据系统 (Important)

### 4.1 配置加载

- [ ] `game_data/loaders/` — JSON/YAML 文件解析
  - [ ] actions.yaml / attributes.yaml / items.yaml / locations.yaml
  - [ ] game_rules.yaml / time.yaml / inventory.yaml / recipes.yaml
  - [ ] skills/ — AI Procedural Skills (SKILL.md)
- [ ] `game_data/registry/` — 运行时配置访问
  - [ ] `get_i32/f32/String` 等方法
  - [ ] 热重载 trigger

### 4.2 Formula 引擎

- [ ] `FormulaEngine` — `evalexpr` 统一表达式计算
- [ ] 派生属性计算 — HP上限/攻击力等
- [ ] 伤害公式 — `damage = ...` 动态解析
- [ ] 恢复公式 — `heal = ...` 动态解析

### 4.3 热更新

- [ ] `actions.yaml` 修改检测 — 每 Tick 检查 mtime
- [ ] `ServerMessage::ConfigUpdate` 广播 — 通知所有 Agent
- [ ] Cache invalidation — `GameDataCache` 刷新

---

## 五、Action 系统 (Important)

### 5.1 已实现动作

| 类别 | 动作 | 状态 |
|------|------|------|
| 生存 | 休息, 使用, 进食, 饮水, 拾取, 丢弃, 移动 | ✅ |
| 战斗 | 攻击, 逃跑 | ✅ |
| 江湖技能 | 偷窃, 打坐, 修炼 | ✅ |
| 社交 | 说话, 私语, 大喊 | ✅ |
| 经济 | 给予, 采集, 制造 | ✅ |

### 5.2 Action Executor 分类

- [ ] `executor/basic/` — idle / speak / move / shout / flee
- [ ] `executor/combat/` — attack
- [ ] `executor/interaction/` — give / steal / pickup / drop / gather / craft / practice

### 5.3 数据驱动验证

- [ ] ActionValidator — `actions/validator.rs`
- [ ] ActionType 字符串化 — 非枚举，配置驱动
- [ ] `AvailableAction` schema — valid_targets / required_fields / ooc_risk

### 5.4 未实现动作 (待开发)

- [ ] defend / dodge / parry — 防御相关
- [ ] heavy_strike — 重击
- [ ] follow / stealth / poison / repair — 高级动作

---

## 六、三层记忆系统 (Important)

### 6.1 Working Memory

- [ ] `VecDeque<MemoryEntry>` FIFO 队列
- [ ] 短期上下文保留

### 6.2 Episodic Memory

- [ ] SQLite 持久化 — `backends/episodic.rs`
- [ ] Ebbinghaus 遗忘曲线 — `forgetting.rs` `FORGETTING_INTERVAL_TICKS = 84`
- [ ] ImportanceScorer — 记忆重要性评分
- [ ] `archive_memories()` — 归档 stub (未实现)

### 6.3 Semantic Memory

- [ ] HNSW 向量索引 — `instant-distance`
- [ ] `HnswVectorStore` — approximate nearest neighbor
- [ ] FTS Fallback — full-text search 降级
- [ ] `add()` — 当前为空操作，待实现向量写入

### 6.4 Outcome Memory

- [ ] SQLite action result learning — `outcome.rs`
- [ ] Action → Result 映射存储
- [ ] 决策上下文学习

---

## 七、位置与场景系统 (Important)

### 7.1 Location Graph

- [ ] `LocationNode` — Region / Map / SubScene 层次
- [ ] `LocationEdge` — 节点连接 + travel_cost
- [ ] `get_neighbors()` — 显式邻居
- [ ] `get_implicit_neighbors()` — 父子隐式连接
- [ ] `is_connected()` — 连通性判断

### 7.2 WorldState Scene

- [ ] `adjacent_nodes` — 附近位置
- [ ] `gatherable_items` — 可采集资源
- [ ] `environmental_damage` — 环境伤害

---

## 八、对话系统 (Important)

### 8.1 DialogueSession 状态机

- [ ] `Request` — 对话请求
- [ ] `Accept` / `Reject` — 接受/拒绝
- [ ] `Content` — 消息内容
- [ ] `End` — 结束

### 8.2 服务端路由

- [ ] 中间人转发 — `dialogue_handler.rs`
- [ ] 消息限流 — `MessageLimitReached` 错误
- [ ] Session 状态管理 — `DialogueSession` 持久化

### 8.3 Agent 对话客户端

- [ ] `DialogueClient` — `component/social/dialogue.rs`
- [ ] `DialogueEventHandler` — 事件处理

---

## 九、叙事与传记 (Secondary)

### 9.1 Narrative 配置

- [ ] `NarrativeConfig` — 阈值→描述映射
- [ ] 内置默认 — HP / hunger / thirst / stamina
- [ ] `get_description()` — 数值→中文描述

### 9.2 Chronicle 传记生成

- [ ] `ChronicleCollector` — 7日数据聚合
  - [ ] agent stats / highlights / location stats / deaths / births
- [ ] `ChronicleGenerator` 双模式
  - [ ] Template 模式 — 规则生成 (同步，始终可用)
  - [ ] LLM 模式 — 增强叙事 (异步，失败降级template)
- [ ] `ChronicleStorage` — 持久化到 DB

---

## 十、社会关系 (Secondary)

### 10.1 Relationship Store

- [ ] SQLite 持久化 — `component/social/relationship.rs`
- [ ] `KeyEvent` — 关系关键事件
- [ ] `RelationshipMemory` — 关系记忆
- [ ] `get_relationship_level()` — 关系等级计算

### 10.2 Narrative Generator

- [ ] LLM 关系描述更新
- [ ] `relationship_narrative.rs`

---

## 十一、角色系统 (Secondary)

### 11.1 Dynamic Persona

- [ ] `DynamicPersona` — 演化角色
- [ ] `PersonaState` / `ThreadSafePersona`
- [ ] Trait 系统 — `trait_types.rs`

### 11.2 特性演化

- [ ] `EventTraitMapper` — 事件→特性映射
- [ ] `TraitChange` — 特性变更记录

---

## 十二、OpenClaw 集成 (Secondary)

- [ ] npm 包 `@8kugames/cyber-jianghu-openclaw` (独立仓库)
- [ ] `OpenClawBridge` 实现 `LlmClient` trait
- [ ] WebSocket 协议 — `runtime/claw/protocol.rs`
- [ ] `WsDecisionState` / `WsSharedState` — tick 广播

---

## 十三、待实现功能 (Roadmap)

| 功能 | 位置 | 优先级 |
|------|------|--------|
| 物品耐久衰减 | `tick/decay.rs:225-232` | Medium |
| 语义记忆向量写入 | `component/memory/backends/semantic/backend.rs:161-163` | Medium |
| 记忆归档 | `component/memory/backends/episodic.rs` | Low |
| 未实现战斗动作 | `actions.yaml` 注释掉的 | Low |
| 地魂工具接入 | `search_memory` / `recall_archived` | Medium |

---

## 十四、架构决策备忘

### 数据流

```
Agent ─[WebSocket]→ Transport ─[WorldState]→ CognitiveEngine ─[Intent]→ ReflectorSoul ─[ValidatedIntent]→ IntentWorker
                                                    │                                    │
                                                    ↓                                    ↓
                                              ActorSoul                          Layer1/2/3 Validation
                                              (人魂直连)                                │
                                                                                       ↓
                                                                        ┌──────────────┴──────────────┐
                                                                        ↓                             ↓
                                                                   Approved                          Rejected
```

### 状态写回

```
IntentWorker: Persist to DB (await) → Update DashMap → Send ExecutionResult
                                ↑
                          Persist failure → DashMap NOT updated → ExecutionResult(success=false)
```

### 三魂职责

| 灵魂 | 职责 | 性能目标 |
|------|------|----------|
| 人魂 | 直连WorldState生成Intent | <10ms |
| 地魂 | tool calling 工具池 | LLM决定 |
| 天魂 | 三层审核拦截 | <50ms total |
