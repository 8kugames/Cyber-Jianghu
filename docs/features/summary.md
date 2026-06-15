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
  - 作为纯时钟推进时间，计算 HP、体力、各类属性衰减，并执行周期 WorldState 广播。
  - 寿终正寝判定。
- **10 原子数据驱动动作系统 (Data-Driven Actions)**
  - 从 20 个语义化动作精简为 10 个原子原语：予/取/用/移动/说话/观察/攻击/休整/制造/教导。予/取/用以纯方向性替代给予/偷窃/进食/饮水/拾取/丢弃/采集等社会语义分类；说话通过 channel 参数（public/private/broadcast）统一三种形态；休整替代休息/打坐/修炼。
  - `actions.yaml` 统一定义：`transmission`（Broadcast/Session/Silent）、`display_name`、`validator_kind`、`highlight_kind` 等属性。所有旧动作名彻底移除，不做向后兼容。

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

---

## 动作演化治理 (Action Evolution Governance)

### P0 核心机制

- **拒绝 → 提案转化**: Agent 提交未知动作时，天魂拒绝后经 `ServerGovernanceMapper` 映射为 `GovernanceCode`（UnknownAction），Agent 异步提交提案（含 action_type + action_data + rationale）到 Server 演化池。
- **三皇共治（火云洞天宏观治理智能）**:
  - **伏羲氏（演化之主）**: 世界多样性 + 演化方向，倾向引入新变量。初审 + 终审双角色。
  - **神农氏（生存之主）**: 种群生存率 + 资源平衡，倾向稳健生态策略。同辈并行审议。
  - **轩辕氏（秩序之主）**: 世界观稳定秩序——天道法则自洽 + 世界循环稳定。不审查个体 agent 命运，不审查合理 violence（PK/报仇/抢夺是社会涌现）。
- **三阶段管道（stage 持久化）**:
  - 阶段 1：伏羲初审 → 拒绝直接关单 / 批准（含 inferred_action_config）推进阶段 2
  - 阶段 2：神农 ‖ 轩辕并行（tokio::join!）→ 全部拒绝关单 / ≥approve_threshold 推进阶段 3
  - 阶段 3：伏羲终审（注入同辈反馈，可能调整 inferred_action_config）→ 写入 actions.yaml
- **管道约束**:
  - 禁止弃权（LLM 超时/失败由系统强制注入 Reject）
  - 同 similarity_key 多 proposal 共享 fate
  - 写入失败保护：避免 group 标 Approved 但 actions.yaml 未写入的状态分裂
  - close_stale_groups 仅关闭 awaiting_fuxi_initial 超时 group
- **数据驱动**: `souls.yaml` 配置三皇 + `approve_threshold`（默认 2/3）+ 阶段超时；新增 Soul 只需加 YAML 条目。
- **能力注册表 (CapabilityManifest)**: 从 `ActionRegistry` 自动投影，作为审议引擎的"事实层"真源。
- **优雅降级**: `init_governance` 失败时 `governance: None`，非治理路径不受影响。

### 延后项（明确登记，Phase 2 实施）

- 神农氏核心职责：种群生存率/资源平衡/生态稳健的指标监控
- 轩辕氏核心职责：世界观稳定秩序监控（法则自洽/循环稳定/规则套利防御）
- `SourceProvider` trait 的实际 metric 接入

### 破坏性更新

- 删除 `ProposedActionIR` + `IRSource` 类型（protocol crate）
- DB migration 013 删除 `action_evolution_proposals` 表 IR 字段，新增 `action_data`
- DB migration 014 新增 `action_evolution_proposal_groups.stage` 列
- `ProposalRequest.ir` 字段删除，新增 `action_data: serde_json::Value`
- `ReviewVerdict` 新增 `reject_reason` + `inferred_action_config` 字段
- `GroupVote.vote` 类型从 `ProposalStatus` 改为 `VoteChoice`
- `SoulsReviewConfig` 删除 `reject_threshold` + `source_bindings` 字段
- 旧版 `ExecutionResult.governance_code` 字段保持向后兼容（`Option` + `skip_serializing_if`）
