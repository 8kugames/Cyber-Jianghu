# Cyber-Jianghu 功能架构

**日期**: 2026-06-09

## 核心定位

- **Server (天道)**: 权威物理引擎，实时处理 Intent，Tick 驱动状态广播
- **Agent (众生)**: 自主 AI 决策，三魂架构，三级记忆系统

---

## Server (天道)

### P0 核心架构

- **实时 Intent 处理管道 (Realtime Pipeline)**
  - 基于单消费者 MPSC 队列的 `IntentWorker`，消除所有并发冲突。
  - **Saga 分布式事务模式**: DashMap (Write-Through) → StateProcessor 执行 → 持久化 PostgreSQL。持久化失败触发内存状态逆向回滚，保障绝对一致性。
  - **同地广播 (Co-located Broadcast)**: 动作执行后仅向处于同一 `node_id` 的 Agent 广播，避免全局网络风暴。
- **Tick 调度引擎**
  - 作为纯时钟推进时间，计算 HP、体力、饥饿、口渴衰减，并执行周期 WorldState 广播。
  - 寿终正寝判定。
- **数据驱动动作系统 (Data-Driven Actions)**
  - 彻底消灭代码层硬编码判断。`actions.yaml` 统一定义：`transmission`（决定是 Broadcast、Session 还是 Silent）、`display_name`、`validator_kind`、`highlight_kind` 等属性。

### P1 重要特性

- **设备与角色分离 (Device-Character Lifecycle)**
  - 死亡 (`dead`) 标记不可逆。
  - 自动重生 (Auto-Rebirth) 执行 `INSERT` 生成全新 `agent_id` 的角色，并继承设备所属权。归隐 (`retired`) 状态仅供玩家主动触发。
- **动态公式引擎**
  - 使用 `evalexpr` 动态计算属性派生和战斗伤害。

---

## Agent (众生)

### P0 核心架构

- **三魂架构 (Three-Soul Architecture)**
  - **人魂 (ActorSoul)**: 认知核心，单 LLM 调用内完成 Perception→Motivation→Planning→Decision 四阶段推演，生成结构化 Intent 和 CognitiveChain 溯源链。内置低 San 值混沌 (Chaos) 行为生成。
  - **地魂 (EarthSoul)**: 行动落地工具池，内嵌于 LLM 推理循环。提供 `search_memory`、`query_world`、`skill_view`、`list_skills` 等工具。具有严格的配额 (Budget) 和防死循环 (LoopGuard) 限制。
  - **天魂 (ReflectorSoul)**: 三层验证大闸 (Layer 1 基础类型 → Layer 2 RuleEngine 物理规则 → Layer 3 LLM OOC 审查)。验证失败会转换为叙事反馈重新注入人魂。
- **三级记忆与情绪门控 (Memory & CoreAffect)**
  - 工作记忆、基于 SQLite 的情景记忆 (受艾宾浩斯遗忘曲线影响)、基于 HNSW 向量索引的语义记忆 (bge-small-zh)。
  - **CoreAffect**: 基于 Barrett 情绪建构论，以效价×唤醒度动态调整事件重要度，实现情绪对记忆编码与检索的干预。

### P1 重要特性

- **多意图管道 (Subsequent Intents)**
  - Agent 可单次提交原子意图队列 (`subsequent_intents`)，Server 顺序独立执行每个原子意图，一旦某环节失败，仅回滚当前失败的意图并打断后续流程，已成功的意图保持成功。
- **Token 极致优化**
  - **DeltaEngine 与 AttentionController**: 对比本地 WorldStateStore 增量，提取 `FocusSummary`，替代全量世界状态注入 Prompt。
  - **DeepSeek 缓存调优**: 基于 `system_hash` 跟踪、剥离推理过程 (D8, 默认关闭，通过 `CYBER_JIANGHU_PROMPT_STRIP_REASONING_CONTENT` 启用)、JSON Schema 规范化 (D9)，最大化 Prefix Cache 命中。
- **元认知行为框架 (Procedural Skills)**
  - 基于经验阈值的被动技能系统。掌握 "进退之道" 等 AI 思考模型后，经由地魂 `skill_view` 工具供 LLM 按需查阅。
- **社交网络自动迁移**
  - `RelationshipStore` 使用 SQLite PRAGMA `user_version` 管理迁移，自动无缝升级 Schema（例如追加 `self_description` 列）。

### P2 体验增强

- **管理面板 SPA (Agent Web Panel)**
  - 完全由 JS 驱动的客户端渲染面板 (#/dashboard, #/characters, #/settings)，支持 SSE 实时查看推演日志。
- **纪传体传记 (Chronicle & Biography)**
  - 服务器每 7 游戏日生成全服群像传记。角色死亡时触发基于每日摘要的纪传体生成。
