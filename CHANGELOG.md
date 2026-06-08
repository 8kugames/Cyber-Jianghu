# Cyber-Jianghu 更新日志

---

## [Unreleased]

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
- **关系图谱迁移 (RelationshipStore PRAGMA Migration)**
  - SQLite PRAGMA `user_version` 自动迁移落地，`relationships` 表从 5 列扩展为 7 列（新增 `self_description`, `description_tick`），代码量精简。

### 智能体认知 (Agent Cognitive)

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
