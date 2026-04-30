# Cyber-Jianghu 功能架构

**日期**: 2026-04-24

## 核心定位

- **Server (天道)**: 权威物理引擎，Tick 驱动，状态广播
- **Agent (众生)**: 自主 AI 决策，三魂架构，三级记忆

## 说明

每项功能的详细文档参照跳转链接指向的超链接文档。

---

## Protocol (通信与协议层)

### P0 核心

- [x] **[WebSocket 全双工通信](../../crates/protocol/docs/architecture/p0_core/websocket_pipeline.md)**: Server 与 Agent 之间的实时数据管道，负责下发世界状态和上报 Agent 意图。
- [x] **[数据驱动的动作系统 (ActionType)](../../crates/protocol/docs/architecture/p0_core/action_type.md)**: 将动作定义为字符串，彻底解耦硬编码，所有动作属性和限制均由 YAML 配置决定。
- [x] **[统一错误码体系 (GameError)](../../crates/protocol/docs/architecture/p0_core/game_error.md)**: 规范化全局错误类型和状态码，确保异常信息在前后端流转时的明确性。

### P1 重要特性

- [x] **[三魂认知流转 (SoulCycleReport)](../../crates/protocol/docs/architecture/p1_major/soul_cycle_report.md)**: 将 Agent 决策过程拆分为人魂推演、天魂审查、最终意图三步，便于前端可视化展示。
- [x] **[多意图管道 (Subsequent Intents)](../../crates/protocol/docs/architecture/p1_major/subsequent_intents.md)**: 允许 Agent 一次性提交包含后续动作的序列，用于复杂连续行为的排队执行。
- [x] **[Agent 对话会话 (DialogueSession)](../../crates/protocol/docs/architecture/p1_major/dialogue_session.md)**: 管理 NPC 间的对话状态，支持请求、接受、拒绝、内容传递和结束五步流转。
- [x] **[层级位置图系统](../../crates/protocol/docs/architecture/p1_major/hierarchical_map.md)**: 定义大区到子场景的树状地图结构（Region→Map→SubScene），并自动推导场景间的连通关系。
- [x] **[COI 属性组件 (AttributeComponent)](../../crates/protocol/docs/architecture/p1_major/attribute_component.md)**: 采用组合优于继承的设计，将 Agent 的基础属性、动态状态和派生属性模块化管理。

### P2 体验增强

- [x] **[即时事件广播 (ImmediateEvent)](../../crates/protocol/docs/architecture/p2_enhancement/immediate_event.md)**: 绕过 Tick 时钟周期的即时消息通道，专用于处理需要立刻感知的说话或耳语。
- [x] **[分级 LLM 验证机制](../../crates/protocol/docs/architecture/p2_enhancement/graded_llm_validation.md)**: 根据行为的 OOC（出戏）风险等级（总是、自适应、跳过）决定是否触发大模型审核。
- [x] **[自然语言状态映射](../../crates/protocol/docs/architecture/p2_enhancement/nl_state_mapping.md)**: 自动将饥饿、口渴、血量等数值状态转化为中文描述文本，便于直接喂给 LLM。
- [x] **[数值泄漏防护 (Numeric Leak)](../../crates/protocol/docs/architecture/p2_enhancement/numeric_leak_guard.md)**: 通过后置正则检测阻止 LLM 在输出文本中直接暴漏系统数值（如“扣除 10 点 HP”），并利用 Guard Prompt 自动重试。
- [x] **[世界观设定边界 (WorldBuilding)](../../crates/protocol/docs/architecture/p2_enhancement/world_building.md)**: 规定游戏所属时代及允许/禁止的概念，限制 LLM 生成不符合背景的现代词汇。

---

## Server (天道)

### P0 核心

- [x] **[Tick 调度引擎](../../crates/server/docs/architecture/p0_core/tick_scheduler.md)**: 游戏世界的心跳起搏器，负责推进时间、计算生理衰减以及周期性广播世界状态。
  - [x] 基于 Unix 时间戳生成 tick_id，非事件驱动。
  - [x] 原子化管理当前接收的 accepting_tick_id。
  - [x] 处理 HP、体力、饥饿、口渴等生理状态随时间的自然衰减。
  - [x] 寿终正寝检查：超龄自动清零 HP。
  - [x] 触发 TickBoundary 事件，如每 7 个游戏日触发传记生成。
- [x] **[实时 Intent 处理管道 (Real-time Pipeline)](../../crates/server/docs/architecture/p0_core/realtime_pipeline.md)**: 零并发冲突的单线程意图调度器。
  - [x] **单消费者 MPSC 队列**：彻底消除写锁竞争和数据资源冲突。
  - [x] **同地广播 (Co-located Broadcast)**：发生动作后仅向处于同一 `node_id` 的周围 Agent 广播事件（如说话、攻击），避免全局风暴。
- [x] **[状态处理器 (StateProcessor)](../../crates/server/docs/architecture/p0_core/state_processor.md)**: 严格执行业务逻辑的核心管道。
  - [x] **Saga 分布式事务模式**：基于 DashMap 实现写穿透（Write-through），执行包含验证（Validate）、执行（Execute）与数据库持久化。
  - [x] **失败回滚 (Rollback)**：当数据库持久化失败时，利用 Saga 模式逆向回滚 DashMap 中的状态，确保内存与数据库绝对一致。
  - [x] **死亡与掉落机制 (Death Physics)**：Agent 死亡时触发清空背包 (`InventoryManager`)，物品化为 `ground_items` 散落原地，供其他 Agent `pickup`。
  - [x] **物品消耗管线 (`execute_use`)**：统一实装了“进食/饮水/消耗品”的基础逻辑与对生理属性（如饱食度、水分）的增益影响。
- [x] **[动作执行体系 (Action System)](../../crates/server/docs/architecture/p0_core/action_system.md)**: 根据数据字典验证和执行具体交互行为。
  - [x] 基础动作 (Basic)：休息、说话、移动、大喊、修炼、拾取、丢弃、采集、制造。
  - [x] 战斗动作 (Combat)：攻击、逃跑、使用（包含进食/饮水）。
  - [x] 交互动作 (Interaction)：给予、偷窃。
  - [ ] *未实装动作*：防御、闪避、招架、重击、跟随、潜行、下毒、修理。
- [x] **[高性能状态管理](../../crates/server/docs/architecture/p0_core/high_performance_state.md)**: 保障十万级 Agent 并发读写的内存与持久化架构。
  - [x] DashMap 内存缓存层，支持高并发 Write-Through。
  - [x] PostgreSQL 异步持久化，入库成功后才更新内存状态。
  - [x] Per-agent 请求限流器，防止单一 Agent 过载服务器。

### P1 重要特性

- [x] **[连接与会话控制](../../crates/server/docs/architecture/p1_major/connection_session.md)**: 管理所有存活 Agent 的网络接入状态。
  - [x] WebSocket 凭证校验与连接握手。
  - [x] 基于 tungstenite 的 Ping/Pong 自动心跳保活。
  - [x] 针对性或全区广播死亡通知及配置更新。
- [x] **[游戏数据驱动系统](../../crates/server/docs/architecture/p1_major/game_data_driven.md)**: 将所有业务逻辑抽离为外部配置文件，实现修改即生效。
  - [x] 支持 actions/attributes/items/locations/skills/recipes 等模块的 YAML/JSON 配置。
  - [x] 监听文件 mtime 变化，支持配置热重载。
  - [x] 引入 evalexpr 公式引擎，支持动态计算派生属性和伤害数值。
  - [x] 体感叙事系统（`narrative_config.yaml` → `attribute_descriptions`）：Agent 通过世界状态自主感知，替代天道干预式警告注入。
- [x] **[AI 过程性技能系统 (Procedural Skills)](../../crates/server/docs/architecture/p1_major/procedural_skills.md)**: 基于 Markdown 的行为指令系统，体现“身心分离”架构的核心设计。
  - [x] **Server 注册表**：基于 `SKILL.md`（YAML + Markdown）的动态加载与注册 (`SkillRegistry`)。
  - [x] **习得链路**：通过 `practice` 动作触发 `SkillMutator`，将技能 ID 注入 `AgentState`。
  - [x] **认知集成**：Agent 地魂实现 `skill_view` 工具，LLM 按需检索长文本行为指令，避免将庞大技能规则硬塞入 System Prompt。

### P2 体验增强

- [x] **[群像传记生成 (Chronicle)](../../crates/server/docs/architecture/p2_enhancement/chronicle.md)**: 自动编纂世界历史记录的史官系统。
  - [x] 每 7 个游戏日聚合 Agent 数据（击杀、高光时刻、生死等）。
  - [x] 结合模板规则与异步 LLM 生成长篇传记，并支持失败降级。
- [x] **[跨 Agent 传承 (Lessons)]()**: 基于集体死亡经验的共享教训库（Layer 1-2）。
  - [x] Layer 1: AgentDied.metadata 携带属性快照/存活时间/死因，broadcast 透传。
  - [x] Layer 2: public_lessons 表按死因聚合教训，WorldState.lessons_learned 下发，Agent DecisionContext 注入。
  - [ ] Layer 3: 代际遗传（待 Layer 1-2 验证后推进）。
- [x] **[HTTP API 与管理后台](../../crates/server/docs/architecture/p2_enhancement/http_api_admin.md)**: 提供可视化管理和人工干预入口。
  - [x] HTTP API 端点
    - [x] /admin/* — 管理面板 (带 Read/Write Token 鉴权)
    - [x] /api/v1/agent/* — Agent 管理 (包含 Vendor 补货规则配置)
    - [x] /api/config/* + /api/admin/reload-config — 热重载配置
    - [x] /api/config/llm/* — LLM 服务在线测试与切换
    - [x] /api/dashboard/chronicles — 传记查询
    - [x] /health — 节点存活与 Tick 周期探针
  - [x] 管理后台前端 UI (Server Admin Dashboard)
    - [x] 静态多页 Web 应用 (crates/server/static/admin)
    - [x] 细粒度权限控制 (Read/Write Token 鉴权登录)
    - [x] 服务器状态大盘监控 (Tick流转、Agent在线分布)
    - [x] 智能体全生命周期管理 (检索、详情面板、物品定点发放干预)
    - [x] NPC商人自动补货规则管理 (Vendor Refill 阈值/预算设定)
    - [x] 运行时配置编辑器 (YAML/JSON 语法高亮与热重载)
    - [x] LLM 服务配置面板 (Ollama/OpenAI 兼容接口在线测试与切换)
    - [x] 全局经历日志流查询 (Experiences Stream)
    - [x] 世界传记检阅器 (Chronicles Viewer)

---

## Agent (众生)

### P0 核心

- [x] **[三魂架构 (Three-Soul)](../../crates/agent/docs/architecture/p0_core/three_soul.md)**: Agent 决策的哲学分层模型，隔离认知、执行与自我审查。
  - [x] **人魂 (ActorSoul)**：主导动机推演与规划的“感性与理性大脑”。
    - [x] 直连世界状态生成因果推导链，结合环境上下文、记忆和社交关系生成 Intent。
    - [x] 内置低 San 值混沌行为注入器，模拟精神崩溃时的非理性行为（如发疯、喃喃自语）。
  - [x] **地魂 (EarthSoul)**：对接物理世界的“工具执行池”。
    - [x] 负责将人魂意图转换为系统 API 调用，并在决策中途按需获取外部数据。
    - [x] 记忆检索工具 (`search_memory`, `recall_archived`)：供 LLM 检索工作记忆与情景/语义记忆。
    - [x] 技能查阅工具 (`skill_view`)：供 LLM 按需获取武功等长文本技能详情，避免撑爆 System Prompt。
  - [x] **天魂 (ReflectorSoul)**：三段式“自我审查官”。
    - [x] Layer 1 动作校验：基础 ActionType 与参数合法性验证。
    - [x] Layer 2 物理规则审查 (RuleEngine)：YAML 配置驱动的世界观刚性规则和物理可行性检验（如禁止穿墙、禁止使用不存在的物品）。
    - [x] Layer 3 角色 OOC 审查：基于 LLM 的人物性格符合度动态拦截，按严重程度分类 OOC 等级。
      - [x] 角色名排除：验证 prompt 包含角色名 + 穿越排除说明，防止历史人物同名被误判。
- [x] **[认知流转引擎 (CognitiveEngine)](../../crates/agent/docs/architecture/p0_core/cognitive_engine.md)**: 将环境感知转化为具体行动的思考中枢。
  - [x] **认知链追踪 (Cognitive Chain)**：全链路追踪并记录从“感知”到“动机”再到“规划”的每一步逻辑推导，不仅用于日志分析，还作为核心数据打包进 `SoulCycleReport`。
  - [x] 单次 LLM 调用融合“感知→动机→规划→决策”四阶段，降低延迟。
  - [x] 基于滑动窗口的历史行为摘要提取。
  - [x] 动态 YAML 模板渲染，结合 Persona 缓存加速 Prompt 构建。
  - [x] 内置中英别名翻译转换，纠正 LLM 产生的格式幻觉。
    - [x] 动作名称翻译（如将 LLM 幻觉的"攻击某人"映射为"攻击"）。
    - [x] 字段映射转换（如将 LLM 幻觉的 "destination" 映射为 "target_location"）。
    - [x] 对象 ID 解析（从 WorldState 解析周围实体名称，转换为 UUID / NodeID / ItemID）。
- [x] **[三级记忆系统](../../crates/agent/docs/architecture/p0_core/memory_system.md)**: 模拟人类记忆衰退与联想机制的数据结构。
  - [x] **地魂记忆回溯接入 (Memory Tools)**: 在地魂工具池中实装 `search_memory` 和 `recall_archived`，使 LLM 能在思考过程中按需检索情景与语义记忆。
  - [x] **工作记忆 (Working Memory)**：基于 FIFO 队列维护短期上下文。
  - [x] **情景记忆 (Episodic Memory)**：利用 SQLite 持久化存储带时间戳的事件，包含遗忘曲线与重要度评分机制。
    - [x] 基于艾宾浩斯遗忘曲线的记忆归档机制。
    - [x] 自动基于事件类型与元数据为记忆进行重要性打分。
  - [x] **语义记忆 (Semantic Memory)**：采用 HNSW 向量索引实现相似度联想，并在失败时降级为全文检索。Docker 镜像内建 bge-small-zh-v1.5 嵌入模型（~100MB）。
- [x] **[双栖运行模式](../../crates/agent/docs/architecture/p0_core/dual_mode.md)**:
  - [x] **Cognitive 模式**：调用内置 LLM 的独立智能体。
  - [x] **Claw 模式**：通过 OpenClaw 桥接外部第三方 LLM 的附庸模式。

### P1 重要特性

- [x] **[模型网关与调度](../../crates/agent/docs/architecture/p1_major/model_gateway.md)**: 统一的 LLM 客户端池，支持主备模型无缝切换及 Token 消耗监控。
  - [x] `prefer_stream` 全局流式优化：支持流式的模型跳过 400 降级，直接走 streaming。
  - [x] Token 统计修复：单模型场景 `UsageTrackingStream` 包装 + 非 streaming 路径估算兜底。
  - [x] max_tokens 自适应：API 400 错误中提取 per-model 限制，运行时学习并持久化。
  - [x] `FallbackModelConfig` 支持 per-model 独立 max_tokens 配置。
- [x] **[经验结果记忆 (Outcome Memory)](../../crates/agent/docs/architecture/p1_major/outcome_memory.md)**: Agent 对动作结果的经验学习池，用于优化未来决策。
- [x] **[动态角色演化 (DynamicPersona)](../../crates/agent/docs/architecture/p1_major/dynamic_persona.md)**: 允许 Agent 经历特定事件后获得新性格标签（Trait），实现性格随阅历成长。

### P2 体验增强

- [x] **[异步即时事件引擎 (SessionTriageEngine)](../../crates/agent/docs/architecture/p2_enhancement/session_triage.md)**: 处理非 Tick 周期突发事件的后台大脑。
  - [x] 使用 WAL 模式 SQLite 确保事件不丢失。
  - [x] 基于 LLM 的事件分类器，区分“需立刻响应”、“可稍后批处理”与“忽略”。
    - [x] urgent (立刻响应): 立即注入 Agent 的 Memory Context 供下一轮主决策循环使用。
    - [x] batch (稍后批处理): 收集并在当前游戏日结束时打包。
    - [x] ignore (忽略): 从记录中清理或不进入主流程。
  - [x] 每日摘要生成：游戏日结束时 `produce_daily_summary()` 生成当日事件摘要。
  - [x] 摘要本地存储：lifecycle 接收摘要后写入 Episodic Memory（importance 可配置）。
  - [x] 摘要 Server 提交：通过 WebSocket `ClientMessage::DailySummary` 提交，支持指数退避重试。
  - [x] Server 入库：`agent_daily_summaries` 表 UPSERT（agent_id + game_day），Admin 端可查询。
- [x] **[人际社交网络 (RelationshipStore)](../../crates/agent/docs/architecture/p2_enhancement/relationship_store.md)**: 记录并量化 Agent 间的互动历史与好感度阶梯，影响其社交决策。
  - [x] 支持物品转移（SocialInteraction）、公开说话（PublicMessage）、密语（PrivateDialogue）三种事件类型触发好感度更新。
  - [x] 名字解析链路 `name_map → store → "陌生人"`，防止已有真名被覆写。
  - [x] 控制台关系卡片化 + Modal 详情（Agent ID、密语沟通记录、关键事件）。
- [x] **[玩家控制台 (Agent Control Panel)](../../crates/agent/docs/architecture/p2_enhancement/agent_control_panel.md)**: 允许人类玩家观察并干预 AI 角色的前端面板。
  - [x] 实时 SSE 数据流展示心跳、推演记录与周围状态。
  - [x] 辅助创建角色，一键生成世界树与属性雷达图。
  - [x] 托梦接口：上帝视角向指定 Agent 注入强制文本思想。
- [x] **[命令行工具 (CLI)](../../crates/agent/docs/architecture/p2_enhancement/cli.md)**:
  - [x] 提供 `run` / `config` / `create-character` / `show` / `reset` 等快速运维指令。
  - [x] 支持通过 `--port 0` 自动探测并分配可用通信端口。
  - [x] `CYBER_JIANGHU_DATA_DIR` 环境变量：Docker 容器内数据持久化到挂载卷。

---

## 待实装/优化功能 (Roadmap)

| 功能 | 位置 | 分类 | 说明 |
|------|------|------|------|
| **每日事件摘要入库** | `component/immediate/session_triage.rs` | ✅ 已实装 | 游戏日结束时 `produce_daily_summary` 生成摘要 → Agent 本地 Episodic Memory + WebSocket 提交 Server 存档（`agent_daily_summaries` 表，UPSERT），Chronicle 聚合时 LEFT JOIN 注入 `AgentSummary.narrative`；玩家端 `GET /api/v1/memory/daily-summaries` 查看个人摘要；Admin 端 `admin/history.html（历史记录 → 每日摘要 tab）` 查看所有摘要。 |
| **未实现交互动作拓展** | `actions/executor/` | 功能补全 | 防御、闪避、招架、重击、跟随、潜行、下毒、修理等配置已规划但逻辑未落地。 |
| **动作冷却检查 (Cooldown)** | `actions/validator.rs:55` | 机制完善 | `TODO BUG-2`: 待在 AgentState 中补充 `last_action_ticks` 以支持动作频率限制。 |
