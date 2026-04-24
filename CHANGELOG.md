
# Cyber-Jianghu 更新日志

本变更日志记录每次重要提交的汇总信息和影响面。

---

## [Unreleased]

### Added

- **Agent**: 地魂记忆回溯工具接入
  - 在 `EarthToolExecutor` 中实装了 `search_memory` 和 `recall_archived`
  - 通过 `Arc<tokio::sync::RwLock<MemoryManager>>` 解决了记忆管理器的并发所有权问题
  - 将 `MemoryManager` 实例注入到 `CognitiveEngine` 和地魂工具池中
  - 支持 LLM 在思考过程中按需检索情景与语义记忆

### ⚠️ Breaking Changes

- **Agent**: `MemoryBackend::add()` 签名破坏性变更
  - 旧: `async fn add(&mut self, MemoryEntry) -> Result<()>`
  - 新: `async fn add(&mut self, &mut MemoryEntry) -> Result<i64>`
  - 返回值从 `()` 改为插入记录的 DB ID（-1 表示跳过/过滤）
  - `add_batch` 默认实现改为 `for mut memory in memories` 消费所有权
  - 影响: WorkingMemoryBackend / EpisodicMemoryBackend / SemanticMemoryBackend 全部适配

- **Protocol**: `LifespanRules` 删除 `ticks_per_year` 字段
  - 改为从 `time.yaml` 唯一配置源派生：`ticks_per_hour * hours_per_day * days_per_season * seasons_per_year`
  - `game_rules.yaml` lifespan 配置仅保留 `max_age` / `aging_start_age`
  - Agent 端 `LifespanCalculator` / `LifespanConfig` / `LifespanStatus` 全部删除
  - Agent 寿命数据改为从 Server 下发的 WorldState 被动读取

- **Protocol**: `AgentSelfState` 新增 `age_years: Option<u32>` / `max_age: Option<u32>`
  - `skip_serializing_if = "Option::is_none"` 兼容旧客户端
  - Agent 仅用于叙事，不用于决策

- **Server**: DB migration `014_agent_birth_tick.sql` — agents 表新增 `birth_tick BIGINT` 列
  - 新注册角色写入 `birth_tick = current_tick_id`
  - 已有角色 `birth_tick = NULL` → 视为不朽，不触发寿命检查

- **Protocol**: 移除 `TRADE` 动作常量 (`protocol::types::actions::TRADE`)
  - 不再存在系统强制的交易动作类型
  - 交易改由 Agent 自行通过 `speak` 议价 + `give` 交割

- **Protocol**: 移除 `TradeExecuted` StateChange 变体
  - `state_changes::TradeExecuted` 不再存在
  - Server 端交易处理逻辑（~230 行 DB 事务）全部移除

- **Protocol**: 移除 `default_adaptive_types()` / `default_adaptive_field_mapping()` 中的 `"交易"` 条目
  - 自适应动作系统不再包含交易类型

- **Server**: 移除 `actions.yaml` 交易动作定义
  - 交易不再是 Server 识别的合法动作

- **Server**: 配置文件 `world-building-rules.yaml` 重命名为 `world_building_rules.yaml`（snake_case 统一）
  - 旧文件名不再生效

- **Agent**: 即时事件架构全面重写：内存队列 → SQLite 持久化 + Session Triage LLM
  - 旧架构: `Vec<WorldEvent>` (capacity 32) 内存队列 + per-event LLM triage (4s timeout) + `RespondNow` 即时回应
  - 新架构: EventStore (SQLite WAL) + SessionTriageEngine (每游戏日后台任务，批量 LLM triage) + Notify 信号
  - `ImmediateEventHandler` 职责简化为: 收消息 → DB 写入 + Notify 信号（纯 IO，<1ms）
  - 主 tick 消费 triaged 事件: urgent 逐条注入 memory_context，batch 摘要格式注入
  - 移除 `RespondNow`: 所有回应由主 tick 统一决策（60s 确定性 > 5s best-effort）
  - `ImmediateEventConfig` 新增 `event_triage: Option<EventTriageConfig>` 配置段
  - `WorldEventType` 新增 `Hash` derive（HashMap key）
  - `game_rules.yaml` 新增 `event_triage` 配置节（lifecycle / pre_filter / context / retention）
  - `GameRules` 新增 `calendar: Option<CalendarConfig>` 字段（数据驱动 game_day 计算）
  - `WorldTime` struct 注释修正（month/day/hour 范围由 time.yaml 控制，非固定 1-12/1-30/0-23）
  - 移除 `ImmediateDecisionRules` / `immediate_routing_actions` 旧架构死代码（~100 行 Rust + 30 行 YAML）
  - `mark_processed` 改为按 ID 精确标记（`mark_processed_by_ids`），消除与后台 triage 竞态
  - Agent `close()` 正确终止 `session_triage_handle` 后台任务

### Added

- **Server**: AI Procedural Skills 系统
  - SKILL.md 行为指令文件（7 个初始技能：武功/社交/生存/采集）
  - `skills_loader` + `skill_registry` + `SkillMutator` 完整加载链路
  - `practice` 动作 → `SkillLearned` StateChange → `SkillMutator` 追加到 `AgentState.skills`
  - `SkillInfo` protocol 类型 + `WorldState.self_state.skills` 字段
  - 路径: `config/skills/{category}/{skill_id}/SKILL.md`

- **Agent**: 地魂（EarthSoul）tool-calling 工具池
  - `EarthToolExecutor`: 复合 ToolExecutor，路由 skill_view / search_memory / recall_archived
  - `skill_view`: 从缓存或文件加载 SKILL.md body，供 LLM 按需获取技能详情
  - `search_memory` / `recall_archived`: 预留接口（待 MemoryManager 所有权解决后接入）
  - 工具池位于 `soul/earth/` 目录，与天人二魂并列

- **Agent**: LlmClient tool-calling 增强
  - 新增 `complete_with_conversation_and_tools()` trait 方法
  - 提取 `run_tool_loop()` 消除 tool-calling 循环代码重复
  - 支持对话历史 + tool-calling 组合路径（正常部署主路径）
  - `FallbackLlmClient` 完整转发支持

- **Agent**: Progressive disclosure 技能加载
  - prompt_templates.yaml 新增 `skill_index_header` / `skill_full_header` / `tool_hints_header` section
  - Tool-calling 启用时：prompt 只注入技能索引，LLM 通过 `skill_view` 按需加载详情
  - 非 tool-calling 降级：注入完整 SKILL.md body（向后兼容）
  - 工具描述从 `ToolDefinition.description` 动态构建（单一数据源，数据驱动）

- **Agent**: 交易 prompt 引导
  - 新增"交易规则" prompt section：speak 议价 + give 交割
  - 强调先给风险（"江湖规矩，信错人要付出代价"）
  - 无公定价，价格由双方自行决定

### Changed

- **Server**: 寿命系统 Server 权威化
  - `decay.rs` 新增寿终检查：生理衰减 + 环境伤害之后，`birth_tick` 非空时计算年龄
  - `compute_age_years(birth_tick, tick_id)` 复用 broadcaster 相同公式，从 `time.yaml` + `game_rules.yaml` 派生
  - 超龄 → 清零 HP → 复用现有死亡流程（DeathNotification + AgentDied + 背包清空 + 自动重生）
  - `broadcaster.rs` 3 个 AgentSelfState 构造点注入 `age_years` / `max_age`
  - `death_defaults` 新增 `old_age: { cause: "old_age", message: "你已寿终正寝，安详离世......" }`
  - 重生时 `birth_tick` 重置为 `rebirth_tick`（新角色重新计算寿命）

- **Agent**: `think_direct()` 路由重构
  - tool-calling 提升为顶层条件（先前嵌套在 `conv_data == None` 分支下是死代码）
  - 路由顺序：tool-calling + conversation → tool-calling only → streaming/plain
  - streaming 不支持 tool-calling 组合（文档说明）

- **Agent**: `build_direct_prompt()` 新增 `use_tool_calling` 参数
  - 根据 LLM 能力自动切换 progressive disclosure / full body 模式

- **Agent**: `config_dir` 统一解析
  - `CognitiveEngine` 新增 `config_dir` 字段，启动时从 env 解析一次
  - `build_skill_instructions()` 使用 `self.config_dir` 替代重复 env var 解析

- **Agent**: `extract_skill_body()` 去重
  - 统一为 `earth::skill_tool::extract_skill_body()`（`pub(crate)`）
  - 消除 `engine_prompts.rs` 和 `skill_tool.rs` 两份不一致实现

- **Agent+Server**: "赠送" 统一改为 "给予"
  - Server: processor/executor.rs state_change description
  - Agent: lifecycle.rs / social.rs / chaos.rs / relationship.rs 测试

### Removed

- **Agent**: `LifespanCalculator` / `LifespanConfig` / `LifespanStatus` / `AgingEffectValues` / `AgingEffects` / `AgingStage`
  - 删除 `crates/agent/src/component/persona/lifespan.rs`
  - 删除 `crates/agent/src/component/persona/lifespan_types.rs`
  - 12+ 文件移除所有 lifespan_calculator 引用

- **Server**: 交易动作完整链路
  - `actions.yaml` 交易定义
  - `TradeData` struct / `TRADE` 常量
  - `execute_trade()` (~80 行) / `TradeExecuted` handler (~230 行)
  - validator 交易验证规则

---

## [0.1.0] — 实时架构改造

### ⚠️ Breaking Changes

Tick 批处理模式全面退役，Intent 实时化。版本 0.0.x → 0.1.0。

- **Protocol**: 新增 `ServerMessage::ExecutionResult` 变体
  - 格式: `{ tick_id, intent_id, success, error?, state_change_summary? }`
  - Agent **必须**处理此消息以获取实时执行反馈
  - 旧版 Agent 无法感知 Intent 执行结果

- **Agent**: 统一 `ClientMessage` 通道替代双通道
  - 删除 `immediate_msg_tx` / `immediate_msg_rx` 独立通道
  - speak/whisper/emote 等即时事件统一通过 `intent_tx: Sender<ClientMessage>` 发送
  - `ClientMessage::SoulCycleReport` 替代旧的 `SoulCycleData` 直接发送

- **Server**: 删除 `IntentManager` 批处理意图缓存
  - `AppState.intent_manager` 字段移除
  - `create_intent_manager()` / `take_intents_for_tick()` 函数移除
  - `websocket::IntentManager` 类型别名移除
  - handler.rs 不再将 Intent 写入 IntentManager，改为直接入队 IntentWorker

- **Server**: 删除 `accepting_tick_id` 校验
  - handler.rs 不再检查 `intent.tick_id == accepting_tick_id`
  - Agent 不再需要同步 tick_id 即可提交 Intent

- **Server**: `TickScheduler` 移除批处理字段
  - 删除 `closed_dialogue_records`、`execution_summaries`、`dialogue_manager`、`intent_manager` 字段
  - `broadcast_states()` 不再接收这两个参数

### Added

- **Server**: `IntentWorker` 实时处理引擎 (`tick/realtime.rs`)
  - 单消费者 MPSC channel(256)，顺序处理 Intent + TickBoundary
  - Intent 路径：DashMap 读取 → StateProcessor 执行 → DB persist → DashMap 更新 → 广播
  - TickBoundary 路径：批量衰减 → persist → 死亡处理（物品掉落 + DB标记 + DashMap清理 + WS断连）
  - `WorkerMessage` 枚举统一 Intent 和 TickBoundary

- **Server**: `StateProcessor::process_single_intent()` 单条 Intent 处理
  - 从 `process_intents()` 提取，接受单个 `AgentState` + `&[Intent]`
  - 保留完整 Saga 快照/回滚机制
  - 新增 `all_states: &[AgentState]` 参数支持跨 Agent 校验

- **Server**: `AgentStateCache` (DashMap) 内存缓存 (`state.rs`)
  - `Arc<DashMap<Uuid, AgentState>>`，启动时从 DB 加载
  - write-through: persist 到 DB 确认后才更新 DashMap
  - `broadcast_speak_to_location` 从 DashMap 读取位置，不再查 DB

- **Server**: `broadcast_speak_to_location` 改用 DashMap
  - speak 广播路径消除 SQL 查询，纯内存过滤同位置 Agent

- **Protocol**: `ServerMessage::ExecutionResult` 实时执行反馈
  - Agent 端通过 `try_receive_execution_result()` 获取（watch channel，非阻塞）

### Changed

- **Server**: Tick 退化为纯时钟
  - 每周期：发送 TickBoundary（触发衰减）→ 广播 WorldState
  - 不再收集/结算 Intent

- **Server**: handler.rs Intent 路由改造
  - 删除 accepting_tick_id 检查、IntentManager 写入
  - 非阻塞 `try_send` 入队 IntentWorker

- **Agent**: lifecycle.rs 主循环统一发送路径
  - `send_immediate_intent()` 走 `send_intent()`（统一 `Sender<ClientMessage>`）
  - 即时事件 binding 使用 `intent_sender()` 替代 `immediate_msg_sender()`

- **Agent**: websocket.rs 后台任务单一 recv 循环
  - `intent_rx.recv()` 统一处理 `ClientMessage::Intent` 和 `ClientMessage::SoulCycleReport`

### Removed

- **Agent**: 双通道系统（`immediate_msg_tx` / `immediate_msg_rx`）
  - `send_immediate_message()` 方法
  - `immediate_msg_sender()` 方法
  - `immediate_msg_tx` / `immediate_msg_rx` channel

- **Server**: IntentManager 整条链路
  - `IntentManager` type alias
  - `create_intent_manager()` 函数
  - `take_intents_for_tick()` 函数
  - `AppState.intent_manager` 字段

- **Server**: TickScheduler 批处理字段
  - `closed_dialogue_records`、`execution_summaries`、`dialogue_manager`

---

## [0.0.104] - 2026-04-10

---

## [0.0.33] - 2026-03-23

- **Agent**: CLI 移除 `--role` 和 `--target-endpoint` 参数
  - 移除远程 Observer 模式（HTTP 轮询其他 Agent）
  - ReflectorSoul 现在作为进程内双 Soul 架构默认启用
  - 原因：简化架构，统一使用 AgentBuilder 接口

- **Agent**: HTTP Intent API 禁用
  - 移除 `POST /api/v1/intent` 路由
  - 强制使用 WebSocket 提交 Intent（确保 Tick 同步）
  - 原因：HTTP 轮询无法保证 tick_id 实时同步，会导致意图被拒绝

### Added

- **Agent**: ActorSoul 和 ReflectorSoul LLM 独立配置
  - 新增 `llm_reflector` 配置字段，支持独立配置 ReflectorSoul LLM
  - 新增 GET /api/v1/config/llm/providers 端点
  - 新增 GET /api/v1/config/llm 端点获取当前配置
  - 新增 POST /api/v1/config/llm 端点更新配置
  - Web 面板新增 LLM 配置界面
  - 配置变更通过文件监听自动热重载
  - API Key 格式验证和内存安全（zeroize）
  - 配置更新原子替换 + 备份回滚机制

- **Agent**: ActorSoul + ReflectorSoul 双 Soul 架构
  - 新增 `ReviewStore` 共享内存用于进程内审查通信
  - ActorSoul (行动之魂)：生成意图，执行行动
  - ReflectorSoul (反思之魂)：审查意图，世界观一致性审查（默认启用）
  - AgentBuilder 新增 `with_review_store()` 和 `with_reconnect_rx()` 方法

- **Agent**: 审查系统默认启用
  - Cognitive 和 Claw 模式均默认启用 ReflectorSoul
  - 支持三种审查结果：Approved、Rejected、TimeoutApproved
  - 审查超时自动批准（默认 30 秒）

- **Agent**: 架构统一（COI 原则）
  - Cognitive 和 Claw 模式统一使用 AgentBuilder
  - 移除 `Agent::new()` 的使用（改用 Builder）
  - 确保两种模式功能完全一致

- **Server**: agent_id → device_id 反向映射系统
  - 新增 `AgentToDeviceMap` 类型维护角色到设备的映射
  - 在 `agent_register` 和 WebSocket 连接时自动更新映射
  - 解决设备与角色分离后，WorldState 广播找不到正确连接的问题

- **Agent**: WebSocket Tick 消息集成四阶段认知上下文
  - `DownstreamMessage::Tick` 新增 `cognitive_context` 字段
  - 结构化四阶段推理引导：Perception → Motivation → Planning → Decision
  - OpenClaw 可直接使用认知上下文进行推理，无需额外 API 调用

### Changed

- **Agent**: 配置文件新增 `config_path` 字段

- **Server**: WebSocket 连接管理改用 device_id 作为 key
  - 连接管理器现在以 device_id 而非 agent_id 存储连接
  - 支持同一设备管理多角色的场景

### Removed

- **Agent**: 移除远程 Observer 模式相关代码
  - 删除 `run_observer_mode()` 函数
  - 删除 `fetch_pending_reviews()` 和 `process_review_remote()` 函数
  - 删除 `--role observer` 和 `--target-endpoint` CLI 参数
  - 保留 HTTP API 端点供外部监控工具使用

- 删除过时的设计文档：
  - `docs/openclaw-cognitive-integration.md`
  - `docs/superpowers/plans/2026-03-23-agent-death-notification.md`
  - `docs/superpowers/specs/2026-03-22-agent-openclaw-error-forwarding-design.md`
  - `docs/superpowers/specs/2026-03-23-agent-death-notification-design.md`
  - `联调测试.md`

---

## [0.0.33] - 2026-03-23

### Added

- **Agent**: Server → OpenClaw 消息透传机制
  - Agent 实时转发 Server 下行消息给 OpenClaw（WebSocket）
  - 支持：错误消息、对话消息、游戏规则更新、世界观规则更新
  - 新增 `ServerErrorCode` 结构化错误码枚举
  - 新增 `DownstreamMessage` 变体：`ServerError`、`ServerDialogue`、`ServerGameRulesUpdate`、`ServerWorldBuildingRulesUpdate`、`MissedMessages`

- **Agent**: WebSocket Server 安全限制
  - 仅允许 localhost 连接（拒绝远程连接）
  - 单连接限制（同一时间只允许一个 OpenClaw 连接）
  - 连接断开时自动释放 slot

- **Agent**: WebSocket Client 回调机制
  - 新增 `set_server_msg_callback()` 方法
  - 收到 Server 消息时触发回调，实现消息透传

### Fixed

- **Agent**: 修复单连接限制的竞态条件
  - 问题：拒绝第二个连接时错误地释放了第一个连接的 slot
  - 解决：拒绝连接时不调用 `store(false)`，slot 由已建立连接在断开时释放

### Changed

- **Agent**: 版本号 0.0.29 → 0.0.33

### Technical Details

消息流转路径：
```
Game Server → WebSocket Client → server_msg_callback → broadcast::Sender
           → WebSocket Server → OpenClaw
```

新增 API：
- `Agent::set_server_msg_callback(callback)` - 设置 Server 消息透传回调
- `AgentClient::set_server_msg_callback(callback)` - 同上
- `WebSocketClient::set_server_msg_callback(callback)` - 同上

---

## [0.0.20] - 2026-03-22

### ⚠️ Breaking Changes

- **Agent**: 移除 `--mode` 命令行参数，现在只有 Claw 模式（默认）
  - 旧命令: `cyber-jianghu-agent --mode claw run`
  - 新命令: `cyber-jianghu-agent run`

- **Agent**: Intent API 响应格式变更
  - 旧格式: 纯文本 `"Intent submitted"`
  - 新格式: JSON `{"status": "submitted", "intent_id": "...", "tick_id": N, "action_type": "..."}`

### Fixed

- **Agent**: 修复 HTTP API 死锁问题
  - 问题: 注册回调中 RwLock 读锁未释放就尝试获取写锁，导致永久阻塞
  - 解决: 显式 `drop(old_id)` 释放读锁后再获取写锁
  - 影响: 修复后 HTTP API 正常响应

- **Server**: 修复生产环境部署失败问题
  - 修复空 Token 问题：环境变量为空字符串时自动生成随机 Token
  - 添加数据库迁移自动执行

- **Server**: 修复 `get_agent_by_device_id` 函数未导出问题
  - 添加到 `db/mod.rs` 导出列表

- **Agent**: 修复 Agent Docker 部署和数据库类型不匹配问题

### Added

- **Agent**: Cognitive Context API (`/api/v1/cognitive`)
  - 四阶段推理结构：Perception → Motivation → Planning → Decision
  - 引导 LLM 按认知流程进行决策

- **Agent**: 多角色管理系统
  - `GET /api/v1/characters` - 获取所有角色列表
  - `POST /api/v1/characters/switch` - 切换当前活跃角色
  - 支持已故和归隐角色的历史记录

- **Agent**: Web Panel 智能路由
  - 首页根据服务器连通性和角色状态自动跳转
  - 角色信息页支持多角色切换

- **Agent**: 服务器热切换 API
  - `POST /api/v1/config/server` - 动态切换服务器地址
  - 自动触发 WebSocket 重连

- **Server**: 设备认证系统
  - `POST /api/v1/agent/connect` - 设备注册获取 auth_token
  - WebSocket 连接需要 token 参数

- **Server**: Intent 全链路追踪
  - 每个 Intent 分配唯一 `intent_id`
  - 支持 `priority` 字段

### Changed

- **Agent**: 重构决策模式
  - 移除 `http` / `ws` / `cognitive` 模式区分
  - 统一为 Claw 模式（HTTP API + WebSocket 服务）

- **Agent**: 版本号 0.0.15 → 0.0.16 → 0.0.20

- **Config**: `CharacterConfig` 新增字段
  - `server_url`: 角色所属服务器
  - `status`: 角色状态 (alive/dead/retired)

### Removed

- **Agent**: 移除过时的 OpenClaw 内联模式代码
- **Agent**: 移除 `--mode` 命令行参数

---

## [0.0.16] - 2026-03-22

### Added

- **Agent**: 多角色管理系统
  - 支持在同一设备上管理多个角色（包括已故和归隐角色）
  - 每个角色关联到特定服务器，记录角色来源
  - 新增 `CharacterStatus` 枚举（Alive/Dead/Retired）跟踪角色状态
  - `GET /api/v1/characters` - 获取所有角色列表
  - `POST /api/v1/characters/switch` - 切换当前活跃角色

- **Agent**: Web Panel 智能路由
  - 首页根据服务器连通性和角色状态自动跳转
  - 无角色或服务器不可达时优先显示管理页
  - 有存活角色且服务器可达时显示角色信息页

- **Agent**: 角色信息页增强
  - 多角色选择器，支持在存活角色间切换
  - 显示角色所属服务器
  - 支持查看已故和归隐角色

- **Agent**: 服务器切换改进
  - 切换服务器时正确检测设备注册状态
  - 返回 `needs_device_registration` 和 `needs_character_creation` 标志
  - 显示该服务器上的历史角色列表

### Changed

- **Agent**: 版本号从 0.0.15 升级到 0.0.16
- **Config**: `CharacterConfig` 新增 `server_url` 和 `status` 字段
- **Config**: `Config` 新增 `characters` 数组存储角色历史

### Fixed

- **Agent**: 修复服务器切换时的 RwLock 使用错误（identity 不是 RwLock）

---

## [0.0.9] - 2025-03-21

### Fixed

- **Server**: 修复生产环境部署问题
  - 修复空 Token 问题：当 `ADMIN_READ_TOKEN` 或 `ADMIN_WRITE_TOKEN` 环境变量为空字符串时，现在会正确自动生成随机 Token
  - 添加数据库迁移自动执行：容器启动时自动执行 `/app/migrations/*.sql` 迁移文件

### Added

- **Scripts**: 新增 `scripts/version-bump.sh` 版本管理脚本
  - 自动检测 crate 变更并升级版本号
  - 支持 `--pre-commit` 模式在提交时自动运行

### Changed

- **Server**: 版本号从 0.0.7 升级到 0.0.9
- **Config**: `config.rs` 中 Token 读取逻辑增加空字符串过滤
