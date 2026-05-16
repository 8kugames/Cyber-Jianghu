# Token 优化实现计划 v2：注意力门控 + Tool-First 架构

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 Agent 每 24h ~58M token 消耗削减 60-75%，通过注意力门控 + Tool-First 架构。

**Architecture:** Server 全量推送不变（配置改动除外），Agent 本地落存 WorldState。Delta Engine 检测变化，Attention Controller（规则过滤 + 轻量 LLM 排序）生成 Focus Summary，Prompt 只含焦点数据。其余通过 EarthSoul tool calling 按需取用。ReflectorSoul 消灭 13 轮重试循环，改为单次+同轮自修正。

**Tech Stack:** Rust, tokio async runtime, Arc<RwLock<T>> shared state, serde YAML config, existing EarthSoul tool calling framework

**Spec:** `docs/superpowers/specs/2026-05-14-token-optimization-design.md`

**v2 修订说明:** 基于 3 位审查员投票（1 APPROVE / 2 REJECT），修复 10 项关键问题。主要变更：
1. **Phase 重排**: ReflectorSoul 重构前置为 Phase 0（最大单项收益，零架构改动）
2. **Server 约束修复**: TokenOptimizationConfig 移到 Agent crate 内部，配置放 agent.yaml
3. **混合模型完整实现**: Phase 2 轻量 LLM 排序不再暂缓，纳入实施
4. **Token 经济学修正**: 精简 tool 数量，合并功能相近的 tool，降低 tool definitions 固定开销
5. **self_correct 修复**: 使用标准 multi-turn message，不伪造 tool_result
6. **Multi-intent 兼容**: 显式处理 multi-intent pipeline

**Triple-Review 投票结果 (2026-05-14):** 2/3 APPROVE (通过)
- Architecture Voter: APPROVE — 符合项目惯例，数据驱动，无 YAGNI 违规
- Scope Voter: APPROVE — 8 项用户需求全覆盖，无范围蔓延
- Feasibility Voter: REJECT — self_correct 使用了不存在的 Conversation API（已修复：改为构造临时 Vec<ConversationTurn>）

---

## Token 经济学精算

### Tool Calling vs 全量 Prompt 对比

当前 tool calling 已有 8 个 tools。新增 5 个后共 13 个，tool definitions 固定开销 ~1.3-2K tokens/LLM调用。

**优化策略：合并功能相近的 tool，控制总量**:

| 合并前 | 合并后 | 节省 |
|--------|--------|------|
| `query_inventory()` + `nearby_entities()` + `query_environment()` + `get_state_detail()` | `query_world(section, filter?)` | 3 个 tool definition 的 schema 开销 |

最终 tool 列表（共 11 个，比原方案少 2 个）:

| 类别 | Tools |
|------|-------|
| 已有 (8) | skill_view, search_memory, recall_archived, get_relationship, list_relationships, record_social_event, list_known_recipes, view_recipe_detail |
| 新增 (3) | `get_action_detail(action_type)`, `query_world(section, filter?)`, `list_skills()` |

`query_world` 的 section 参数: `"inventory"` | `"entities"` | `"environment"` | `"state"` | `"recipes"` | `"events"`

### 各路径 token 预算

| 路径 | 固定开销 | 1 tool call | 2 tool calls | 3 tool calls |
|------|---------|------------|-------------|-------------|
| 当前全量 prompt | 4-8K | - | - | - |
| Lean prompt + tools | 1.2-1.8K(tool defs) + 1.5-2.5K(prompt) = 2.7-4.3K | +0.5-1K | +1-2K | +1.5-3K |
| **总计** | **2.7-4.3K** | **3.2-5.3K** | **3.7-6.3K** | **4.2-7.3K** |

**结论**: 0-1 次 tool call 时显著节省（40-60%），2 次时持平，3 次时可能反超。因此 **max_tool_rounds 应从 3 降为 2**，且 Critical 预加载进一步降低 LLM 发起 tool call 的需求。

---

## File Structure

### New Files
| File | Responsibility |
|------|---------------|
| `crates/agent/src/component/state_store.rs` | WorldStateStore: 全量落存 + prev/curr + delta 触发 |
| `crates/agent/src/component/delta_engine.rs` | Delta Engine: prev vs curr 对比, StateChange 生成 |
| `crates/agent/src/component/attention.rs` | Attention Controller: 规则过滤 + 轻量 LLM 排序（完整实现） |
| `crates/agent/src/soul/earth/state_tool.rs` | 3 个新 EarthSoul tools (get_action_detail, query_world, list_skills) |
| `crates/agent/src/config/token_optimization.rs` | TokenOptimizationConfig 类型（Agent crate 内部） |

### Modified Files
| File | Change |
|------|--------|
| `crates/agent/src/component/mod.rs` | 添加 3 个新 module 声明 |
| `crates/agent/src/component/llm/token_tracking.rs` | 扩展 per-component token 计量 + prompt section 估算 |
| `crates/agent/src/config.rs` or `crates/agent/src/config/mod.rs` | 添加 token_optimization module |
| `crates/agent/src/core/agent.rs` | 添加 `world_state_store` 字段 |
| `crates/agent/src/core/builder.rs` | Builder 初始化 WorldStateStore |
| `crates/agent/src/core/lifecycle.rs` | WorldState 更新走 WorldStateStore + 替换 retry 循环 |
| `crates/agent/src/soul/actor/engine.rs` | Lean prompt 集成 attention |
| `crates/agent/src/soul/actor/engine_prompts.rs` | 新增 lean prompt 构建方法 |
| `crates/agent/src/soul/earth/executor.rs` | EarthToolContext 添加 WorldStateStore + 新 tool 路由 |
| `crates/agent/src/soul/earth/mod.rs` | 新 tool module 声明 |
| `crates/agent/src/soul/earth/config.rs` | max_tool_rounds 默认值从 3 降为 2 |
| `crates/agent/src/infra/api/mod.rs` | HttpApiState 改用 WorldStateStore |
| `crates/agent/src/infra/api/handlers.rs` | http_decision 同步迁移 WorldStateStore |

---

## Chunk 1: Phase 0a — Instrumentation + ReflectorSoul 重构

> **为什么前置 ReflectorSoul**: 13 轮重试循环是最大 token 黑洞（最坏 ~62-123K tokens/tick）。消灭它是纯代码重构，不依赖任何新组件，单独即可在最坏场景下削减 >90%。

### Task 1: Instrumentation — 扩展 token_tracking + prompt section 估算

**Files:**
- Modify: `crates/agent/src/component/llm/token_tracking.rs`
- Modify: `crates/agent/src/soul/actor/engine_prompts.rs`
- Test: `crates/agent/tests/test_token_metrics.rs`

- [ ] **Step 1: 添加 component 标签枚举和 ComponentMetrics 结构**

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LlmComponent {
    CognitiveEngine,
    AttentionController,
    ReflectorLayer3,
    ToolCalling,
    SocialProcessing,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ComponentMetrics {
    pub call_count: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
}
```

- [ ] **Step 2: 扩展 record_token_usage 接受可选 component**

签名添加 `component: Option<LlmComponent>`。新增全局 `OnceLock<Mutex<HashMap<LlmComponent, ComponentMetrics>>>` 收集 per-component 数据。

- [ ] **Step 3: 添加 prompt section token 估算**

在 `engine_prompts.rs` 的 `build_direct_prompt()` 组装后，按 section 估算字符数:

```rust
pub struct PromptSectionEstimate {
    pub system: usize,
    pub persona: usize,
    pub world_state: usize,
    pub action_descriptions: usize,
    pub memory: usize,
    pub skill_instructions: usize,
    pub other: usize,
}
```

字符数 / 4 作为粗估 tokens，写入 `tracing::info!` 每tick输出。

- [ ] **Step 4: 写测试 + 运行**

```bash
cargo nextest run -p cyber-jianghu-agent test_token_metrics
```

- [ ] **Step 5: 在 CognitiveEngine 和 ReflectorSoul LLM 调用后插入计量**

`engine.rs` think_direct LLM 调用后: `record_token_usage(model, input, output, Some(CognitiveEngine))`
`validator.rs` Layer 3 LLM 调用后: `record_token_usage(model, input, output, Some(ReflectorLayer3))`

- [ ] **Step 6: 编译 + 测试 + Commit**

```bash
cargo build -p cyber-jianghu-agent && cargo nextest run --workspace
git commit -m "feat(agent): add per-component token metrics and prompt section estimation"
```

### Task 2: ReflectorSoul — 消灭 13 轮重试循环

**Files:**
- Modify: `crates/agent/src/core/lifecycle.rs:1280-1563`
- Modify: `crates/agent/src/soul/actor/engine.rs` (self_correct 方法)

- [ ] **Step 1: 理解当前 retry 循环中的 multi-intent 流程**

`lifecycle.rs:1294-1563` 的循环内:
- ActorSoul 生成可能包含多个 intent (multi-intent pipeline, line 1382-1426)
- 每个 intent 独立走 ReflectorSoul 验证
- Chaos generator 可注入额外 intents
- 拒绝反馈通过 `set_rejection_feedback()` 传递给下一轮 ActorSoul

**新流程必须保持**: 单次 ActorSoul 生成 -> multi-intent 逐个验证 -> 拒绝的 intent 自修正 -> 仍失败则 chaos_fallback 该 intent。

- [ ] **Step 2: 替换 retry 循环为固定 3 阶段流程**

```rust
// 阶段 1: ActorSoul 生成 intents (一次)
let raw_intents = self.generate_intents(&world_state, &memory_context, fb).await?;
let all_intents = self.apply_chaos_and_pipeline(raw_intents, &world_state).await?;

// 阶段 2: 逐个验证 + 自修正
let mut approved_intents = Vec::new();
for intent in all_intents {
    match self.validate_with_reflector(intent, &world_state, graded_config).await? {
        Approved(intent) => approved_intents.push(intent),
        Rejected { intent, reason } => {
            if self.config.token_optimization().reflector.self_correction {
                match self.self_correct(intent, reason, &conversation, &tools, &executor).await {
                    Ok(corrected) => match self.validate_with_reflector(corrected, &world_state, graded_config).await? {
                        Approved(intent) => approved_intents.push(intent),
                        Rejected { .. } => approved_intents.push(self.chaos_fallback_intent(&world_state).await?),
                    },
                    Err(_) => approved_intents.push(self.chaos_fallback_intent(&world_state).await?),
                }
            } else {
                approved_intents.push(self.chaos_fallback_intent(&world_state).await?);
            }
        }
    }
}

// 阶段 3: 提交
for intent in approved_intents { self.submit_intent(intent).await; }
```

- [ ] **Step 3: 实现 self_correct — 标准 multi-turn message**

`ConversationInput` 是不可变值类型（`{summary, turns: &[ConversationTurn], current_prompt}`），不能动态 append。修正方案：构造临时 `Vec<ConversationTurn>` 扩展 turns。

```rust
async fn self_correct(
    &self,
    rejected_intent: Intent,
    reason: String,
    original_input: &ConversationInput<'_>,
    tools: &[ToolDefinition],
    executor: &EarthToolExecutor,
) -> Result<Intent> {
    // 构造扩展的 turns: 原始 turns + 被拒 intent(assistant) + 拒绝原因(user)
    let mut extended_turns: Vec<ConversationTurn> = original_input.turns.to_vec();
    extended_turns.push(ConversationTurn {
        role: "assistant".into(),
        content: serde_json::to_string(&rejected_intent)?,
    });
    extended_turns.push(ConversationTurn {
        role: "user".into(),
        content: format!("你的意图验证未通过。原因：{reason}。请修正后重新输出。"),
    });

    let corrected_input = ConversationInput {
        summary: original_input.summary,
        turns: &extended_turns,
        current_prompt: "请输出修正后的意图。",
    };

    self.llm_client.complete_json_with_tools::<DirectCognitiveResponse>(
        &corrected_input,
        tools,
        executor,
        1,  // 自修正时限制 tool call 轮次
    ).await
}
```

注意: 需确认 `ConversationTurn` 的实际字段名（可能通过 ConversationHistory API 构造）。实现时从 `conversation_history.push_turn(tick_id, user, assistant)` 或直接构造 struct。

- [ ] **Step 4: 实现 tick 级 LLM 失败计数器**

```rust
// 在 tick 循环开始时重置
let llm_fail_count = AtomicU32::new(0);
// 在任何 LLM 调用失败时递增
// 达到 chaos_on_llm_fail 阈值时，跳过后续 LLM 调用，直接 chaos_fallback
```

- [ ] **Step 5: 添加 Agent 侧 TokenOptimizationConfig**

在 `crates/agent/src/config/` 下新建 `token_optimization.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TokenOptimizationConfig {
    pub enabled: bool,                    // 默认 false (安全回退)
    pub reflector: ReflectorOptConfig,
    pub attention: AttentionConfig,
    pub delta: DeltaConfig,
    pub tool_preload: ToolPreloadConfig,
}
// + 各子结构体及 Default 实现
```

在 `agent.yaml` 中添加配置块（非 game_rules.yaml），从 agent 本地配置读取。

- [ ] **Step 6: 编译 + 全量测试**

```bash
cargo build -p cyber-jianghu-agent && cargo nextest run --workspace
```

- [ ] **Step 7: Commit**

```bash
git commit -m "refactor(agent): replace 13-round retry loop with single-turn + self-correction"
```

---

## Chunk 2: Phase 0b — WorldStateStore + Delta Engine

### Task 3: 实现 WorldStateStore

**Files:**
- Create: `crates/agent/src/component/state_store.rs`
- Modify: `crates/agent/src/component/mod.rs`
- Modify: `crates/agent/src/core/agent.rs`
- Modify: `crates/agent/src/core/builder.rs`

- [ ] **Step 1: 写测试**

```rust
#[test]
async fn test_update_sets_curr_and_prev() {
    let store = WorldStateStore::new(make_ws(1));
    assert_eq!(store.current().await.tick_id, 1);
    assert!(store.previous().await.is_none());
    store.update(make_ws(2)).await;
    assert_eq!(store.current().await.tick_id, 2);
    assert_eq!(store.previous().await.unwrap().tick_id, 1);
}
```

- [ ] **Step 2: 实现 WorldStateStore**

```rust
pub struct WorldStateStore {
    // 单把锁保护 (prev, curr) 原子更新，避免双 RwLock 写饥饿
    state: Arc<RwLock<(Option<WorldState>, WorldState)>>,
}

impl WorldStateStore {
    pub fn new(initial: WorldState) -> Self {
        Self { state: Arc::new(RwLock::new((None, initial))) }
    }
    pub async fn update(&self, new_state: WorldState) {
        let mut guard = self.state.write().await;
        let prev = std::mem::replace(&mut guard.1, new_state);
        guard.0 = Some(prev);
    }
    pub async fn current(&self) -> WorldState {
        self.state.read().await.1.clone()
    }
    pub async fn previous(&self) -> Option<WorldState> {
        self.state.read().await.0.clone()
    }
}
```

- [ ] **Step 3: 在 Agent + Builder 中添加字段**

Agent struct: `pub(crate) world_state_store: Option<Arc<WorldStateStore>>`
AgentBuilder: `with_world_state_store()` 方法

- [ ] **Step 4: lifecycle.rs 迁移 WorldState 写入**

替换 `lifecycle.rs:784-785` 和 `infra/api/mod.rs` 中的 `current_state` 写入。
**同步修改 `infra/api/handlers.rs` 的 `http_decision` 函数**（Claw 模式入口），确保 Claw 模式下 WorldStateStore 也更新。

- [ ] **Step 5: 运行测试 + Commit**

```bash
cargo nextest run -p cyber-jianghu-agent test_state_store
git commit -m "feat(agent): add WorldStateStore with atomic prev/curr updates"
```

### Task 4: 实现 Delta Engine

**Files:**
- Create: `crates/agent/src/component/delta_engine.rs`
- Modify: `crates/agent/src/component/mod.rs`

- [ ] **Step 1: 写测试（覆盖所有 5 个 category）**

- [ ] **Step 2: 实现 StateChange + DeltaEngine**

关键设计: `detect_survival_changes` **遍历 `DeltaConfig.survival_thresholds` 的 keys** 匹配 attributes，而非硬编码字段名。新增监控属性只需改配置。

```rust
fn detect_survival_changes(
    curr_attrs: &HashMap<String, i32>,
    prev_attrs: &HashMap<String, i32>,
    config: &DeltaConfig,
) -> Vec<StateChange> {
    let mut changes = Vec::new();
    for (key, &threshold) in &config.survival_thresholds {
        let curr_val = curr_attrs.get(key).copied().unwrap_or(0);
        let prev_val = prev_attrs.get(key).copied().unwrap_or(0);
        if curr_val == prev_val { continue; }
        let urgency = if curr_val as f32 / 100.0 >= threshold {
            Urgency::Critical
        } else if (curr_val - prev_val).unsigned_abs() as f32 / 100.0 >= config.change_percentage_threshold {
            Urgency::Important
        } else {
            Urgency::Info
        };
        // ... build StateChange with tool_hint
    }
    changes
}
```

Social/Environment/Inventory/Location 检测目标是 WorldState 结构体的固定字段（`entities`, `events_log`, `inventory`, `node_id`），这些字段名与 WorldState serde schema 绑定，硬编码合理。

- [ ] **Step 3: 运行测试 + Commit**

```bash
cargo nextest run -p cyber-jianghu-agent test_delta_engine
git commit -m "feat(agent): add Delta Engine with configurable survival detection"
```

---

## Chunk 3: Phase 1 — Attention Controller（完整混合模型）

### Task 5: 实现 Attention Controller — 规则过滤 + 轻量 LLM 排序

**Files:**
- Create: `crates/agent/src/component/attention.rs`
- Modify: `crates/agent/src/component/mod.rs`

- [ ] **Step 1: 写测试**

覆盖:
- Phase 1: Critical 自动纳入, Important+Survival 纳入, Social 目标匹配, 候选集分离
- Phase 2: 轻量 LLM 从候选集中选取 Top-N（mock LLM 响应）
- Focus Summary 生成: 叙事格式 + tool hints
- first_tick_focus_cap 放宽

- [ ] **Step 2: 实现 Phase 1 规则过滤**

同 v1，但 `should_auto_include` 的规则优先级明确:
1. `Urgency::Critical` -> 自动纳入（`critical_auto_include` 控制）
2. `Urgency::Important` + `Category::Survival` -> 纳入
3. `Category::Social` + 目标在 `social_targets` -> 纳入
4. `active_tasks` 相关 -> 纳入
5. 其余 -> 候选集

- [ ] **Step 3: 实现 Phase 2 轻量 LLM 排序**

```rust
pub async fn rank_with_llm(
    &self,
    candidates: &[StateChange],
    remaining_slots: usize,
    location: &str,
    current_activity: &str,
) -> Result<Vec<StateChange>> {
    if candidates.is_empty() || remaining_slots == 0 {
        return Ok(Vec::new());
    }
    let prompt = format!(
        "以下是本轮状态变化候选集。请选出最多 {remaining_slots} 项最需要角色关注的变化。\n\
         角色当前在 {location}，正在 {current_activity}。\n\
         候选：{}\n\
         输出格式：JSON array of indices (0-based)",
        serde_json::to_string(candidates)?
    );
    // 使用配置中的 llm_ranking_model (默认 haiku)
    let response = self.llm_client.complete(&prompt).await?;
    let indices: Vec<usize> = serde_json::from_str(&response)?;
    Ok(indices.into_iter()
        .filter_map(|i| candidates.get(i).cloned())
        .take(remaining_slots)
        .collect())
}
```

- [ ] **Step 4: 实现 Focus Summary 生成**

叙事格式 + tool hints。tool hint 格式使用 `tool_name(arg=val)`:

```
[紧迫] 饥饿度升至72 (查询食物: query_world(section=inventory, filter=food))
[变化] 铁匠铺出现商人张三 (查询详情: query_world(section=entities, filter=张三))
[社交] 李四向你对话 (查询关系: get_relationship(target=李四))
[环境] 远处打斗声 (查询: query_world(section=environment))
```

- [ ] **Step 5: 运行测试 + Commit**

```bash
cargo nextest run -p cyber-jianghu-agent test_attention
git commit -m "feat(agent): add Attention Controller with rule filtering + lightweight LLM ranking"
```

### Task 6: Attention Controller 集成到 lifecycle

**Files:**
- Modify: `crates/agent/src/core/lifecycle.rs`

- [ ] **Step 1: 在 tick 循环中集成**

WorldState 更新后、prompt 组装前:
1. 从 WorldStateStore 获取 delta
2. 从 agent config 读取 TokenOptimizationConfig
3. `enabled == true`: 调用 AttentionController.filter() + rank_with_llm() + generate_summary()
4. 将 FocusSummary 传递给 CognitiveEngine
5. `enabled == false`: 跳过，走原有全量 prompt

- [ ] **Step 2: Commit**

```bash
git commit -m "feat(agent): integrate Attention Controller into tick lifecycle"
```

---

## Chunk 4: Phase 2 — EarthSoul Tools + Lean Prompt

### Task 7: 添加 3 个新 EarthSoul Tools

**Files:**
- Create: `crates/agent/src/soul/earth/state_tool.rs`
- Modify: `crates/agent/src/soul/earth/executor.rs`
- Modify: `crates/agent/src/soul/earth/mod.rs`

- [ ] **Step 1: 扩展 EarthToolContext**

```rust
pub struct EarthToolContext {
    // 已有字段...
    pub world_state_store: Option<Arc<WorldStateStore>>,
    pub available_actions: Option<Vec<AvailableAction>>,  // 非 ActionDescription
    pub skill_index: Option<Vec<(String, String)>>,       // (skill_id, skill_name)
}
```

- [ ] **Step 2: 实现 3 个新 tools**

**1. `get_action_detail(action_type: String)`**
- 从 `available_actions` 查找匹配 action
- 返回完整描述 + cost + requirements + field schema

**2. `query_world(section: String, filter: Option<String>)`**
- section: `"inventory"` | `"entities"` | `"environment"` | `"state"` | `"events"`
- filter: 可选过滤（如 `"food"`, `"武器"`, entity name）
- 从 WorldStateStore.current() 读取对应 section 数据
- 替代原来分散在 prompt 中的 inventory/entities/location/events/attributes 信息

**3. `list_skills()`**
- 返回已掌握技能的索引列表 (id + name + brief)
- 详细内容通过已有 `skill_view` tool 获取
- 替代 prompt 中的 Skill Instructions

- [ ] **Step 3: 注册到 executor + mod.rs**

- [ ] **Step 4: 写测试 + 运行**

```bash
cargo nextest run -p cyber-jianghu-agent test_state_tools
```

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(agent): add 3 EarthSoul tools (get_action_detail, query_world, list_skills)"
```

### Task 8: 实现 Lean Prompt Assembler

**Files:**
- Modify: `crates/agent/src/soul/actor/engine_prompts.rs`
- Modify: `crates/agent/src/soul/actor/engine.rs`
- Modify: `crates/agent/src/soul/earth/config.rs` (max_tool_rounds 降为 2)

- [ ] **Step 1: 添加 build_lean_direct_prompt()**

```
Lean Prompt 组成:
+ System Prompt (cached, ~200 tok)
+ Persona Summary (cached, ~100 tok)
+ Focus Summary (dynamic, ~200-500 tok, 含 tool hints)
+ Action Index (name + 一句话描述, 无消耗信息, ~200-300 tok)
+ Skill Index (name only, ~50-100 tok)
// 不含: WorldState 全量, Action Descriptions, Action Field Hints, Skill Instructions
```

- [ ] **Step 2: Action Index 实现**

```rust
pub fn build_action_index(actions: &[AvailableAction]) -> String {
    let mut s = String::from("## 可用动作 (查询详情: get_action_detail(action_name))\n\n");
    for action in actions {
        s.push_str(&format!("- {}: {}\n", action.action, action.description));
    }
    s
}
```

只用 `action` (name) + `description`，不引用不存在的 `stamina_cost` 字段。

- [ ] **Step 3: Skill Index 实现**

```rust
pub fn build_skill_index(skills: &[(String, String)]) -> String {
    let mut s = String::from("## 已掌握技能 (查询详情: skill_view(skill_id))\n\n");
    for (id, name) in skills {
        s.push_str(&format!("- {} ({})\n", name, id));
    }
    s
}
```

- [ ] **Step 4: 修改 think_direct 切换路径**

`token_optimization.enabled == true` -> `build_lean_direct_prompt()`
`token_optimization.enabled == false` -> 原有 `build_direct_prompt()`

- [ ] **Step 5: 降低 max_tool_rounds 默认值**

`crates/agent/src/soul/earth/config.rs` 中 `max_tool_rounds` 从 3 改为 2。

- [ ] **Step 6: 编译 + 测试 + Commit**

```bash
cargo nextest run --workspace
git commit -m "feat(agent): add lean prompt assembler with action index and skill index"
```

### Task 9: Critical 焦点预加载

**Files:**
- Modify: `crates/agent/src/soul/actor/engine.rs`

- [ ] **Step 1: Focus Summary 含 Critical 项时自动预加载**

当 `tool_preload.critical_preload == true` 且 Focus Summary 含 Critical:
- 识别 Critical 项的 category
- 直接在 Focus Summary 后面追加对应 `query_world` 结果
- 避免多一轮 tool call

- [ ] **Step 2: Commit**

```bash
git commit -m "feat(agent): add critical focus tool preload"
```

---

## Chunk 5: 集成验证

### Task 10: 集成测试 + 全量验证

**Files:**
- Test: `crates/agent/tests/test_token_optimization_e2e.rs`

- [ ] **Step 1: 写集成测试**

场景:
- `enabled = false` -> 原有全量 prompt 路径
- `enabled = true` -> lean prompt 路径
- 首轮 (无 prev) -> first_tick_focus_cap 生效
- Delta Engine 正确检测变化
- Attention Controller 规则过滤 + LLM 排序正确
- 新 tools 可被调用
- ReflectorSoul: 单次验证 + 自修正 + chaos_fallback
- Multi-intent: 多个 intent 逐个验证
- LLM 失败计数器: 达到阈值直接 chaos_fallback
- Skill Instructions 不出现在 lean prompt 中

- [ ] **Step 2: 全量验证**

```bash
cargo nextest run --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

- [ ] **Step 3: Commit**

```bash
git commit -m "test(agent): add token optimization integration tests"
```

---

## 实施注意事项

1. **Phase 0a (Chunk 1) 单独即可削减最坏场景 >90%** — 应立即上线并采集数据
2. **每 Task 完成后运行 `cargo nextest run --workspace`** 确保无回归
3. **`enabled = false` 是安全回退** — 任何阶段出问题可立即关闭
4. **max_tool_rounds = 2** — 超过 2 轮 tool call 时 token 消耗持平全量 prompt，无意义
5. **Phase 2 轻量 LLM 排序已纳入 Task 5** — 完整实现用户要求的混合模型
6. **Agent 侧配置** — TokenOptimizationConfig 在 agent.yaml，不修改 game_rules.yaml
7. **Multi-intent 保持兼容** — Task 2 的固定流程按 intent 逐个验证，保持 pipeline 语义
8. **Claw 模式同步迁移** — Task 3 同时处理 lifecycle.rs 和 http_decision 的 WorldStateStore 写入
