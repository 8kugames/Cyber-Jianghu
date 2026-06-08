# Cyber-Jianghu 更新日志

---

## [Unreleased]

### Fixed — 联调测试 0608

- 修复 MiniMax 等模型将 JSON 包裹在 thinking 标签内导致空响应 EOF：新增模型适配层，将标签剥离从通用 JSON 管线中分离
- Token 用量统计改为按实际活跃时长计算每小时均值（替代自然小时桶计数）
- 角色注册 Enum 字段校验放宽：`options` 仅作示例，不再强制匹配
- Dashboard 角色列表新增 `is_alive` 字段

### Added — 规则按需检索 (Rule-On-Demand)

- 规则注入从全量推送改为按需检索：system message 仅注入极简索引（~150 tokens），LLM 通过 EarthSoul `query_rules` 工具按类别查询规则全文
- Token 节省 ~825-1125 tokens/tick，规则区增长曲线从线性降为常数

### Added — 情绪-记忆联动

- 基于 Barrett 情绪建构论的核心情感系统：效价×唤醒度确定性计算，情绪增强记忆编码，心境一致检索偏置
- LLM 情绪构造、体感信号注入、全量 YAML 配置驱动

### Changed — ActionType 数据驱动化

- 动作系统全面数据驱动：`transmission`（广播/私语/静默）、`display_name`、`validator_kind`、`highlight_kind` 全部从 `actions.yaml` 读取，消灭硬编码字符串判断
- RelationshipStore PRAGMA 自动迁移，文件从 842 行降至 754 行

### Changed — 设备身份 v2 [BREAKING]

- 废弃 `/api/v1/agent/connect`，拆分为 `/api/v1/device/verify`（只读）+ `/api/v1/device/register`（Server 生成 device_id），从协议层消除撞库
- TOCTOU 修复，WebSocket 401 自动恢复，stale yaml 死循环修复

### Fixed — Auto-Rebirth 归隐语义 [BREAKING]

- 已死亡角色不再被 auto-rebirth 错误归隐。`retired` 状态语义专属玩家主动归隐，`dead` 角色转世后保持死亡标记

### Changed — Agent Web Panel SPA 重构

- 6 页碎片化 HTML 重构为 3 页 SPA（#/dashboard, #/characters, #/settings），CSS 从 3369 行压缩至 ~445 行

### Fixed — 三魂数据模型 [BREAKING]

- `FinalIntentReport` 新增 `pipeline_actions` 承载多意图完整视图，`action_data` 回归单 intent 语义

### Changed — Schema-Driven 角色生成 [BREAKING]

- 角色生成从 18 个独立字段改为 schema-driven：`FieldSpec` enum + `fields` 列表，Prompt 和校验共享同一 YAML schema

### Fixed — 联调测试 0515

- 429 限流 early-exit 防止 OOM，自引用 target_agent_id 校验，经历日志补全多 intent pipeline 展示

### Added — 角色生成姓氏多样性

- 随机百家姓 index 注入 prompt，打破 LLM 姓氏趋同

### Changed — 多 Intent 输出引导强化

- Prompt 改为鼓励 2-5 步连续动作，示例从 3 单 + 1 多改为 1 单 + 4 多，初始食物/水 3→8

### Added — LLM 配置 Web UI 完善

- Web Panel 暴露全部 LLM 配置字段（temperature、max_tokens、context_window_tokens、enable_streaming 等）

### Changed — 语义去重合并到 Layer 3

- 语义去重从独立 LLM 调用合并到 ReflectorSoul Layer 3 单次调用，节省 1 次往返/tick

### Changed — 截断审计与清理

- 日志截断值×10，移除非日志截断（outcome 完整失败原因、relationship 完整描述、biography 完整叙事等）

### Changed — 代码质量与模块化

- `lifecycle.rs` 2502→8 文件模块，`handlers.rs` 5166→15 子模块，`dashboard.rs` 1366→6 子模块
- `lock().unwrap()` → `expect()` 全量覆盖（126 处），`255.0` 硬编码→命名常量

### Changed — 地魂 Tool Loop 提取 + Alias 清除 [BREAKING]

- `run_tool_loop` 从 DirectLlmClient 提取到共享层，`LlmClient` trait 新增 `send_chat_exchange`
- 移除全链路 alias 容错机制，LLM 必须输出精确值，错误由 ReflectorSoul 拒绝反馈学习

### Fixed — 联调诊断与稳定性

- 429 circuit breaker 1h 自动恢复，SSE 流式 tool_call 修复，空响应 reasoning 兜底
- circuit breaker 跳过已禁用模型，叙事化拒绝反馈，target_agent_id 可见性修复

### Added — DeepSeek 前缀缓存调优

- 3 阶段数据驱动调优：Phase 0 测量（system_hash 维度）→ D8 reasoning 剥离 → D9 schema 规范化
- 所有阈值/开关由环境变量驱动，0 硬编码。13 个新测试

---

## [0.1.0] — 实时架构改造 (2026-04)

Tick 批处理全面退役，Intent 实时化。

### Breaking Changes

- 新增 `ServerMessage::ExecutionResult` 实时执行反馈，Agent 必须处理
- 删除 `IntentManager` 批处理缓存，Intent 直接入队 IntentWorker
- 删除 `accepting_tick_id` 校验，Agent 无需同步 tick_id

### Added

- `IntentWorker` 实时处理引擎：单消费者 MPSC channel，顺序处理 Intent + TickBoundary
- `AgentStateCache` (DashMap) 内存缓存：write-through 持久化，广播从缓存读取
- `StateProcessor` 单条 Intent 处理，保留完整 Saga 回滚

### Changed

- Tick 退化为纯时钟（衰减 + WorldState 广播），不再收集/结算 Intent

---

## [0.1.1 ~ 0.1.116] — 认知架构完善 (2026-04 ~ 2026-05)

涵盖 server 0.1.1 ~ 0.1.116 / agent 0.1.1 ~ 0.1.135 / protocol 0.1.43。

### Token 优化

- 注意力门控 + Tool-First 架构：WorldState 本地落存，Delta Engine 增量检测，AttentionController 两阶段过滤
- 3 个新 EarthSoul 工具：`get_action_detail`、`query_world`、`list_skills`
- ReflectorSoul 从 13 轮重试改为固定流程（generate → validate → self_correct once → chaos_fallback）

### 三层审查统一

- ReflectorSoul 三层审查成为运行时唯一入口，Cognitive/Claw/HTTP `/validate` 统一链路
- `ValidationRequest` 显式携带运行时上下文，避免不同模式各自拼装校验条件

### PromptTemplate YAML→JSON

- PromptTemplate 从 agent crate 迁移至 protocol crate，热重载走 YAML→JSON→SHA256→广播 JSON ConfigUpdate
- Agent 端彻底消除 `serde_yaml` 解析故障

### SKILL.md 元认知行为框架 [BREAKING]

- 7 个 RPG 技能替换为 5 个元认知行为框架（识人之明、进退之道、审时度势、未雨绸缪、见微知著）
- 技能习得从显式"研读"改为经验阈值自动触发

### 其他功能

- 纪传体传记自动生成（角色死亡时触发）
- 托梦 Intent 显式标记（`DreamMarker` 全链路追踪）
- 记忆叙事合成（高重要性事件经 LLM 加工后写入情景记忆）
- 跨 Agent 传承（共享教训库 + 死亡元数据携带）
- 每日 LLM 日志摘要 Server 存档
- 混沌降级结构化标记（`ChaosMarker`）
- 社交事件扩展（说话/密语纳入好感度评估）
- bge-small-zh-v1.5 嵌入模型 Docker 集成

### Breaking Changes

- `auto_rebirth` 从 UPDATE-in-place（回魂）改为 INSERT 新 agent（转世）
- `IntentBatchConfig` 默认值从编译期改为 Server 配置下发
- 即时事件架构重写：内存队列→SQLite 持久化 + Session Triage LLM 批量处理
- `POST /api/v1/agent/auto-rebirth` 请求体变更为含 device_id/auth_token/new_agent_id
- `MemoryBackend::add()` 签名破坏性变更（返回 DB ID）
- `GameRules` 移除 4 个生存阈值字段
- `LifespanRules` 删除 `ticks_per_year`，从 `time.yaml` 派生
- 移除 `TRADE` 动作类型及 230 行交易处理逻辑
- 寿命系统 Server 权威化

---

## [0.0.33 ~ 0.0.104] — 三魂架构与 OpenClaw 集成 (2026-03 ~ 2026-04)

- ActorSoul + ReflectorSoul 双 Soul 架构，独立 LLM 配置
- Server→OpenClaw 消息透传机制（WebSocket 消息流转）
- CLI 统一为 Claw 模式，移除远程 Observer 模式
- WebSocket 安全限制（localhost、单连接、断开自动释放）

---

## [0.0.9 ~ 0.0.20] — 基础功能建设 (2026-03)

- 多角色管理系统（角色列表、切换、状态跟踪）
- Cognitive Context API（四阶段推理：Perception→Motivation→Planning→Decision）
- Web Panel 智能路由（根据连通性和角色状态自动跳转）
- 服务器热切换 API
- 设备认证系统（auth_token + WebSocket token 校验）
- Intent 全链路追踪（唯一 intent_id + priority）
