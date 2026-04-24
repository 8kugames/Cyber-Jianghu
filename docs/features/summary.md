# Cyber-Jianghu 功能架构

**日期**: 2026-04-24

## 核心定位

- **Server (天道)**: 权威物理引擎，Tick 驱动，状态广播
- **Agent (众生)**: 自主 AI 决策，三魂架构，三级记忆

## 说明

每项功能的详细文档参照跳转链接指向的 docs/architecture/*.md

---

## protocol

 - [x] WebSocket 通信协议 — ServerMessage/ClientMessage 全双工消息
 - [x] 三魂认知架构支持 — SoulCycleReport (renhun/tianhun/final_intent)
 - [x] 数据驱动 Actions — ActionType 字符串化，配置与代码分离
 - [x] 多意图管道 — subsequent_intents 支持批量/管道执行
 - [x] Agent 对话系统 — DialogueSession 会话管理 (Request/Accept/Reject/Content/End)
 - [x] 实时事件广播 — ImmediateEvent (speak/whisper 绕过 tick 周期)
 - [x] 位置图系统 — Region→Map→SubScene 层次 + 隐式连接
 - [x] COI 属性系统 — AttributeComponent/StatusComponent/DerivedAttributeComponent
 - [x] 分级 LLM 验证 — OOC 风险分类 (always/adaptive/skip)
 - [x] 叙事配置 — 数值阈值→中文描述 (HP/饥饿/口渴等)
 - [x] Numeric Leak 检测 — `generate_execution_narrative_impl` 中 LLM 输出后置正则 `/\d+/` 检测 + leak guard prompt 重试
 - [x] WorldBuilding 规则 — Era 设定、允许/禁止概念
 - [x] 统一错误码 — GameError + ERROR_CODE_* 常量

---

## server

 - [x] Tick 调度引擎
  - [x] 时钟驱动调度 (非 intent 驱动)
  - [x] tick_id 生成 (Unix 秒级时间戳)
  - [x] accepting_tick_id 原子管理 (AtomicI64)
  - [x] 生理衰减计算 (HP/stamina/hunger/thirst)
  - [x] 寿终检查 (birth_tick → age_years → 超龄清零 HP)
  - [x] 周期性 WorldState 广播
  - [x] TickBoundary 事件处理 (每7游戏日触发 chronicle)

 - [x] 实时 Intent 处理
  - [x] IntentWorker (MPSC channel 单消费者)
  - [x] StateProcessor (验证→执行→Saga 回滚)
  - [x] Intent Resolver (解析校验)
  - [x] Action Executor (状态变更执行)
  - [x] Mutator (item transfer / attribute change)
  - [x] Event Generator (游戏事件构建)
  - [x] ExecutionResult 实时反馈

 - [x] 状态管理
  - [x] AgentStateCache (DashMap write-through)
  - [x] PostgreSQL 持久化 (await 确认后再更新 DashMap)
  - [x] Persist failure 处理 (DashMap 不更新，Agent 收到 success=false)
  - [x] AgentToDeviceMap 反向映射 (device_id → agent_id)
  - [x] RateLimiter (per-agent 限速)

 - [x] WebSocket 连接管理
  - [x] ConnectionManager (在线 Agent 列表)
  - [x] WebSocket Upgrade Handler (凭证校验)
  - [x] 消息广播 (WorldState/对话/死亡通知/配置更新)
  - [x] Ping/Pong 心跳 (tungstenite 自动处理)

 - [x] 游戏数据系统
  - [x] YAML/JSON 配置加载 (actions/attributes/items/locations/skills/recipes)
  - [x] 热重载支持 (actions.yaml mtime 检测 + ConfigUpdate 广播)
  - [x] Formula 引擎 (evalexpr 统一表达式计算)
  - [x] 派生属性/伤害/恢复公式动态解析

 - [x] Action 系统
  - [x] 数据驱动验证 (ActionValidator + AvailableAction schema)
  - [x] 分类执行器
   - [x] basic: idle / speak / move / shout / flee
   - [x] combat: attack
   - [x] interaction: give / steal / pickup / drop / gather / craft / practice
  - [x] 未实现动作 (待开发): defend / dodge / parry / heavy_strike / follow / stealth / poison / repair

 - [x] NPC 对话管理
  - [x] DialogueSession 会话 (Request→Accept/Reject→Content→End)
  - [x] 消息限流 (MessageLimitReached)
  - [x] Session 内存管理 (SessionRegistry in-memory RwLock，非 DB 持久化)

 - [x] 传记生成 (Chronicle)
  - [x] 数据收集器 (7 日聚合: agent stats/highlights/location/deaths/births)
  - [x] 双模式生成 (模板规则同步 / LLM 增强异步)
  - [x] 异步 LLM 补充 + 失败降级

 - [x] 数据库层 (SQLx/PostgreSQL)
  - [x] Agent CRUD (注册/连接/重生)
  - [x] State 持久化 (tick log / action log / soul cycle metadata)
  - [x] GroundItem 管理
  - [x] Vendor 规则管理

 - [x] HTTP API 端点
  - [x] /admin/* — 管理面板
  - [x] /api/v1/agent/* — Agent 管理
  - [x] /api/config/* + /api/admin/reload-config — 热重载配置
  - [x] /api/dashboard/chronicles — 传记查询

---

## agent

 - [x] 三魂架构 (Three-Soul)
  - [x] ActorSoul (人魂) — 直连 WorldState，输出结构化 Intent
   - [x] CognitiveChain 因果推理链
   - [x] 四阶段流程 (Perception→Motivation→Planning→Decision)
   - [x] ChaosGenerator (低 sanity 随机行为注入)
  - [x] 地魂 — Tool calling 工具池
   - [x] EarthToolExecutor (复合工具执行器)
   - [x] skill_view (SKILL.md 加载)
   - [ ] search_memory / recall_archived (返回 unimplemented 错误)
   - [ ] Progressive disclosure 设计
  - [x] ReflectorSoul (天魂) — 三层审查
   - [x] Layer1: action_type 合法性校验
   - [x] Layer2: RuleEngine 规则引擎
   - [x] Layer3: LLM persona/worldview 最终审核
   - [x] Graded Validation (OOC 风险分类)
   - [x] Numeric Leak Detection — `generate_execution_narrative_impl` LLM 输出后正则检测 + leak guard prompt 重试
   - [x] Review Store (PendingReview / ReviewDecision 持久化)

 - [x] 认知引擎 (CognitiveEngine)
  - [x] think_direct 主决策入口 (四阶段合并单次 LLM 调用)
  - [x] NarrativeSummaryWindow (action history sliding window)
  - [x] PromptTemplate (YAML 驱动模板加载)
  - [x] PromptCache (persona + actions prompt 缓存)
  - [x] Translation 层 (中文 boundary translation: aliases→canonical)

 - [x] 三层记忆系统
  - [x] Working Memory — VecDeque FIFO 短期上下文
  - [x] Episodic Memory — SQLite 事件时序存储
  - [x] Ebbinghaus 遗忘曲线 (FORGETTING_INTERVAL_TICKS=84)
  - [x] ImportanceScorer (记忆重要性评分)
  - [x] archive_memories (store.archive_by_ids 实际 SQL 实现)
  - [x] Semantic Memory — HNSW 向量索引 (instant-distance)
   - [x] HnswVectorStore (近似最近邻搜索)
   - [x] FTS Fallback (full-text search 降级)
   - [x] add() (embedding 生成 → episodic DB blob 写入 → HNSW 索引更新)

 - [x] Outcome Memory — Action 结果学习
  - [x] SQLite action result learning
  - [x] Action→Result 映射存储
  - [x] 决策上下文学习

 - [x] 动态角色系统
  - [x] DynamicPersona — 特性演化
  - [x] Trait 系统 — 事件→特性映射 (EventTraitMapper)

 - [x] LLM 客户端抽象
  - [x] DirectLlmClient — 直连 API
  - [x] FallbackLlmClient — 主备模型切换
  - [x] TokenTracking (token 使用量跟踪)

 - [x] 社会关系系统
  - [x] RelationshipStore (SQLite)
  - [x] KeyEvent (关系关键事件)
  - [x] get_relationship_level() (关系等级计算)
  - [x] LLM 关系描述更新

 - [x] 实时事件处理 (ImmediateEvent)
  - [x] 规则门控 (<1ms)
  - [x] 2阶段 LLM 决策 (4s timeout)

 - [x] WebSocket 传输层 — Pure I/O，无业务逻辑

 - [x] HTTP API 服务
  - [x] /api/v1/state — WorldState 查询
  - [x] /api/v1/context — 决策上下文快照
  - [x] /api/v1/character/* — 角色管理/重生/梦境
  - [x] /api/v1/memory/* — 记忆操作
  - [x] /api/v1/review/* — Intent 审查系统

 - [x] 两种运行时模式
  - [x] Cognitive 模式 — FallbackLlmClient 内置 LLM
  - [x] Claw 模式 — OpenClawBridge 外部 LLM 桥接
   - [x] WebSocket Server (runtime/claw/server.rs)
   - [x] Protocol 转换 (DownstreamMessage/UpstreamMessage)

---

## 待实现功能 (Roadmap)

 | 功能 | 位置 | 分类 |
 |------|------|------|
 | 物品耐久衰减 | tick/decay.rs:225-232 | 历史遗留 |
 | 地魂工具接入 | soul/earth/executor.rs:71-81 | 事实待完成 |
 | Progressive disclosure 设计 | executor.rs / lifecycle.rs | 事实待完成 |
