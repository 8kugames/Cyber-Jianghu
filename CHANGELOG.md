# Cyber-Jianghu 更新日志

---

## [Unreleased]

> **BREAKING**: `ExecutionResult` 新增 `governance_code` 字段（向后兼容：`Option<GovernanceCode>` 序列化时 `skip_serializing_if`）。protocol crate 版本从 0.1.68 升级到 0.1.69。

### 治理系统 (Governance)

- **Soul 审议引擎 (SoulReviewEngine)**
  - 新增 `SoulReviewEngine`：基于 `souls.yaml` 配置的投票式提案审核引擎，支持硬性规则 (hard_reject_if / hard_approve_if) 自动裁定。
  - 引入 `ProposalStore`：提案组生命周期管理（PendingReview → UnderReview → Approved/Rejected/EscalatedAdmin）。
  - `TopicClassifier`：基于 `action_evolution.yaml` 规则的治理主题分类器，自动路由提案到对应 Soul。
  - 审议结果广播：Approved 提案组通过 `ConfigUpdate` 广播到所有在线 Agent。
- **动作演化 API (Action Evolution API)**
  - 新增 `POST /api/v1/action-evolution/propose`：Agent 提交动作演化提案。
- **数据库表**
  - `action_evolution_proposals`：动作演化提案表。
  - `action_evolution_proposal_groups`：提案组表。
  - `soul_review_votes`：Soul 审议投票记录表。

### 核心架构 (Core Architecture)

- **实时 Intent 处理管道 (Realtime Pipeline)**
  - 彻底退役 Tick 批处理，Intent 实现实时执行。
  - 引入 `IntentWorker`：单消费者 MPSC Channel 设计，消除所有状态锁竞争，确保完全无竞态。
  - `StateProcessor` 实现单 Intent Saga 事务回滚，保证 `DashMap` (内存 Write-Through 缓存) 与 PostgreSQL 绝对一致。
- **设备身份与角色分离 (Device-Character Separation)**
  - 角色生命周期重构：死亡角色保持 `dead` 状态，转世重生 (Auto-Rebirth) 从 UPDATE-in-place 改为 INSERT 全新 `agent_id`，保留设备与新角色的映射。
  - 归隐语义 (`retired`) 严格专属玩家主动操作，消除幽灵重生错误。
- **数据驱动的动作体系 (Data-Driven ActionType)**
  - `ActionType` 彻底数据驱动化。`transmission` (Broadcast/Session/Silent)、`display_name`、`validator_kind` 剥离硬编码，由 `actions.yaml` 定义。
- **动作系统 v2 原子化重构 — BREAKING**
  - 从 20 个语义化动作精简为 10 个原子原语：予/取/用/移动/说话/观察/攻击/休整/制造/教导（原给予/偷窃/进食/饮水/拾取/丢弃/采集/私语/大喊/打坐/修炼 已移除）。
  - 予/取/用 替代所有物品交互（出背包/入背包/消耗），纯方向性无社会语义。
  - 说话 通过 channel 参数（public/private/broadcast）统一三种形态。
  - 所有旧动作名从代码、配置、提示词、前端、测试中彻底移除，不做向后兼容。
  - 遗留数据中旧 action_type 字符串（如 "eat", "give"）将触发"未知的动作类型"错误。
- **关系图谱迁移 (RelationshipStore PRAGMA Migration)**
  - SQLite PRAGMA `user_version` 自动迁移落地，`relationships` 表从 5 列扩展为 7 列（新增 `self_description`, `description_tick`），代码量精简。

### 智能体认知 (Agent Cognitive)

- **Embedding 服务独立化 (Embedding Service Extraction)**
  - 提取独立 `crates/embedding/` crate，支持双模式部署：Docker 环境使用远程 HTTP 服务（端口 23350），进程部署使用本地内嵌推理。
  - `EmbedderService` 自动检测 `CYBER_JIANGHU_EMBEDDER_REMOTE_URL` 环境变量选择 Local/Remote/Unavailable 三种 provider。
  - 模型下载使用 reqwest + SHA256 校验（无 hf-hub 依赖），Dockerfile 构建时预下载模型。
  - Remote 模式 fast fail，不静默降级；OnceLock + async `ensure_initialized()` 消除 TOCTOU 竞态与 `block_in_place` 反模式。
- **Token 与注意力优化 (Attention & Token Optimization)**
  - 引入 `WorldStateStore` 与 `DeltaEngine` 记录状态增量，基于 `AttentionController` 输出 `FocusSummary`，替代全量 WorldState 注入 Prompt。
  - DeepSeek 前缀缓存调优：基于 system_hash 监控指标、D8 Reasoning 剥离、D9 JSON Schema 递归排序标准化，提高缓存命中率。
  - 规则按需检索 (Rule-On-Demand)：系统提示词仅保留索引，LLM 通过 `query_rules` 检索全文。
- **三魂架构演进 (Three-Soul Evolution)**
  - ReflectorSoul (天魂) 三层审查统一，作为唯一的 Intent 合规性验证入口。
  - 剥离所有别名容错，要求 LLM 输出精准 ID，错误由 ReflectorSoul 以叙事形式反馈拦截。
- **情绪-记忆联动 (CoreAffect)**
  - 基于 Barrett 情绪建构论，实现效价×唤醒度的核心情感计算，情绪门控增强记忆重要度，实现心境一致性检索偏置。

### 控制台与运维 (Admin & Control Panel)

- **Agent Web Panel SPA 重构**
  - 6 个分散的 HTML 重构为 3 页 SPA (#/dashboard, #/characters, #/settings)，消减冗余 CSS，实现细粒度组件渲染。
  - 暴露完整的 LLM 参数控制与热重载接口，支持自动重生配置动态开关。
- **指标与监控**
  - 完善 `/api/v1/metrics?system_hash=<hex>` 接口，支持精准的 LLM 性能度量与 Token 消耗分析。
