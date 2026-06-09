# 三皇议会动作演化治理方案

>继承并扩展自 `action-evolution-governance.design.md`（伏羲审议链）。
> 本文档将"单皇审议"升级为"三皇议会：单皇首审 + 双皇复审 +2/3多数决"。

##1.文档目的

本文定义一套完整、可落地、可审计、可回滚的"动作演化治理"方案，用于实现以下目标：

- 天魂保持现有拒绝行为，不放松执行安全边界。
- 当 Agent 因未知动作或现有动作表达力不足而被拒绝时，异步触发一次自评估。
- 只允许提审原子行为，复合行为直接在提审前被丢弃（不予通过）。
- 若自评估判断该需求真实存在且为原子行为，则向 Server发起"动作演化提案"。
- 由三皇议会审议提案：
 - **单皇首审**：基于议题优先级，由伏羲/神农/轩辕中的一人担任首审。
 - **双皇复审**：另外两皇基于各自真源进行复审。
 - **2/3多数决**：2票赞成即通过；2票反对即否决；1/1/1 转人工。
 - **反对意见存档**：所有反对票的 rationale 进入 dissent_log，不强制复议。
- 通过的白名单部分生成规则化 action 配置并热更新广播；不可自动演化但确有必要的部分进入 server-admin提案页。

本文从"谁有权改变世界规则、谁负责执行世界规则、谁只负责提出需求、谁主持治理"四个第一性问题出发，重新定义动作演化的治理链路。

##2. 用户目标

用户预期目标可以被严格表达为：

1.保持现有三魂执行链路不变，未知动作仍然拒绝。
2.拒绝不终止信号价值，而是转化为演化提案。
3. **只允许提审原子行为 action，不允许提审复合行为 action，判定为复合 action 的不予通过。**
4. 自动演化必须以数据驱动、配置驱动为核心，不允许靠硬编码补洞。
5. 对无法自动演化但确有必要的新能力，必须进入可审阅、可排期、可跟踪的人类开发流程。
6. **治理必须分权制衡：单皇首审、双皇复审、2/3多数决、反对意见存档。**
7. **初审永远只有1 人**：无论议题跨几个职能，首审皇都是唯一的；其他两皇作为复审参与。
8. **三皇的真源可量化、可审计**：伏羲看 Capability Manifest，神农看资源/生存指标，轩辕看秩序/伦理指标。

这八点必须同时成立。缺任何一点，方案就偏离原始目标。

##3. 第一性原理

###3.1 世界规则修改权必须属于 Server

- Agent 的职责：感知、决策、提案。
- Server 的职责：验证、执行、治理、广播。
- 配置的职责：表达当前允许存在的世界规则。
- 三皇议会的职责：审议 Server 上的演化提案，决定是否纳入配置。

如果允许 Agent 直接落配置，系统就不再是 server authoritative。
如果允许单皇单独通过提案，系统就不再是分权制衡。

###3.2 执行链与治理链必须分离

- **执行链严格拒绝**（ActorSoul → ReflectorSoul → Server StateProcessor）。
- **治理链异步审议**（Self-Evaluator → Proposal Aggregator → 三皇议会）。
- 两链不能合一。Agent 不能绕过治理直接获得新能力。

###3.3 数据驱动的真实边界必须说清

当前项目的动作系统是"**半开放的配置驱动系统**"，不是"任意新语义可自动涌现系统"。

真正的一等事实不应是"文档里手写了哪些允许字符串"，而应是：

- Server 当前加载出的动作 schema能力（伏羲真源）
- Server 当前执行器实际暴露的能力清单（伏羲真源）
- 由两者合成的**能力注册表（Capability Manifest）**

三皇各自的真源：

|皇 | 真源层 |关注的指标 |
|---|---|---|
|伏羲（演化） | Capability Manifest |动作 schema覆盖率、表达力 gap、新能力需求频次 |
| 神农（生存） | Resource & Survival Indicators |资源产出率、消耗率、食物链压力、Agent存活率、饥饿致死率 |
|轩辕（秩序） | Order & Ethics Indicators |伦理违规率、帮派冲突指数、社会稳定度、规则一致性 |

本文后续凡是出现"允许维度""禁止维度""能力分组"等概念，均以这三层真源为唯一机器真源。

###3.4原子性先于演化

复合行为（如"交易"、"结盟"）本质上是多个主体、多个状态机、多个原子动作的协议编排。
把复合行为压缩成单个 action 会导致：

-隐藏子事务，破坏单一动作结算原则。
-引入新的状态机，超出配置驱动的表达边界。

因此，**必须在提案阶段通过机器可判定的 IR (Intermediate Representation) 进行原子行为过滤，拒绝所有复合行为。**

###3.5 分权制衡先于效率（新增）

三皇议会的引入不是为了"审核更快"，而是为了"防止单一价值观主导世界演化"。

-伏羲单独审议 →演化压力过大，神农/轩辕的反对意见无制度化通道。
- 神农单独审议 →抑制演化，世界僵化。
-轩辕单独审议 →过度强调秩序，新涌现被压制。

只有当三皇在制度上对等，且**每个提案都至少有两个皇参与复审**，分权才有意义。

但分权不等于冗余：**初审必须只有1 人**，否则就退化为"委员会全员审批"的低效模式。单皇首审保证审议深度（首审负责深度分析），双皇复审保证审议广度（复审负责各自真源的对齐）。

###3.6 可解释性先于智能（新增）

三皇不能是"黑盒模型投票"。每个皇的审议结果必须可追溯：

- 真源指标是什么（量化输入）
-判定依据是什么（规则 + LLM软裁决解释）
-反对意见的完整文本（如果投反对）
- 同类提案的历史分歧记录

否则治理层无法被审计，无法被回滚，无法被运营改进。

##4.术语定义

###4.1 被拒绝动作

指 ActorSoul产出的动作，在天魂校验阶段因以下原因被拒绝：

- 当前 `actions` 中不存在该动作
- 当前动作意图可被识别，但缺少世界级支持

###4.2动作演化提案 IR (ProposedActionIR)

指由 Agent 在拒绝后异步产生的一份**类型化中间表示**，而不是自然语言散文。
它严格描述了行为的执行特征（参与者数量、阶段数、时间跨度、副作用数量）。

扩展字段（相比原文档）：

```json
{
 "ir": {
 "actor_arity":1,
 "target_arity": "0_to_many",
 "tick_span":0,
 "phase_count":1,
 "protocol_kind": "none",
 "state_transition_count":1,
 "effect_refs": ["..."],
 "requirement_refs": ["..."]
 },
 "governance_topics": ["evolution"],
 "topic_confidence": {"evolution":0.92},
 "rationale": "string"
}
```

- `governance_topics`：议题涉及的治理职能集合。每个元素取值为 `evolution` / `resource` / `order`。
- `topic_confidence`：每个议题的分类置信度（0–1）。分类器路由时使用。

###4.3原子行为

通过 ProposedActionIR机器可判定的单一不可分割行为。必须满足：

- `actor_arity ==1`
- `phase_count ==1`
- `tick_span ==0`
- `protocol_kind == none`

反例：

- `交易`：包含双方给予，`protocol_kind != none`，不是原子行为。
- `拜师收徒`：包含多阶段确认，`phase_count >1`，不是原子行为。

**裁决：复合行为不予通过，不允许提审。**

###4.4 三皇 (Three Emperors)

|皇 |治理职能 |关注的真源 |
|---|---|---|
|伏羲 (Fuxi) |演化 | Capability Manifest |
| 神农 (Shennong) |生存 | Resource & Survival Indicators |
|轩辕 (Xuanyuan) |秩序 | Order & Ethics Indicators |

###4.5审议路由 (Review Routing)

按以下规则确定**唯一一个首审皇**：

1.议题优先级：`evolution` > `resource` > `order`
2. 取 `governance_topics` 中优先级最高且 `topic_confidence >=0.6` 的议题
3. 该议题的 primary_soul 即为首审皇
4. 其余两皇作为复审参与

反例：

- topics=[evolution, resource]，confidence 都 ≥0.6 → 首审 =伏羲，复审 = 神农 +轩辕
- topics=[resource, order]，confidence 都 ≥0.6 → 首审 = 神农，复审 =伏羲 +轩辕
- topics=[order] → 首审 =轩辕，复审 =伏羲 + 神农
- topics=[] 或所有 confidence <0.6 →分类失败，转人工

**核心原则：初审永远只有1 人。**

###4.6投票语义 (Voting Semantics)

每个皇在审议中产出 `vote ∈ {approve, reject, abstain}` + `rationale` + `evidence_refs`。

最终决议判定：

|赞成 |反对 |弃权 |决议 |
|---|---|---|---|
| ≥2 | — | — | **通过**，存档反对意见（如有）|
| — | ≥2 | — | **否决**，存档反对意见 |
|1 |1 |1 | **转人工**（admin提案页）|
|2 |1 |0 | **通过**，存档反对意见 |
|1 |2 |0 | **否决**，存档反对意见 |
|1 |0 |2 | **通过** |
|0 |1 |2 | **否决** |
|0 |0 |3 | **转人工** |

**核心原则：纯2/3多数决，不强制复议。**

###4.7反对意见存档 (Dissent Log)

所有 `vote == reject` 的票必须完整存档：

-投反对的皇
-反对的 rationale
- 引用的真源指标（evidence_refs）
- 时间戳

存档位置：`proposal_groups.dissent_log`（jsonb字段）。

后续同类提案出现时，dissent_log 作为优先级提示，但不强制提高审议门槛。

###4.8 同源提案合并

- 同源提案：基于 IR相似度（effect_refs + semantic_scope cosine）合并。
- 同源提案被合并时，dissent_log累计。
-累计反对意见 ≥3 → 该类提案自动转 admin（标记 `contested`）。

###4.9复合提案拆解闭环

前置闸门已**不允许复合行为提审**。但如果首审皇（通常是伏羲）发现 Agent 的 IR撒谎（如名为"交易"，IR伪装成原子），首审皇必须将其状态标记为 `rejected_composite`，并记录为 Agent 的不良行为样本（影响 Agent后续提案的可信度权重）。

##5.总体架构

###5.1总体链路

```text
ActorSoul产出动作 -> ReflectorSoul校验 ->拒绝
 ->
异步触发 Rejection Self-Evaluator (生成 ProposedActionIR with governance_topics)
 ->
执行原子行为判定 (基于 IR)
 ->
若为复合行为：直接 Drop (不予通过)
若为原子行为且真实需要：提交 ActionEvolutionProposal 到 Server
 ->
Server Proposal Aggregator聚合相似提案 +分类器复核 governance_topics +路由器选首审皇
 ->
单皇首审 (Primary Soul)产出 vote + rationale + evidence_refs
 ->
双皇复审 (Co-Reviewers) 并行产出 vote + rationale + evidence_refs
 ->
投票语义判定器（2/3多数决）
 ->
通过：白名单内 -> 生成 ActionConfig -> staged校验 ->原子热更新 ->广播 ConfigUpdate
通过：白名单外但必要 ->转入 server-admin提案页
否决：Proposal 进入 closed_rejected，dissent_log存档
1/1/1 分裂：转 server-admin提案页（保留全部证据）
```

###5.2 三皇分工速查

|议题类型 | 首审皇 |复审皇 |适用场景 |
|---|---|---|---|
| `evolution` 单议题 |伏羲 | 神农、轩辕 | 新动作、新技能、新物品的引入 |
| `resource` 单议题 | 神农 |伏羲、轩辕 |资源相关能力的扩展 |
| `order` 单议题 |轩辕 |伏羲、神农 |涉及伦理边界、规则一致性的能力 |
| `evolution + resource` 双议题 |伏羲 | 神农、轩辕 | 如"新采矿技能"（伏羲主导，复审对齐资源/秩序）|
| `evolution + order` 双议题 |伏羲 | 神农、轩辕 | 如"新战斗技能"（伏羲主导，复审对齐资源/秩序）|
| `resource + order` 双议题 | 神农 |伏羲、轩辕 | 如"资源配额规则"（神农主导，复审对齐演化/秩序）|
| 三议题 |优先级最高者 | 另外两皇 |极复杂议题，仍是单皇首审 |
|分类失败（topics 空 / 全低置信度）| 无 | 无 | 转 server-admin |

**核心约束：无论议题跨几个职能，首审永远只有1 人。**

##6. Agent侧设计：Self-Evaluator 与原子闸门

###6.1拒绝信号契约：RawRejectionFact 与 GovernanceCode

为了防止 Agent越权或漏采信号，协议拆分为两层：

1. **RawRejectionFact**：Reflector 或 Server 执行器只产出原始事实（哪个动作、缺什么参数、哪个校验失败）。
2. **GovernanceCode**：Server治理入口统一将 RawRejectionFact映射为治理分类码（如 `unknown_action`, `expression_gap`, `non_governance_reject`）。

Agent 的 Self-Evaluator 仅在收到映射后的 `unknown_action` 或 `expression_gap`才会触发。

###6.2 自评估与 IR 生成（含 governance_topics）

Self-Evaluator接收拒绝事实和上下文，输出严格的 `ProposedActionIR` 和决策：

```json
{
 "decision": "drop | use_existing | propose",
 "ir": {
 "actor_arity":1,
 "target_arity": "0_to_many",
 "tick_span":0,
 "phase_count":1,
 "protocol_kind": "none",
 "state_transition_count":1,
 "effect_refs": ["mining.dig_action"],
 "requirement_refs": ["tool.pickaxe"]
 },
 "governance_topics": ["evolution", "resource"],
 "topic_confidence": {
 "evolution":0.85,
 "resource":0.72
 },
 "rationale": "现有动作中缺少对矿山采掘的原子操作；新动作对资源产出有显著影响。"
}
```

**Agent端的分类是 best-effort**。Server端的分类器（详见7.3）会再次校验并修正。

###6.3原子行为闸门 (硬拦截)

Self-Evaluator产出 IR 后，必须通过本地/服务端的原子行为函数校验：

```rust
fn is_atomic(ir: &ProposedActionIR) -> bool {
 ir.actor_arity ==1 &&
 ir.tick_span ==0 &&
 ir.phase_count ==1 &&
 ir.protocol_kind == ProtocolKind::None
}
```

**如果 `!is_atomic(ir)`，则 `decision`强制被覆写为 `drop`，不予通过。**

###6.4议题分类辅助（软建议）

Agent 不强制要求给出 `governance_topics`，但如果给出：

-必须是 `evolution` / `resource` / `order` 的子集。
- 不能为空。
- `topic_confidence` 总和应近似1（不强校验）。

Server端分类器将以 Agent 输出为初始值，结合 effect_refs 和语义做最终判定。

##7. Server侧设计

###7.1提案表与主载体

新增表：

- `action_evolution_proposals`：只存 raw evidence（继承原方案）。
- `action_evolution_proposal_groups`：治理状态机的主载体（继承原方案）。

**新增字段**（在 `proposal_groups` 表）：

|字段 | 类型 | 说明 |
|---|---|---|
| `primary_soul` | `enum(fuxi, shennong, xuanyuan)` |路由结果：首审皇 |
| `co_reviewers` | `array<soul>` |路由结果：复审皇列表 |
| `governance_topics` | `array<topic>` |议题涉及的治理职能 |
| `votes` | `jsonb` | 每皇的 vote + rationale + evidence_refs |
| `final_decision` | `enum(approved, rejected, escalated)` | 最终决定 |
| `dissent_log` | `jsonb` |反对意见累计 |

**新增表** `soul_review_votes`：

|字段 | 类型 | 说明 |
|---|---|---|
| `id` | `uuid` | 主键 |
| `proposal_group_id` | `uuid` |关联提案组 |
| `soul` | `enum(fuxi, shennong, xuanyuan)` |投票皇 |
| `role` | `enum(primary, co_reviewer)` |角色：首审或复审 |
| `vote` | `enum(approve, reject, abstain)` |投票 |
| `rationale` | `text` |判定依据 |
| `evidence_refs` | `jsonb` | 引用的真源指标 |
| `created_at` | `timestamp` |投票时间 |

###7.2能力注册表：Capability Manifest (事实层 -伏羲真源)

Server启动时由各执行器自动投影生成。
仅包含：`capability_id`, `kind`, `semantic_scope`。
**不包含策略**，策略交由 `game_rules.yaml` 配置。

###7.3治理分类与路由层 (NEW)

####7.3.1分类器

Server端 `TopicClassifier` 函数签名：

```rust
fn classify_topics(
 ir: &ProposedActionIR,
 agent_topics: &[Topic],
 agent_confidence: &HashMap<Topic, f64>,
 manifest: &CapabilityManifest,
) -> ClassificationResult {
 //1.优先信任 Agent 的 topics (若 non-empty 且 confidence >0.6)
 //2. 否则基于 effect_refs 在 manifest 中的 semantic_scope 做规则映射
 //3.兜底：单议题 evolution
 //4. 返回 (topics, confidence, fallback_used)
}
```

分类规则表（YAML 配置）：

```yaml
topic_classifier:
 rules:
 - match:
 effect_refs_prefix: ["mining.", "harvest.", "gather."]
 topics: ["resource"]
 - match:
 effect_refs_prefix: ["combat.", "duel.", "martial."]
 topics: ["order"]
 - match:
 effect_refs_prefix: ["skill.", "craft.", "discover."]
 topics: ["evolution"]
 - match:
 effect_refs_any: ["economy.*", "trade.*"]
 topics: ["evolution", "order"]
```

####7.3.2路由器：单皇首审

```rust
fn route_review(
 topics: &[Topic],
 confidence: &HashMap<Topic, f64>,
) -> RoutePlan {
 //优先级：evolution > resource > order
 let priority = [Topic::Evolution, Topic::Resource, Topic::Order];

 //1. 取 topics 中优先级最高且 confidence >=0.6 的议题
 let primary_topic = topics
 .iter()
 .filter(|t| confidence.get(t).copied().unwrap_or(0.0) >=0.6)
 .min_by_key(|t| priority.iter().position(|p| p == *t).unwrap_or(99));

 match primary_topic {
 Some(topic) => {
 let primary = topic.primary_soul();
 let co = other_two_souls(primary);
 RoutePlan {
 primary,
 co_reviewers: co,
 mode: SinglePrimary,
 }
 }
 None => RoutePlan {
 //分类失败：转人工
 primary: None,
 co_reviewers: vec![],
 mode: EscalateToAdmin,
 },
 }
}
```

**核心约束：无论 topics集合多大，`primary`永远只有1 个皇。**

###7.4 三皇 Worker架构 (NEW)

####7.4.1物理部署

- **三个独立 Worker**：FuxiWorker / ShennongWorker / XuanyuanWorker，每个是独立 tokio task。
- **独立 in-flight队列**：每个皇有自己的 MPSC队列，互不阻塞。
- **共享 Proposal Aggregator**：所有 Worker 都从同一个 group 表拉取，但只拉取路由到自己名下的 proposal（首审或复审）。

```rust
//伪代码
let proposal_aggregator = Arc::new(ProposalAggregator::new(db_pool));
let (fuxi_tx, fuxi_rx) = mpsc::channel(64);
let (shennong_tx, shennong_rx) = mpsc::channel(64);
let (xuanyuan_tx, xuanyuan_rx) = mpsc::channel(64);

spawn_fuxi_worker(fuxi_rx, proposal_aggregator.clone());
spawn_shennong_worker(shennong_rx, proposal_aggregator.clone());
spawn_xuanyuan_worker(xuanyuan_rx, proposal_aggregator.clone());
```

####7.4.2皇的真源数据获取

每个 Worker启动时加载自己的真源层：

```rust
impl FuxiWorker {
 async fn load_source(&self) {
 self.manifest = CapabilityManifest::load().await?;
 }
}

impl ShennongWorker {
 async fn load_source(&self) {
 self.resource_indicators = ResourceIndicators::subscribe().await?;
 self.survival_indicators = SurvivalIndicators::subscribe().await?;
 }
}

impl XuanyuanWorker {
 async fn load_source(&self) {
 self.order_indicators = OrderIndicators::subscribe().await?;
 self.ethics_indicators = EthicsIndicators::subscribe().await?;
 }
}
```

####7.4.3皇的审议逻辑

每个皇的 Worker持有自己的 SystemPrompt + 真源 +判定规则。统一接口：

```rust
trait SoulWorker {
 async fn review(
 &self,
 proposal: &ProposalGroup,
 ir: &ProposedActionIR,
 role: ReviewRole,
 ) -> ReviewVerdict;
}

enum ReviewRole {
 Primary, // 首审：深度分析，输出 vote + rationale + evidence_refs
 CoReviewer, //复审：基于各自真源对齐，输出 vote + rationale + evidence_refs
}

struct ReviewVerdict {
 vote: Vote,
 rationale: String,
 evidence_refs: Vec<EvidenceRef>,
}
```

**神农的判定示例**（硬规则 + LLM软裁决）：

```yaml
shennong_policy:
 hard_reject_if:
 - metric: "agent_survival_rate_24h"
 operator: "<"
 threshold:0.3
 reason: "生存压力已饱和，禁止引入新的资源消耗能力"
 soft_concern_if:
 - metric: "resource_yield_rate"
 operator: "<"
 threshold:0.5
 reason: "资源产出低迷，谨慎评估新能力的资源消耗"
 hard_approve_if:
 - metric: "agent_survival_rate_24h"
 operator: ">"
 threshold:0.7
 - all_effects_in: ["survival.*", "efficiency.*"]
```

**轩辕的判定示例**：

```yaml
xuanyuan_policy:
 hard_reject_if:
 - effect_ref_matches: ["instant_kill", "force_trade", "mass_exterminate"]
 reason: "伦理红线"
 - metric: "faction_conflict_index"
 operator: ">"
 threshold:0.8
 reason: "社会秩序已紧张，禁止引入加剧冲突的能力"
 soft_concern_if:
 - effect_ref_matches: ["combat.*"]
 requires: ["ethics_review_passed"]
```

**首审与复审的角色差异**：

|角色 |职责 |深度 | 输出 |
|---|---|---|---|
| Primary（首审）|深度分析提案合理性 | 高 | vote +详细 rationale + 主引用的 evidence_refs |
| CoReviewer（复审）| 基于各自真源对齐 | 中 | vote +简洁 rationale + 引用的真源指标 |

首审的 rationale长度通常 ≥复审（首审负责"扛责任"）。

###7.5跨皇审议协议 (NEW)

####7.5.1 标准流程（单皇首审 + 双皇复审）

```text
T0 Proposal Aggregator分类完成 → route = (Primary, [Co1, Co2])
T1 Primary Worker收到 proposal_group → 首审 → 输出 vote + rationale + evidence_refs
T2 Proposal Aggregator写入 soul_review_votes (role=primary)
T3同步触发 Co1 和 Co2 Worker（并行复审）
T4 Co1 Worker复审 → vote + rationale + evidence_refs
T5 Co2 Worker复审 → vote + rationale + evidence_refs
T6 Proposal Aggregator收到3票 →触发投票语义判定器（2/3多数决）
T7a2票赞成 → approved_r1 → 进入白名单判定
T7b2票反对 → 进入 closed_rejected，dissent_log存档
T7c1/1/1 分裂 → escalated_admin
```

####7.5.2跨职能议题流程（仍单皇首审）

跨职能议题**不改变**首审唯一性：topics=[evolution, resource] 时首审=伏羲，神农/轩辕作为复审参与。

```text
topics=[evolution, resource] → primary=伏羲 → co=[神农,轩辕]
伏羲首审（深度分析演化需求）
神农复审（对齐 resource 真源）
轩辕复审（对齐 order 真源）
投票判定（2/3多数决）
```

####7.5.3跨皇通信保证

-提案组状态机：每皇投票写入 `soul_review_votes`，状态机在收到所有应到票后才推进。
- 单皇失败超时：SLA30 分钟。超时未投票 → 自动 `abstain` +标记为 `worker_timeout`。
-状态机推进是事件驱动（投票写入即推进），不是轮询等待。

###7.6投票语义判定器 (NEW)

####7.6.1状态机

`proposal_groups.status`状态：

|状态 | 说明 |触发动作 |
|---|---|---|
| `pending_review` | 待审议 |路由完成 |
| `under_review` |审议中 | 三皇 Worker启动 |
| `approved` | 通过 | 进入白名单判定 |
| `rejected` | 否决 | closed_rejected，dissent_log存档 |
| `escalated_admin` | 转人工 | 进入 server-admin提案页 |
| `converged` | 热更新完成 |等待 ACK |
| `closed_approved` | 已完成 |终态 |
| `closed_rejected` | 已否决 |终态 |
| `error` |错误恢复 |终态，带 error_code |

**关键：去掉了"复议"相关状态（needs_rereview / under_review_r2 / approved_final / rejected_final）。**

####7.6.2判定器实现

```rust
fn resolve_votes(
 votes: &[SoulReviewVote],
) -> Decision {
 let approve_count = votes.iter().filter(|v| v.vote == Approve).count();
 let reject_count = votes.iter().filter(|v| v.vote == Reject).count();
 let abstain_count = votes.iter().filter(|v| v.vote == Abstain).count();

 //2/3 通过
 if approve_count >=2 {
 //反对票存档
 return Decision::Approved;
 }

 //2/3反对
 if reject_count >=2 {
 return Decision::Rejected;
 }

 //1/1/1 分裂：转人工
 Decision::EscalateAdmin
}
```

####7.6.3反对意见存档

无论最终是否通过，所有 reject票的 `rationale` 必须存档到 `proposal_groups.dissent_log`。

格式：

```json
{
 "dissent_log": [
 {
 "soul": "xuanyuan",
 "rationale": "新动作会破坏伦理边界...",
 "evidence_refs": ["ethics.red_line.mass_exterminate"],
 "created_at": "2026-06-09T10:00:00Z"
 }
 ]
}
```

后续同类提案出现时，dissent_log 作为参考，但不强制提高审议门槛。

####7.6.4 同源提案合并与优先级

- 同源提案：基于 IR相似度（effect_refs + semantic_scope cosine）合并。
- 同源提案被合并时，dissent_log累计。
-累计反对意见 ≥3 → 该类提案自动转 admin（标记 `contested`）。

###7.7复合提案拆解闭环

因为前置闸门已经**不允许复合行为提审**，进入到三皇议会的提案理论上全是原子的。

但如果首审皇（通常是伏羲）发现 Agent 的 IR撒谎（如名为"交易"，IR伪装成原子），首审皇必须将其状态标记为 `rejected_composite`，并记录为 Agent 的不良行为样本（影响 Agent后续提案的可信度权重）。

##8. 热更新与收敛协议

###8.1唯一真源切分

- **Phase0**：`actions.yaml`仍是真源。不允许自动写配置。
- **Phase1+**：统一切到 `DB 真源 (action_config_versions) + yaml快照导出`。禁用人工绕过 Pipeline 直接改文件。

###8.2协议收敛与 ACK

热更新后，Agent 必须通过新增的 `config_applied_ack`消息反馈。

- 只有本地 `actions_version` 且 prompt cache切到新版本后，Agent 才允许发 ACK。
- Server冻结一个 `rollout target set`。满足多数 ACK 后，Proposal状态机才能进入 `converged -> closed`。

###8.3 三皇视角下的状态机闭环

错误恢复被收敛：

- 所有中间失败落入 `error`，带上 `error_code`。
- 回滚采用**前滚式回滚**（生成 `V+2` 内容同 `V`），保证版本单调，不破坏 ACK语义。
- 三皇任一 Worker崩溃 → 自动 `abstain` +触发该皇的 worker 重启，不影响其他皇的投票。

##9.实施阶段与路线

###9.1路线图总览

```text
Phase0 (MVP 当前)
 └──继承原伏羲审议链的边界
 └── 新增：分类器 + primary_soul字段落库 + governance_topics字段
 └── 不真做三皇审议（继承原方案的 Out of Scope）

Phase1 (过渡形态 - 单皇审议先跑通)
 └──伏羲 Worker完整上线（继承原方案 Fuxi Review Worker）
 └── 神农/轩辕的真源层建设（不审议，只收集指标）
 └──指标监控仪表盘
 └── 白名单自动演化

Phase2 (三皇并行审议)
 └── 神农 Worker 上线（resource + survival 真源）
 └──轩辕 Worker 上线（order + ethics 真源）
 └──跨皇通信协议完整
 └──2/3多数决判定器上线
 └──反对意见存档机制

Phase3+ (火云洞天扩展)
 └──引入更多治理议题（世界观偏移、涌现事件传播、宏观调控）
 └──引入"重大议题"三皇共审通道
 └── 与 chronicle系统的对接
```

###9.2 Phase0详细范围（MVP 当前）

**建议：做分类 +落 `primary_soul`字段**（成本几乎为零、未来升级零迁移、不破坏原方案边界）。

**In Scope**：

-继承原伏羲审议链的所有 Phase0 内容：
 -产生 RawRejectionFact
 -映射 GovernanceCode
 - Self-Evaluator 生成 IR
 - 执行原子闸门拦截
 -落库
 -离线报表
- **新增**：
 - IR 增加 `governance_topics` 和 `topic_confidence`字段（Agent端软建议）
 - Server端 `TopicClassifier`落地（基于 effect_refs规则映射）
 - `proposal_groups` 表新增 `primary_soul` / `co_reviewers` / `governance_topics` / `final_decision`字段
 - `soul_review_votes` 表创建（即使 Phase0 不写入数据，结构预留）
 -分类器产生的 `primary_soul`写入 `proposal_groups`
 -离线报表增加"按 primary_soul 分组"的统计维度

**Out of Scope**：

- 三皇 Worker 实例（不启动 FuxiWorker/ShennongWorker/XuanyuanWorker）
-跨皇通信协议（不实现）
-投票语义判定器（不实现）
- 自动写配置 / 热更新 / Admin 工作流（继承原方案）

**Phase0 完成标志**：

- Agent提交 proposal 时，`governance_topics`字段非空且分类器能给出 `primary_soul`。
-落库的 proposal_groups 行包含 `primary_soul`字段值。
-离线报表能按 primary_soul 分组查看 proposal分布。

###9.3 Phase1详细范围（过渡形态）

**前置条件**：Phase0完成后，proposal 数据已带 primary_soul标签。

**In Scope**：

-启用伏羲单皇 Worker（继承原方案 Fuxi Review Worker 的所有逻辑）。
- 白名单自动演化（继承原方案）。
- 热更新协议 + ACK收敛（继承原方案）。
- 神农真源层建设：
 - `ResourceIndicators` 模块（资源产出率、消耗率、食物链压力）
 - `SurvivalIndicators` 模块（Agent存活率、饥饿致死率、寿命分布）
 -仪表盘数据采集（不参与审议）
-轩辕真源层建设：
 - `OrderIndicators` 模块（帮派冲突指数、社会稳定度）
 - `EthicsIndicators` 模块（伦理违规率、底线行为触发频次）
 -仪表盘数据采集（不参与审议）
- `soul_review_votes` 表开始写入（伏羲单皇投票，复审位暂时为空）。

**Out of Scope**：

- 神农 Worker /轩辕 Worker审议逻辑。
-跨皇通信协议。
-投票语义判定器。

**Phase1 完成标志**：

-伏羲 Worker周期审议 proposal，按白名单自动演化或转 admin。
- 神农/轩辕的真源指标能在仪表盘实时查看。
-任意一条 proposal至少有伏羲一票记录。

###9.4 Phase2详细范围（三皇并行审议）

**前置条件**：Phase1完成后，伏羲单皇审议已稳定运行，神农/轩辕的真源层已采集到足够数据。

**In Scope**：

-启用神农 Worker：
 -加载 `ResourceIndicators` + `SurvivalIndicators` 真源
 -实施 `shennong_policy`规则（详见7.4.3）
 - 支持 approve / reject / abstain 三种投票
 - 支持首审和复审两种角色
-启用轩辕 Worker：
 -加载 `OrderIndicators` + `EthicsIndicators` 真源
 -实施 `xuanyuan_policy`规则
 - 支持 approve / reject / abstain 三种投票
 - 支持首审和复审两种角色
-跨皇通信协议：
 - ProposalAggregator 实现（按路由分发到三个 Worker）
 - 单皇失败 SLA + abstain兜底
-投票语义判定器（详见7.6.2）。
-反对意见存档（dissent_log）。
- 同源提案合并与优先级（详见7.6.4）。

**Out of Scope**：

- 与 chronicle系统的对接。
- 世界观偏移、宏观调控等火云洞天扩展议题。

**Phase2 完成标志**：

- 一个 proposal完整走完"首审 →复审 →2/3多数决 →决议"流程。
-反对意见存档至少触发过一次并产生 dissent_log。
- 同源提案合并可工作。
- 三皇任一 Worker崩溃不影响其他皇投票。
-1/1/1 分裂转人工的路径可工作。

###9.5 Phase3+范围（火云洞天扩展）

**In Scope**（路线图级别，非详细范围）：

-引入更多治理议题类型：
 - 世界观偏移（worldview_shift）
 -涌现事件传播（emergence_propagation）
 -宏观调控（macro_economic_control）
-引入"重大议题"通道（无论 topics数量，强制三皇全员深度审议）。
- 与 chronicle系统的对接：三皇审议结果进入群像传记叙事层。
- 与小千世界（Cyber-Worlds-Microcosm）治理对接。

**说明**：本节为路线图预留，详细设计在 Phase3启动时单独成文。

##10.风险评估

###10.1治理成本风险

三皇审议的 LLM成本是单皇的约2–3 倍（首审1 次 +复审2 次）。Phase2启动后需要预算评估。

**对策**：

- 神农/轩辕的硬规则（hard_reject_if / hard_approve_if）能覆盖大部分场景，避免无谓的 LLM 调用。
- 真源指标优先，LLM 仅做软裁决和解释。
- 单皇失败 SLA + abstain兜底，避免阻塞。

###10.2分类器漂移风险

`TopicClassifier` 是基于 effect_refs 的规则映射，规则需要持续维护。

**对策**：

-分类器规则 YAML化（已实现），支持热更新。
-离线报表监控分类器的 fallback_used比例，超过阈值告警。
- Agent端的 governance_topics 作为软建议持续训练分类器。

###10.3跨皇通信死锁风险

三个 Worker 通过共享 `proposal_groups` 表通信，可能出现：Worker A 等 Worker B投票，Worker B 等 Worker A投票。

**对策**：

- 每个 Worker独立 in-flight队列，不共享锁。
-投票写入 `soul_review_votes` 是幂等的，重投视为同一票。
- 超时机制：30 分钟未投票 → abstain，不阻塞流程。
-状态机推进是事件驱动（投票写入即推进），不是轮询等待。

###10.4 首审皇偏见风险

初审仅1 人，首审皇的个人偏好可能主导结果。

**对策**：

- 双皇复审提供制度性制衡。
-反对意见存档形成长期记忆，同类提案累积反对会自动转 admin。
- 首审皇的 rationale强制要求详细，且必须引用 evidence_refs（防止黑箱）。
- Phase3+引入"重大议题三皇共审通道"作为兜底。

###10.5 真源层缺失风险

Phase2启动时，神农/轩辕的真源层可能不完善，导致审议质量下降。

**对策**：

- Phase1阶段专门建设真源层，不审议只采集。
- 真源层指标 YAML化，支持热更新和回滚。
- 神农/轩辕的硬规则初始为空（不预设策略），让数据说话后再加规则。

###10.6审计可解释性风险

三皇审议的可解释性比单皇差。需要详细记录每皇的真源引用、判定依据、反对意见。

**对策**：

- `soul_review_votes` 表强制要求 `rationale` 和 `evidence_refs`。
-仪表盘按时间窗 + 按皇展示历史投票记录。
- dissent_log 作为长期存档，支持同类提案的回溯分析。

##11. 与原方案的关系

本文档是 `action-evolution-governance.design.md` 的**演进版本**，不是替代。

|维度 | 原方案（伏羲审议链） | 本方案（三皇议会） |
|---|---|---|
|审议主体 | FuxiWorker 单 worker | FuxiWorker + ShennongWorker + XuanyuanWorker |
|审议路由 | 无 | TopicClassifier + RoutePlan（单皇首审） |
| 首审人数 |1（伏羲） |1（按议题优先级动态） |
|复审人数 |0 |2（其余两皇） |
|投票语义 | 单皇独立通过/否决 |2/3多数决 |
|复议机制 | 无 | 无（去掉强制复议） |
|反对意见 | 不存档 |完整存档（dissent_log） |
| 真源层 | Capability Manifest | Manifest + Resource/Survival/Order/Ethics Indicators |
| Phase0范围 |提案落库 + IR闸门 |继承 + primary_soul落库 |
|治理深度 |浅（一层审议） | 深（首审 + 双皇复审） |
|治理成本 | 低 | 中（Phase2 后约为2–3 倍 LLM成本） |
|治理哲学 | 单皇代理 | 分权制衡（首审深、复审广） |

**迁移建议**：

-现有原方案的 Phase0实施可以保留，本文档在其基础上扩展。
- 原方案的 `Fuxi Review Worker`升级为本方案的 `FuxiWorker`，逻辑兼容。
- 新增的 `TopicClassifier` / `RoutePlan` /跨皇通信 /投票语义判定器是新增组件，原方案无对应。
- 数据库迁移：`proposal_groups` 新增字段为非破坏性（nullable default），`soul_review_votes` 表为新建表。

---

##附录 A：核心数据结构

### A.1 ProposedActionIR（Agent端 + Server端共享）

```rust
struct ProposedActionIR {
 actor_arity: u8,
 target_arity: TargetArity,
 tick_span: u8,
 phase_count: u8,
 protocol_kind: ProtocolKind,
 state_transition_count: u8,
 effect_refs: Vec<String>,
 requirement_refs: Vec<String>,
}

enum TargetArity {
 Zero,
 One,
 Many,
 ZeroToMany,
}

enum ProtocolKind {
 None,
 TwoParty,
 MultiParty,
 Staged,
}
```

### A.2 GovernanceTopic

```rust
enum GovernanceTopic {
 Evolution, //演化
 Resource, //资源/生存
 Order, //秩序/伦理
}

impl GovernanceTopic {
 fn primary_soul(&self) -> Soul {
 match self {
 Evolution => Soul::Fuxi,
 Resource => Soul::Shennong,
 Order => Soul::Xuanyuan,
 }
 }
}
```

### A.3 RoutePlan

```rust
struct RoutePlan {
 primary: Soul, //唯一首审皇
 co_reviewers: Vec<Soul>, //复审皇列表（永远2 个，除非分类失败）
 mode: RouteMode,
}

enum RouteMode {
 SinglePrimary, // 单皇首审 + 双皇复审（默认）
 EscalateToAdmin, //分类失败：转人工
}
```

### A.4 ReviewRole

```rust
enum ReviewRole {
 Primary, // 首审：深度分析
 CoReviewer, //复审：基于真源对齐
}
```

##附录 B：示例流程

### B.1 示例1：单议题通过（伏羲首审）

```text
1. Agent提交 proposal，IR.effect_refs = ["skill.combat.cleave"]
2. TopicClassifier → topics = [Evolution] → primary_soul = Fuxi
3. FuxiWorker 首审 → approve（理由：动作表达力 gap真实，引用 evidence_refs: ["manifest.action_gap.skill.combat"]）
4. ShennongWorker复审 → approve（理由：资源消耗可接受，引用 evidence_refs: ["survival.resource_yield"]）
5. XuanyuanWorker复审 → approve（理由：不违反伦理边界，引用 evidence_refs: ["ethics.red_line"]）
6.投票判定：3 approve → approved
7. 白名单判定：cleave 在 allowed_capability_groups → 生成 ActionConfig
8. 热更新 → ACK收敛 → closed_approved
```

### B.2 示例2：双议题通过（伏羲首审，神农/轩辕复审）

```text
1. Agent提交 proposal，IR.effect_refs = ["mining.deep_shaft"]
2. TopicClassifier → topics = [Evolution, Resource] → primary_soul = Fuxi（evolution优先级最高）
3. FuxiWorker 首审 → approve（理由：演化需求真实，详细分析新技能的 schema必要性）
4. ShennongWorker复审 → approve（理由：deep_shaft提升资源产出符合生态，引用 evidence_refs: ["resource.yield_rate"]）
5. XuanyuanWorker复审 → reject（理由：deep_shaft 会破坏地表结构，影响帮派领地稳定，引用 evidence_refs: ["order.faction_conflict_index"]）
6.投票判定：2 approve /1 reject → approved（反对意见存档）
7. dissent_log记录 Xuanyuan 的反对意见
8. 白名单判定 → 生成 ActionConfig
9. 热更新 → ACK收敛 → closed_approved
```

### B.3 示例3：2反对1 通过 → 否决

```text
1. Agent提交 proposal，IR.effect_refs = ["combat.mass_exterminate"]
2. TopicClassifier → topics = [Evolution, Order] → primary_soul = Fuxi
3. FuxiWorker 首审 → approve（演化需求真实）
4. ShennongWorker复审 → reject（资源消耗不可接受）
5. XuanyuanWorker复审 → reject（伦理红线）
6.投票判定：1 approve /2 reject → rejected
7. closed_rejected，dissent_log存档2 条反对意见
8. 同源提案累计反对 ≥3 时，自动转 admin
```

### B.4 示例4：1/1/1 分裂 → 转人工

```text
1. Agent提交 proposal，IR.effect_refs = ["economy.complex.barter"]
2. TopicClassifier → topics = [Evolution, Resource, Order] → primary_soul = Fuxi
3. FuxiWorker 首审 → approve（演化需求真实）
4. ShennongWorker复审 → reject（资源配额不合理）
5. XuanyuanWorker复审 → reject（但与神农理由不同：违反秩序规则）
6.投票判定：1 approve /2 reject → rejected
```

> 如果改成1/1/1：例如 Xuanyuan投 abstain，则1 approve /1 reject /1 abstain → escalated_admin。

---

**文档版本**: v1.1（按用户反馈修订：初审仅1 人 +2/3多数决 + 无强制复议）
**适用阶段**: Phase0实施 + Phase1/2/3+规划
**前置依赖**: `action-evolution-governance.design.md` (Phase0边界)
**后续演进**: Phase3启动时单独成文补充火云洞天扩展议题
