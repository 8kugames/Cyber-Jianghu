
# Cyber-Jianghu 更新日志

本变更日志记录每次重要提交的汇总信息和影响面。

---

## [Unreleased]

### Fixed — 联调测试问题修复

- **Agent**: 429 限流 early-exit — `decision.rs` retry loop 检测 429/rate_limit 立即中断重试，防止 OOM (exit 137)
- **Agent**: 自引用 target_agent_id 校验 — `rule_engine/engine.rs` 拒绝 target_agent_id == agent_id 的定向动作
- **Agent**: 经历日志补全 — `soul_cycle.rs` 遍历 subsequent_intents 逐条记录 (pipe_seq 递增)，修复前端无法看到多 intent pipeline 的问题
- **Agent**: 多 intent pipeline 展示 — `soul_cycle.rs` 将完整 pipeline actions 序列化到 `final_action_data`，`final_action_type` 显示为 "动作1 → 动作2 → 动作3" 格式，SoulCycleRecorder/IntentHistory/Server 上报三链路同步

### Added — 角色生成姓氏多样性

- **Agent**: `character_register.rs` 随机百家姓 index 注入 prompt，打破 LLM 对高频姓氏的趋同

### Changed — 多 Intent 输出引导强化

- **Agent**: `prompt_templates.yaml` 移除"大多数情况只需要 1 个动作"，改为鼓励 LLM 一次性规划 2-5 步连续动作
- **Agent**: 示例从 3 单 + 1 多改为 1 单 + 4 多，覆盖生存/探索/社交场景组合
- **Server**: `initial_inventory.yaml` 初始食物/水 3→8、银子 5→10

### Added — LLM 配置 Web UI 完善

- **Agent**: `FallbackModelConfig` 新增 `context_window_tokens` 字段，支持 per-model 上下文窗口配置（默认使用全局 32K）
- **Agent**: Web Panel LLM 配置页暴露全部缺失字段 — temperature、max_tokens、context_window_tokens、enable_streaming（基础区），summary_trigger_ratio、keep_recent_turns、idle_rotate_threshold、enable_thinking、fallback_models（高级区，collapsible）
- **Agent**: `llm_config_to_info()` + `apply_llm_update()` helper 函数消除 handler 逐字段构造代码重复
- **Server**: LLM 配置页新增 `context_window_tokens` 字段
- **Server**: `config_llm.rs` + `llm_loader.rs` 双 LlmConfig struct 同步增加 `context_window_tokens`

### Changed — 语义去重合并到 Layer 3

- **Agent**: Layer 2.5（`validate_semantic_dedup` 独立 LLM 调用）合并到 Layer 3 单次 LLM 调用，节省 1 次往返/tick
- **Agent**: `RejectionType` 新增 `SemanticRepeat` 变体，`build_validation_prompt()` 条件注入去重指令
- **Agent**: `OBSERVER_SYSTEM_PROMPT` 补充 `semantic_repeat` 拒绝类型和去重审核原则

### Changed — 截断审计与清理

- **Agent**: 日志截断值 × 10（`streaming.rs`、`direct_client.rs`、`tool_loop.rs`、`thinking_log.rs`、`soul_cycle.rs`）
- **Agent**: 移除非日志截断 — `outcome.rs` 完整失败原因、`relationship_narrative.rs` 完整描述、`soul_cycle.rs` 完整 dream 文本、`biography.rs` 完整叙事、`llm_config.rs` 完整 persona 描述
- **Agent**: `conversation.rs` 移除 `truncate_str()` 函数和 `"不超过500字"` prompt 限制
- **Agent**: `prompt.rs` `sanitize_for_prompt()` 移除 500 字符截断，仅保留模板转义
- **Agent**: `DEFAULT_SUMMARY_TRIGGER_RATIO` 0.8 → 0.75

### Changed — 代码质量与模块化重构

- **Agent**: `lifecycle.rs` 2502行 → 8文件模块目录（`callbacks`, `context`, `death`, `helpers`, `reporting`, `soul_cycle`, `tick` + `mod.rs`），800行上限
- **Agent**: `handlers.rs` 5166行 → 15个子模块文件（`basic`, `biography`, `character_helpers`, `character_info`, `character_register`, `config`, `discovery`, `lifespan`, `llm_config`, `memory`, `multi_character`, `relationship`, `soul_cycle`, `tick_notify`, `validate`），800行上限
- **Server**: `dashboard.rs` 1366行 → 6个子模块文件（`agents`, `experience`, `maintenance`, `stats`, `status_config`, `types`）
- **Agent**: 生产代码 `lock().unwrap()` → `lock().expect("...")` 全量覆盖（agent 87处 + server 13处 + 其余 26处，共 126处）
- **Server**: `255.0` 硬编码 → `DEFAULT_STATUS_MAX_VALUE` 命名常量，消灭 attributes.rs 遗留魔法值
- **Agent**: cargo fmt 全工作区格式化（expect() 行长度重排）

### Changed — 地魂 Tool Loop 提取 + Alias 清除

- **Agent**: `run_tool_loop` + `forced_text_exit` 从 `DirectLlmClient` 私有方法提取到 `soul/earth/tool_loop.rs` 共享层。`LlmClient` trait 新增 `send_chat_exchange` 用于模式无关原始消息交换
- **[BREAKING] Agent+Server**: 移除全链路 alias 容错机制 — `ActionAliasMap`/`FieldAliasMap`/`EntityAliasMap`/`EntityTranslationRegistry` 清空，`translation.rs` 仅保留空壳。LLM 必须输出精确 action_type、field name 和 ID，错误值由 ReflectorSoul 拒绝反馈学习
- **Server**: 移除 `normalize_action_data` 函数及所有调用点，`validate_by_rules`/`validate_teach_recipe` 直接使用 `intent.action_data`
- **Server**: `actions.yaml` 5个动作的 `target_agent_id` alias 还原为规范字段名

### Fixed — 联调诊断与稳定性

- **Agent**: 429 circuit breaker 1h 自动恢复 — `disabled_models` 从 `HashSet<usize>` 改为 `HashMap<usize, Instant>` 记录禁用时间戳，冷却期 3600s 后自动 re-enable。streaming fallback 路径补充 429 检测
- **Agent**: `reasoning_content` SSE 捕获 — `OpenAIDelta` 新增 `reasoning_content` 字段，`StreamChunk::ReasoningDelta` + `StreamAccumulator` 累积推理内容，`into_parts()` 返回 3-tuple（content, tool_calls, reasoning_content）
- **Agent**: 空响应 reasoning 兜底 — content 为空但 reasoning_content 非空时用 reasoning 内容作为输出
- **Agent**: SSE 流式 tool_call `arguments: null` 反序列化修复 — `StreamToolCallFunctionDelta` 的 name/arguments 字段新增 `deserialize_null_as_default`，将 null 映射为空字符串。之前 LongCat 首个 tool_call chunk 发送 `arguments: null` 导致整个 chunk 被 serde 静默丢弃
- **Agent**: circuit breaker fallback 跳过已禁用模型 — `call_with_fallback`/`call_streaming_with_fallback` 循环中跳过 `disabled_models` 中的模型；空响应也触发 `disable_model`
- **Agent**: target_agent_id 不可见修复 — 附近实体列表加入 UUID，LLM 能正确填写 target_agent_id
- **Agent**: 叙事化拒绝反馈 — 所有 ReflectorSoul/EarthSoul 拒绝文本改为叙事风格，不再暴露规则数字
- **Agent**: 删除 `max_consecutive_follow` 配置和 follow 循环特殊拦截（语义去重已覆盖同场景）
- **Agent**: 删除行为锁定警告（`get_repetition_warning` + `action_history` 重复检测），ReflectorSoul 硬拦截足够
- **Agent**: 联调诊断修复 — LLM chaos 主动轮换、prompt-estimate 可观测性日志、空响应诊断日志
- **Server**: `INTENT_BATCH_MAX_RETRIES` fallback 3→12，与 `game_rules.yaml` 默认值对齐

---

## [0.1.1 ~ 0.1.116] — 认知架构完善 (2026-04 ~ 2026-05)

涵盖 server 0.1.1 ~ 0.1.116 / agent 0.1.1 ~ 0.1.135 / protocol 0.1.43。从实时架构改造到完整认知引擎的全量迭代。

### Changed — 联调测试 0515 优化

- **Agent**: `LoopGuard` 渐进策略：第 1 次重复 tool call → 注入警告到 tool result，第 2 次重复 → 截断。默认阈值 `max_same_tool_consecutive` 3→2，`max_total_calls` 10→6
- **Agent**: Prompt 强化 tool/action 边界区分 — 在 YAML 模板、lean prompt `tool_calling_guidance`、action index 头部三处明确声明工具名不是动作名，防止模型把 `query_world` 当 action_type
- **Agent**: Action Index 从 `name+description` 降为 `name-only`，描述通过 `get_action_detail` 按需查询。Prompt actions 段从 ~1146 tokens → ~60 tokens（-95%）

### Added — Token 优化：注意力门控 + Tool-First 架构

- **Agent**: `TokenOptimizationConfig` 配置模块，所有参数外部化（`agent.yaml` 的 `token_optimization` 段），默认 `enabled: true`
- **Agent**: `WorldStateStore` 组件 — Agent 侧 WorldState 本地落存，prev/curr 双版本，供 Delta Engine 做增量检测
- **Agent**: `DeltaEngine` — 纯规则 prev vs curr 对比，5 类变化检测（survival/location/inventory/entities/skill），数据驱动阈值
- **Agent**: `AttentionController` — 两阶段过滤（规则自动聚焦 + LLM 排序占位）产出 `FocusSummary`
- **Agent**: Lean Prompt 模式 — 人魂 prompt 从完整 WorldState + 动作描述 → FocusSummary + Action Index + Skill Index
- **Agent**: 3 个新 EarthSoul tool calling 工具：`get_action_detail`、`query_world`、`list_skills`，按需取用详情
- **Agent**: `token_tracking.rs` 扩展 — `LlmComponent` 枚举 + `ComponentMetrics` 结构体，按组件维度追踪 token 消耗

### Changed — Token 优化：ReflectorSoul 重试循环优化

- **[BREAKING] Agent**: `ReflectorSoul` 验证流程从 13 轮重试循环改为固定流程：generate → validate → self_correct once → chaos_fallback。`token_optimization.enabled=true` 时 `max_retries=1`，`false` 时保持原 `max_retries=12`
- **Agent**: `lifecycle.rs` 新增 `self_correct_intent()` 方法，复用 decision callback 进行一次自我修正

### Changed — Agent 统一三层审查入口

- **[BREAKING] Agent**: `ReflectorSoul` 三层审查成为运行时唯一入口，`Cognitive` 主循环、`Claw` WebSocket 验证、HTTP `/api/v1/validate` 统一走同一 `Validator::validate(ValidationRequest)` 链路
- **[BREAKING] Agent**: `ValidationRequest` 新增 `runtime` 上下文，显式携带 `GradedValidationConfig`、连续 `follow` 计数与上限，避免不同运行模式各自拼装隐式校验条件
- **Agent**: `Claw` 验证任务接入实时 `WorldState` 与最近 `GameRules`，不再以 `world_state=None` 退化为 LLM-only 校验
- **Agent**: HTTP `/api/v1/validate` 接入当前 `WorldState` 与 `GameRules`，与主生命周期保持同构
- **Agent**: 执行失败反馈继续通过统一 rejection 通道回灌人魂，不再保留平行的发送前规则拦截分支

### Changed — PromptTemplateConfig YAML→JSON 重构

- **[BREAKING] Protocol**: `PromptTemplateConfig` 从 agent crate 迁移至 protocol crate（`cyber_jianghu_protocol::types::prompt_template`）。agent crate 的 `prompt_template.rs` 改为 re-export + 本地 YAML loading fallback
- **Protocol**: `ConfigUpdate` 新增 `content_hash: Option<String>` 字段，`#[serde(default)]` 向后兼容
- **Protocol**: `PromptTemplateConfig::to_json_bytes()` 两步序列化保证 canonical JSON（`to_value()` → BTreeMap 自动排序 → `to_vec()`），消除 HashMap 迭代顺序不确定性
- **Server**: prompt_templates 热重载路径重构 — YAML→`PromptTemplateConfig`→canonical JSON→SHA256 hash→写入 `AppState.prompt_template_cache`→广播 JSON ConfigUpdate
- **Server**: 启动时预加载 prompt_templates 到缓存（`TickScheduler::preload_prompt_templates()`），消除首个 Agent 连接时缓存为空的时序窗口
- **Server**: WS 连接时下发 prompt_templates ConfigUpdate（补齐之前 game_rules/world_building_rules/skills 有但 prompt_templates 没有的 gap）
- **Agent**: WS 接收路径从 YAML 字符串改为 JSON（`from_json_value()`），彻底消除 agent 端 `serde_yaml` 在 Linux Docker 中的解析故障
- **Agent**: hash skip 优化 — `ConnectionState.prompt_template_hash` 记录已接收 hash，相同内容跳过更新；hash 记录与 JSON 解析结果解耦，防止解析失败时重试风暴
- **Agent**: RuleEngine `prompt_config` 改为 `Arc<RwLock>` 共享状态（`SharedPromptConfig`），WS 回调同时更新 CognitiveEngine + RuleEngine reject 反馈模板
- **Agent**: 回调类型从 `Fn(String)` 改为 `Fn(PromptTemplateConfig)`，消除 YAML→String 中间态

### Changed — SKILL.md 元认知行为框架重构

- **[BREAKING] Server**: SKILL.md 系统推翻重做 — 7 个 RPG 技术技能（sword-basic, unarmed-basic, stealth, qi-meditation, first-aid, herbalism, bargaining）替换为 5 个元认知行为框架（social/trust-reading, social/conflict-navigation, cognitive/risk-assessment, cognitive/resource-planning, survival/situational-awareness）。已掌握旧技能 ID 的 Agent 在 SkillRegistry 中查不到对应定义，broadcaster 静默过滤
- **[BREAKING] Server**: 技能习得机制从显式"研读" action 改为经验阈值自动触发。Agent 执行 action 成功后按 category 累计计数，达到 `game_rules.yaml` 中 `skill_acquisition` 配置的阈值时自动触发 `SkillLearned`
- **Server**: `AgentState` 新增 `action_counts: HashMap<String, i32>` 字段，持久化到 JSONB `attributes._action_counts`。`#[serde(default)]` 兼容旧数据
- **Server**: 连接时全量技能推送改为按 Agent 已掌握技能过滤推送
- **Server**: `realtime.rs` 新增技能习得后增量推送 `ConfigUpdate` 给 Agent
- **Agent**: `skill_cache` 改为内存 + 本地文件（`skill_cache.json`）双层持久化，启动时从文件加载，运行时从 Server 推送更新后同步写入
- **Agent**: `engine_prompts.rs` 和 `skill_tool.rs` 删除文件系统读取逻辑，统一从 `skill_cache` HashMap 读取
- **Agent**: `EarthToolContext` 移除不再使用的 `config_dir` 字段

### Added

- **Agent**: 纪传体传记自动生成 — 角色死亡时 fire-and-forget 触发 LLM 生成传记，写入 character.yaml 并回传 server。核心逻辑从 HTTP handler 提取为 `generate_biography_for_agent()` 共用函数

### Added — 托梦显式 Intent 引用

- **Protocol**: `DreamMarker` 结构体 + `Intent.dream_marker` 字段 — 照搬 `chaos_marker` 模式，全链路追踪"此 intent 受托梦影响"
- **Protocol**: `FinalIntentReport` 补齐 `chaos_marker` + `dream_marker`（修复前端 chaos badge dead code）
- **Agent**: `lifecycle.rs` 捕获 `consume_dream()` 返回值 → 打标本 tick 全部 `all_raw_intents`
- **Server**: `AgentAction` + `agent_action_logs` 新增 `dream_marker JSONB` 列，`processor.rs` 提取并持久化
- **Server**: migration `020_dream_marker.sql`
- **Frontend**: 两个面板（agent panel + admin dashboard）新增"受托梦影响"紫色 badge 渲染
- **Config**: `prompt_templates.yaml` 新增 `dream_marker_thought: 50` 截断配置（数据驱动）

### Added — 记忆叙事合成

- **Agent**: 记忆叙事合成 — 高重要性事件经 LLM 批量叙事加工后写入情景记忆，解决"无意义事件进入长期记忆"问题
  - `CognitiveEngine::synthesize_memory_narrative()`: 人魂处理叙事合成，每 Tick 最多一次 LLM 调用
  - `prompt_templates.yaml` 新增 `memory_narrative` section: `min_events`、`max_events_per_tick`、`max_narrative_len`、`min_narrative_len`、`temperature`、`prompt`
  - `MemoryManager::process_events()` 重构: 所有事件写入工作记忆，高重要性事件（≥ episodic_threshold）经 LLM 叙事加工后写入情景记忆
  - 失败降级文本: `你一阵恍惚，似乎遗漏了一些重要的记忆。`（一字不差）
  - 配置驱动: 所有阈值/参数均从 `prompt_templates.yaml` 读取，零硬编码

### Changed

- **[BREAKING] Server**: `auto_rebirth_agent()` 从 UPDATE-in-place（回魂）改为 INSERT 新 agent（转世）— 旧 agent dead→retired，新 agent 全新 UUID + 初始状态 + 初始物品。事务包裹保证原子性
- **Agent**: rebirth 恢复时重新 open RelationshipStore（新 agent_id → 新 DB 文件），同步更新 CognitiveEngine 内部引用
- **Agent**: `max_tool_rounds` 外部化到 `prompt_templates.yaml` 的 `llm_parameters` 段，消除硬编码

### Added — 数据驱动重构

- **Agent**: EarthSoul tool calling 安全机制 (F1/F2/F3)
  - F1 ToolResultBudget: per-tool + aggregate 字配额，`.chars().count()` 统一 Unicode 安全截断
  - F2 LoopGuard: 连续调用检测，Warn→Terminate 升级机制
  - F3 Error Signaling: 工具执行错误格式化为 `[工具调用失败] 工具: X | 原因: Y`
  - `EarthSoulConfig` 配置驱动，`#[serde(default)]` 向后兼容，`enabled: true` 默认启用
  - `validate()` Fail Fast 校验，启动路径 + 热重载路径均调用
- **Agent**: `IntentBatchConfig` 配置外部化 — `max_intents_per_tick` / `max_retries` / `pipeline_execution_enabled` 从 `game_rules.yaml` 读取，消除硬编码魔法数字
- **Agent**: EarthSoul `validate()` 启动时 + 热重载时 Fail Fast 校验，非法配置立即拒绝

### Changed — 数据驱动重构

- **[BREAKING] Protocol**: 移除 `IntentBatchConfig::default()` 硬编码默认值，改为从 Server 配置下发。旧 Agent 未收到配置时使用编译期 fallback（不再独立决定批次参数）
- **[BREAKING] Server**: processor pipeline 展平记录 — 移除嵌套 `Vec<Vec<...>>`，audit log 直接记录扁平 Intent 执行结果

### Fixed

- **Agent**: 移除 `display_messages` 残留死代码（未使用函数 + 未使用 import）
- **Agent**: skill_view tool description 加强 skill_id 选择指引，引导 LLM 从已掌握技能列表选择
- **Server**: auto-rebirth handler 清理 agent_to_device_map 旧映射 + DashMap 旧缓存，防止幽灵映射
- **Agent**: 地魂工具池扩展至 6 个工具（3 个新增关系工具）
  - `get_relationship`: 查询与特定角色的关系记忆（支持 UUID 或名字查找，SQL 层过滤）
  - `list_relationships`: 列出所有关系概览，可选好感度范围过滤（SQL 层 WHERE）
  - `record_social_event`: 主动记录社交互动和好感度变化（delta clamp [-50, 50]）
  - `RelationshipStore` 新增 `find_relationship()` / `list_relationships_filtered()` 方法 + `target_name` 索引
  - `EarthToolContext` struct 替代 `from_engine()` 签名膨胀模式
- **Agent**: auto-rebirth 闭环修复 — spawn task 解析 `new_agent_id` 传入 main loop，rebirth_notify handler 用 new_id reconnect（之前 nil reconnect 导致永久挂起）；同步更新 `HttpApiState.agent_id`（P2 修复）
- **Agent**: 地魂 tool-calling 不触发 — 三层根因修复
  - summary LLM 调用失败时降级为 `force_truncate_to_recent()`（避免 227 轮对话历史无限堆积）
  - tool-calling 模式下历史轮次限制走 `truncation("tool_calling_history_turns", 8)` 配置驱动
  - 删除 `tool_system_suffix` 硬编码，统一到 `tool_calling_guidance` 单条数据驱动路径
- **Agent**: `search_memory` 与 `recall_archived` 实现去重
  - `recall_archived` 改用 `recall_recent_archived()` 跳过语义搜索，按时间倒序返回
  - `recall_archived` 工具 `query` 参数改为 optional

- **Agent**: Token 统计全零修复 — 单模型场景 `DirectLlmClient` 流式路径缺少 `UsageTrackingStream` 包装，导致 `token_cost_count.tmp` 始终全零
  - `DirectLlmClient` trait impl 的 `complete_streaming` / `complete_conversation_streaming` 加入 `UsageTrackingStream` 包装
  - 非流式 `send_request_once` 当 API 不返回 usage 时用字符长度估算 token
- **Agent+Protocol**: Session triage LLM 兜底分流修复 — 由硬编码二段式改为配置阈值驱动的三段式（urgent/batch/ignored），并区分“超时/调用失败”的兜底 reason；未配置 event_triage 或阈值无效时禁用即时事件处理
- **Agent**: max_tokens 自适应 — API 返回 400 且错误体包含 max_tokens 限制时自动学习并重试
  - `LEARNED_MODEL_LIMITS` 全局状态持久化到 `~/.cyber-jianghu/model_limits.json`
  - 正则提取 4 种错误格式（NVIDIA NIM / DashScope 中英文 / OpenAI / Anthropic）
  - 安全约束：单次重试（无递归）、限制必须 < 配置值、范围 [100, 200000]
- **Agent**: 新增 `FallbackModelConfig`（per-model max_tokens）支持同一 provider 下不同模型独立配置
- **Agent**: `LlmClient` trait 新增 `provider_info()` 方法（统一 provider + model 信息获取）
- **Agent**: MemoryManager 在 Agent 生命周期与 HTTP API handlers 间共享（之前各自创建独立实例，导致 `/api/v1/memory/recent` 等接口始终返回空）
- **Agent**: 社交事件名字解析修复 — `social.rs` 名字解析链路 `name_map → RelationshipStore → "陌生人"`，防止非附近实体的已有真名被覆写回"陌生人"（根因：`entities` 仅含当前在线附近实体，离线/不在范围内时直接 fallback "陌生人"）
- **Agent Panel**: 关系列表从全宽条改为紧凑卡片网格，详情从侧边抽屉改为居中 Modal
- **Agent Panel**: Modal 新增 target Agent ID 展示（可选中复制）和密语沟通记录（从 soul-cycles 提取）
- **Agent Panel**: 三个"加载更多"按钮追加分页时添加 disabled + loading 文字反馈
- **Agent Panel**: SSE `agent_died` 事件触发后立即关闭连接并停止重连，避免死亡后重复弹窗

### Added — candle 升级 0.9.2 → 0.10.2

- **Agent**: auto-rebirth 配置开关 — `RuntimeConfig.auto_rebirth: bool`（默认 true），运行时可通过 `GET/POST /api/v1/config/auto-rebirth` 热切换，Web 面板 create.html / character.html 提供 toggle UI（解决 CPU 后端 `index_select` 不支持 F32 的运行时错误），DType 恢复 F32 原生精度；消除懒加载死锁（search/search_similar 先尝试 embed 触发初始化）
- **Agent**: Session Triage 每日摘要写入 episodic memory（之前仅日志输出，未持久化）
- **Agent**: auto-rebirth spawn 增加重试机制（最多 3 次，间隔 30s），最终失败走 120s 超时兜底 reconnect
- **Server+Agent**: auto-rebirth 重构 — 转世重生创建新 agent_id，旧 agent 保持 dead 状态（死亡/归隐语义分离）
- **Protocol**: `EventTriageConfig` 新增 `daily_summary_importance` 字段（数据驱动，消除硬编码）

- **Server**: multi-intent pipeline 失败通知修复
  - Subsequent intent 执行失败时正确发送 `ExecutionResult(success=false)`
  - Subsequent intent persist 失败时正确发送 `ExecutionResult(success=false)`（之前静默丢失通知）
  - Subsequent intent 死亡检查时正确发送 `ExecutionResult(success=false)`（之前无通知）
  - Subsequent intent 失败时正确清理 whisper session（避免 session 泄漏）
  - 所有 subsequent intent 必有且仅有一条 ExecutionResult 通知

### Added

- **跨Agent传承 Layer 2**: 共享教训库（`public_lessons` 表 + WorldState 下发）
  - Server: 死亡事件按 cause 聚合，达到阈值后自动生成教训条目
  - Protocol: `WorldState.lessons_learned: Vec<PublicLesson>`（cause/lesson/death_count/avg_survival_ticks）
  - Agent: lifecycle 注入"前人教训"到 DecisionContext 供认知引擎参考
  - 配置: `game_rules.yaml lesson.threshold`（默认 3）/ `lesson.max_broadcast`（默认 5）
  - 迁移: `015_public_lessons.sql`

- **Protocol**: `ServerMessage::AgentDied` 新增 `metadata: Option<Value>` 字段（跨Agent传承 Layer 1）
  - 携带死亡时属性快照（hp/hunger/thirst/sanity）、birth_tick、survival_ticks、death_tick、cause

- **Agent+Server**: 每日 LLM 日志摘要提交 Server 存档
  - Protocol: `ClientMessage::DailySummary { game_day, summary }`
  - Server: `agent_daily_summaries` 表（迁移 `016_agent_daily_summaries.sql`），UPSERT，Server 注入 `created_at` 时间戳
  - Chronicle: `collector.rs` LEFT JOIN `agent_daily_summaries`，每日摘要拼接注入 `AgentSummary.narrative`
  - Agent: lifecycle 调用 `client.send_daily_summary()`，指数退避重试（`max_retries`，默认 3）
  - 配置: `game_rules.yaml daily_summary.max_retries` / `daily_summary.ttl_ticks`（默认 10080 = 7 游戏日）
  - `#[serde(skip_serializing_if = "Option::is_none")]` 兼容旧客户端
  - Claw 模式 `DownstreamMessage::AgentDied` 同步透传 metadata
  - **展示**: Agent HTTP API `GET /api/v1/memory/daily-summaries`（玩家查看个人摘要）；Server Admin API `GET /api/dashboard/agent-daily-summaries` + Admin 页面 `admin/agent-daily-summaries.html`（管理后台查看所有摘要）

- **Protocol**: `LifespanRules` 新增 `starting_age: u8` 字段（默认 18）
  - 重生角色 age 从 0 改为配置的 starting_age，避免天魂误判"婴儿"
  - `compute_starting_age_ticks()` 函数从 game_rules.yaml 读取并 clamp

- **Agent+Server+Protocol**: 混沌降级结构化标记
  - Protocol: `ChaosMarker` 枚举（`Sanity { sanity }` / `LlmQuotaExhausted { consecutive_failures }`），`Intent.chaos_marker: Option<ChaosMarker>`
  - Agent: chaos 生成器数据驱动重构（从 `available_actions` + `required_fields` 解析，不再硬编码 action_weights）
  - Agent: `llm_chaos_active` 时抑制认知 fallback "休息"，纯用 chaos intents
  - Server: `agent_action_logs.chaos_marker` JSONB 列（迁移 `017_chaos_marker.sql`），结算时序列化 `intent.chaos_marker`
  - 前端: agent-web + server-web 渲染红色"陷入混乱"徽章（`cm.type === 'Sanity'` / `'LlmQuotaExhausted'`）

- **Agent**: 社交事件扩展 — `process_social_events()` 支持 PublicMessage 和 PrivateDialogue
  - 之前仅 SocialInteraction（物品转移）触发好感度更新
  - 现在说话（speak）和密语（whisper）也纳入 LLM 好感度评估
  - 密语事件 metadata 补充 `from_agent_id` + `action: "whisper"`

- **Agent**: bge-small-zh-v1.5 嵌入模型 Docker 集成（Semantic Memory Docker Plan A）
  - Agent Dockerfile 运行阶段自动下载模型三文件（~100MB）
  - 使用 hf-mirror.com（可通过 `--build-arg HF_MIRROR=` 覆盖）

### Breaking Changes

- **`POST /api/v1/agent/auto-rebirth`**: 请求体从 `{ agent_id }` 变更为 `{ device_id, auth_token, old_agent_id, name, system_prompt }`。响应新增 `new_agent_id`、`old_agent_id`。auto-rebirth 现在创建新 agent_id 而非重置旧 agent_id，旧 agent 保持 dead 状态。

### Fixed

- **Agent**: Ghost Agent — 已死 Agent（`rebirth_delay_ticks == 0`）继续提交 Intent
  - `lifecycle.rs` 死亡等待逻辑缺失 `else { continue; }` 分支

- **Agent**: Soul Cycle DB 停写 — tick 后不再追加
  - `record_renhun` recorder 初始化失败时静默跳过，现增加 `error!` 日志

- **Agent**: ChaosGenerator 不触发 — S<30 持续 22+ tick 但 0 次触发
  - `debug!` 升级为 `info!`，增加参数（sanity/threshold/chaos_action）

- **Agent**: OutcomeMemory 100% success — failure 记录路径缺失
  - `handler.rs` handler 层拒绝（rate limit/agent dead/queue full 等）现发送 `ExecutionResult(success=false)`
  - Agent 端 OutcomeMemory 可记录失败结果用于学习

- **Server**: Session Lock — 对话 session 未正确释放（302 次 "already in dialogue"）
  - Whisper intent 执行后立即 `close_session()`，避免同 tick AlreadyInDialogue
  - `DialogueManager` 新增 `close_session()` 方法

- **Agent**: Token Tracking persist_and_reset 从未调用
  - 从 tokio::select! 宏体内移至每个 tick 结束后的正确位置
  - `fs::write` 改为 write-to-tmp + rename 原子写入

- **Server**: Event Queue 溢出 — 6 Agent 同地 30 events/tick，队列容量 32
  - `connection.rs` channel full 时增加 agent_id/agent_name warn 日志

- **Agent**: Stream 降级往返 — LongCat-Flash-Lite 每次 non-stream 400
  - `should_fallback()` 增加 context-length 错误短路，不再无意义重试
  - `FallbackLlmClient` 400 错误时 warn 建议 `prefer_stream: true`

- **Agent**: Fallback 模型追踪 — `OpenAIResponse` 新增 `model` 字段
  - 非流式响应检测实际 model 与请求 model 是否一致，记录 info 日志

- **Agent+Server**: 记忆/关系系统事件丢失 — Reactive WS events_log 硬编码为空

- **Admin**: experiences.html 叙事列 HTML 破损修复（`<td>` 缺少 `</td>`）

### Changed

- **Agent**: 感知增强配置更新
  - `narrative_config.yaml` 增加 Episodic 噪声过滤、对话污染清理、物品盲区描述、进食紧迫性叙事
  - `prompt_templates.yaml` 相关模板段更新

- **Agent**: 地魂记忆回溯工具接入
  - 在 `EarthToolExecutor` 中实装了 `search_memory` 和 `recall_archived`
  - 通过 `Arc<tokio::sync::RwLock<MemoryManager>>` 解决了记忆管理器的并发所有权问题
  - 将 `MemoryManager` 实例注入到 `CognitiveEngine` 和地魂工具池中
  - 支持 LLM 在思考过程中按需检索情景与语义记忆

### Removed

- **Agent+Server**: 天道无为生存架构重构 — 移除所有天道干预式生存机制
  - Agent: `lifecycle.rs` 移除 `survival_warnings`（hunger/thirst/HP 阈值警告注入）和 `sanity_warning`（精神状态注入）
  - Agent: 交易议价提示独立为 `trade_hints`（经济引导，非生存干预）
  - Server: `game_rules.yaml` 移除 `critical_threshold` / `critical_attack_threshold` / `hp_critical_threshold` / `hp_force_flee_threshold`
  - Protocol: `GameRules` 移除对应 4 个字段
  - Protocol: `SurvivalConfig` 简化为仅 `rebirth_delay_ticks`
  - Agent: `config.rs` 移除 `survival_threshold()` / `critical_attack_threshold()` / `hp_critical_threshold()` / `hp_force_flee_threshold()` accessor
  - Prompt: `prompt_templates.yaml` 移除 `survival_warnings` 和 `sanity_warnings` 模板段
  - 替代方案: Agent 通过 `WorldState.attribute_descriptions`（体感叙事，来自 `narrative_config.yaml`）自主感知状态

### ⚠️ Breaking Changes

- **Agent**: `MemoryBackend::add()` 签名破坏性变更
  - 旧: `async fn add(&mut self, MemoryEntry) -> Result<()>`
  - 新: `async fn add(&mut self, &mut MemoryEntry) -> Result<i64>`
  - 返回值从 `()` 改为插入记录的 DB ID（-1 表示跳过/过滤）
  - `add_batch` 默认实现改为 `for mut memory in memories` 消费所有权
  - 影响: WorkingMemoryBackend / EpisodicMemoryBackend / SemanticMemoryBackend 全部适配

- **Protocol**: `GameRules` 移除 4 个生存阈值字段
  - 删除: `survival_threshold` / `critical_attack_threshold` / `hp_critical_threshold` / `hp_force_flee_threshold`
  - 影响: 旧 Agent 收到新 GameRules JSON 时这 4 个字段被 serde 静默忽略（无 `deny_unknown_fields`）
  - 影响: 消费 `survival_threshold()` 等 accessor 的代码需删除对应调用

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

- **Agent+Server**: HP 逃逸生存机制
  - Server: `game_rules.yaml` 新增 `hp_critical_threshold`(30) / `hp_force_flee_threshold`(15)
  - Protocol: `GameRules` 新增对应字段 + `SurvivalConfig` 结构化参数
  - Agent: `lifecycle.rs` 在 survival_warnings 增加 HP 低阈值检查，分濒死/危险两级
  - HP < 15 注入濒死警告（最高优先级），HP < 30 注入危险警告
  - `prompt_templates.yaml` 新增 `hp_critical_warning` / `hp_force_flee_warning` 模板

- **Agent**: 流式模式全局优化
  - `DirectLlmClientConfig` 新增 `prefer_stream: bool`（默认 false）
  - `send_request()` 短路: `prefer_stream=true` 时直接走 streaming，跳过 400 降级
  - `build_fallback_client()` 从 `config.llm.enable_streaming` 读取
  - 向后兼容：缺失时默认 false

- **Agent**: 天魂角色名排除
  - `PersonaInfo` 新增 `name: Option<String>` 字段
  - ReflectorSoul 验证 prompt 增加"角色：{name}" + 穿越排除说明
  - 防止角色名（如"张三丰"）被误判为穿越概念

- **Agent**: `CYBER_JIANGHU_DATA_DIR` 数据持久化
  - `Config::default()` 从环境变量读取 `servers_dir`
  - `update_game_rules()` 使用环境变量定位 `actions.json`
  - Docker 容器内数据写入挂载卷，避免 `down` 时丢失

### Fixed

- **Agent**: `lifecycle.rs` HP 最大值 key 错误 (`max_hp` → `hp_max`)
  - Server JSONB 存储格式为 `{attr}_max`，即 `hp_max`
  - 修复前 `unwrap_or(100)` 静默掩盖，HP 警告始终显示 X/100

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

## [0.1.0] — 实时架构改造 (2026-04)

Tick 批处理模式全面退役，Intent 实时化。版本 0.0.x → 0.1.0。

### Breaking Changes

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

- **Protocol**: `ServerMessage::ExecutionResult` 实时执行反馈
  - Agent 端通过 `try_receive_execution_result()` 获取（watch channel，非阻塞）

### Changed

- **Server**: Tick 退化为纯时钟 — 每周期：发送 TickBoundary（触发衰减）→ 广播 WorldState，不再收集/结算 Intent
- **Server**: handler.rs Intent 路由改造 — 删除 accepting_tick_id 检查、IntentManager 写入，非阻塞 `try_send` 入队 IntentWorker
- **Agent**: lifecycle.rs 主循环统一发送路径 — `send_immediate_intent()` 走 `send_intent()`，即时事件 binding 使用 `intent_sender()`
- **Agent**: websocket.rs 后台任务单一 recv 循环 — `intent_rx.recv()` 统一处理 `ClientMessage::Intent` 和 `ClientMessage::SoulCycleReport`

### Removed

- **Agent**: 双通道系统（`immediate_msg_tx` / `immediate_msg_rx`）— `send_immediate_message()` / `immediate_msg_sender()` 方法
- **Server**: IntentManager 整条链路 — `IntentManager` type alias / `create_intent_manager()` / `take_intents_for_tick()` / `AppState.intent_manager` 字段
- **Server**: TickScheduler 批处理字段 — `closed_dialogue_records`、`execution_summaries`、`dialogue_manager`

---

## [0.0.33 ~ 0.0.104] — 三魂架构与 OpenClaw 集成 (2026-03-23 ~ 2026-04-10)

涵盖版本: 0.0.33, 0.0.104。从双 Soul 架构到 OpenClaw 消息透传的完整集成。

### Added

- **Agent**: ActorSoul + ReflectorSoul 双 Soul 架构
  - 新增 `ReviewStore` 共享内存用于进程内审查通信
  - ActorSoul (行动之魂)：生成意图，执行行动
  - ReflectorSoul (反思之魂)：审查意图，世界观一致性审查（默认启用）
  - AgentBuilder 新增 `with_review_store()` 和 `with_reconnect_rx()` 方法

- **Agent**: ActorSoul 和 ReflectorSoul LLM 独立配置
  - 新增 `llm_reflector` 配置字段，支持独立配置 ReflectorSoul LLM
  - 新增 GET/POST `/api/v1/config/llm` 端点，GET `/api/v1/config/llm/providers` 端点
  - 配置变更通过文件监听自动热重载，API Key 格式验证和内存安全（zeroize），原子替换 + 备份回滚

- **Agent**: 审查系统默认启用 — Cognitive 和 Claw 模式均默认启用 ReflectorSoul，支持 Approved/Rejected/TimeoutApproved，超时自动批准（30s）

- **Agent**: 架构统一（COI 原则）— Cognitive 和 Claw 模式统一使用 AgentBuilder，移除 `Agent::new()`（改用 Builder）

- **Agent**: Server → OpenClaw 消息透传机制
  - Agent 实时转发 Server 下行消息给 OpenClaw（WebSocket）
  - 新增 `ServerErrorCode` 结构化错误码枚举
  - 新增 `DownstreamMessage` 变体：`ServerError`、`ServerDialogue`、`ServerGameRulesUpdate`、`ServerWorldBuildingRulesUpdate`、`MissedMessages`
  - 消息流转: `Game Server → WS Client → server_msg_callback → broadcast::Sender → WS Server → OpenClaw`

- **Agent**: WebSocket Server 安全限制 — 仅 localhost 连接、单连接限制、断开自动释放

- **Agent**: WebSocket Client 回调机制 — `set_server_msg_callback()` 方法

- **Agent**: WebSocket Tick 消息集成四阶段认知上下文 — `DownstreamMessage::Tick` 新增 `cognitive_context` 字段

- **Server**: agent_id → device_id 反向映射系统 — `AgentToDeviceMap` 类型维护角色到设备映射，解决 WorldState 广播找不到正确连接问题

### Changed

- **Agent**: CLI 移除 `--role` 和 `--target-endpoint` 参数，移除远程 Observer 模式
- **Agent**: HTTP Intent API 禁用 — 移除 `POST /api/v1/intent` 路由，强制 WebSocket 提交
- **Agent**: 配置文件新增 `config_path` 字段
- **Agent**: 版本号 0.0.29 → 0.0.33
- **Server**: WebSocket 连接管理改用 device_id 作为 key，支持同一设备管理多角色

### Fixed

- **Agent**: 修复单连接限制的竞态条件 — 拒绝第二个连接时不再错误释放第一个连接的 slot

### Removed

- **Agent**: 远程 Observer 模式相关代码 — `run_observer_mode()` / `fetch_pending_reviews()` / `process_review_remote()` / `--role observer` / `--target-endpoint`
- 过时设计文档: `docs/openclaw-cognitive-integration.md`、`docs/superpowers/` 下 3 个文件、`联调测试.md`

---

## [0.0.9 ~ 0.0.20] — 基础功能建设 (2026-03-21 ~ 2026-03-22)

涵盖版本: 0.0.9, 0.0.16, 0.0.20。多角色管理、认知上下文、设备认证等基础能力建设。

### Breaking Changes

- **Agent**: 移除 `--mode` 命令行参数，统一为 Claw 模式（默认）
  - 旧命令: `cyber-jianghu-agent --mode claw run`
  - 新命令: `cyber-jianghu-agent run`
- **Agent**: Intent API 响应格式变更 — 纯文本 → JSON `{"status": "submitted", "intent_id": "...", "tick_id": N, "action_type": "..."}`

### Added

- **Agent**: 多角色管理系统
  - `CharacterStatus` 枚举（Alive/Dead/Retired）跟踪角色状态
  - `GET /api/v1/characters` - 获取所有角色列表
  - `POST /api/v1/characters/switch` - 切换当前活跃角色
  - 支持已故和归隐角色的历史记录，每角色关联特定服务器

- **Agent**: Cognitive Context API (`/api/v1/cognitive`) — 四阶段推理结构：Perception → Motivation → Planning → Decision

- **Agent**: Web Panel 智能路由 — 首页根据服务器连通性和角色状态自动跳转，角色信息页支持多角色切换

- **Agent**: 服务器热切换 API — `POST /api/v1/config/server` 动态切换服务器地址，自动触发 WebSocket 重连

- **Server**: 设备认证系统 — `POST /api/v1/agent/connect` 设备注册获取 auth_token，WebSocket 连接需要 token 参数

- **Server**: Intent 全链路追踪 — 每个 Intent 分配唯一 `intent_id`，支持 `priority` 字段

- **Scripts**: `scripts/version-bump.sh` 版本管理脚本 — 自动检测 crate 变更并升级版本号

### Changed

- **Agent**: 重构决策模式 — 移除 `http` / `ws` / `cognitive` 模式区分，统一为 Claw 模式
- **Agent**: 版本号 0.0.7 → 0.0.9 → 0.0.15 → 0.0.16 → 0.0.20
- **Config**: `CharacterConfig` 新增 `server_url` 和 `status` 字段，`Config` 新增 `characters` 数组存储角色历史
- **Server**: `config.rs` Token 读取逻辑增加空字符串过滤

### Fixed

- **Agent**: HTTP API 死锁 — 注册回调中 RwLock 读锁未释放就尝试获取写锁 → 显式 `drop(old_id)`
- **Agent**: 服务器切换时的 RwLock 使用错误（identity 不是 RwLock）
- **Agent**: Docker 部署和数据库类型不匹配问题
- **Server**: 生产环境部署 — 空 Token 自动生成随机 Token + 数据库迁移自动执行
- **Server**: `get_agent_by_device_id` 函数未导出 — 添加到 `db/mod.rs` 导出列表

### Removed

- **Agent**: 过时的 OpenClaw 内联模式代码、`--mode` 命令行参数
