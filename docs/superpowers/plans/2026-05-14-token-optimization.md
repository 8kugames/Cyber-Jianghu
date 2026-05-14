# Token 优化实现计划：注意力门控 + Tool-First 架构

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 Agent 每 24h ~58M token 消耗削减 60-75%，通过注意力门控 + Tool-First 架构。

**Architecture:** Server 全量推送不变，Agent 本地落存 WorldState。Delta Engine 检测变化，Attention Controller 过滤+排序生成 Focus Summary，Prompt 只含焦点数据。其余通过 EarthSoul tool calling 按需取用。ReflectorSoul 改为单轮+同轮自修正。

**Tech Stack:** Rust, tokio async runtime, Arc<RwLock<T>> shared state, serde YAML config, existing EarthSoul tool calling framework

**Spec:** `docs/superpowers/specs/2026-05-14-token-optimization-design.md`

---

## File Structure

### New Files
| File | Responsibility |
|------|---------------|
| `crates/agent/src/component/state_store.rs` | WorldStateStore: 全量落存 + prev/curr + delta 触发 |
| `crates/agent/src/component/delta_engine.rs` | Delta Engine: prev vs curr 对比, StateChange 生成 |
| `crates/agent/src/component/attention.rs` | Attention Controller: 规则过滤 + 轻量 LLM 排序 |
| `crates/agent/src/soul/earth/state_tool.rs` | 5 个新 EarthSoul tools (action_detail, inventory, entities, environment, state) |

### Modified Files
| File | Change |
|------|--------|
| `crates/agent/src/component/mod.rs:5-10` | 添加 3 个新 module 声明 |
| `crates/agent/src/component/llm/token_tracking.rs` | 扩展 per-component token 计量 |
| `crates/agent/src/core/agent.rs:41-150` | 添加 `world_state_store` 字段 |
| `crates/agent/src/core/builder.rs:35-68` | Builder 初始化 WorldStateStore |
| `crates/agent/src/core/lifecycle.rs:784-785` | WorldState 更新走 WorldStateStore |
| `crates/agent/src/core/lifecycle.rs:1280-1563` | 替换 retry 循环为固定流程 |
| `crates/agent/src/soul/actor/engine.rs:691-987` | Lean prompt 集成 attention |
| `crates/agent/src/soul/actor/engine_prompts.rs:19-376` | 新增 lean prompt 构建方法 |
| `crates/agent/src/soul/earth/executor.rs:23-28` | EarthToolContext 添加 WorldStateStore |
| `crates/agent/src/soul/earth/executor.rs:65-196` | 新 tool 路由 |
| `crates/agent/src/soul/earth/mod.rs:14-26` | 新 tool module 声明 |
| `crates/agent/src/infra/api/mod.rs:211-225` | HttpApiState 改用 WorldStateStore |
| `crates/server/config/game_rules.yaml` | 添加 token_optimization 配置块 |
| `crates/protocol/src/types/rules.rs` | 添加 TokenOptimizationConfig 类型 |

---

## Chunk 1: Phase 0 — Instrumentation

### Task 1: 扩展 token_tracking.rs 添加 component 标签

**Files:**
- Modify: `crates/agent/src/component/llm/token_tracking.rs:14-50`
- Test: `crates/agent/tests/test_token_metrics.rs` (新建)

- [ ] **Step 1: 写 component 维度枚举**

在 `token_tracking.rs` 中添加:

```rust
/// LLM 调用组件标签, 用于 per-component token 计量
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum LlmComponent {
    CognitiveEngine,
    AttentionController,
    ReflectorLayer3,
    ToolCalling,
    SocialProcessing,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ComponentMetrics {
    pub call_count: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct TokenMetrics {
    pub components: std::collections::HashMap<LlmComponent, ComponentMetrics>,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub ticks_measured: u64,
}
```

- [ ] **Step 2: 扩展 record_token_usage 支持可选 component 标签**

修改 `record_token_usage()` 签名，添加 `component: Option<LlmComponent>` 参数。在 `PerModelStats` 同级维护一个 `OnceLock<Mutex<HashMap<LlmComponent, ComponentMetrics>>>` 全局状态。

- [ ] **Step 3: 写测试**

```rust
#[test]
fn test_component_metrics_recording() {
    token_tracking::record_token_usage(
        "test-model", 100, 50, Some(LlmComponent::CognitiveEngine)
    );
    let metrics = token_tracking::snapshot_component_metrics();
    let ce = metrics.get(&LlmComponent::CognitiveEngine).unwrap();
    assert_eq!(ce.call_count, 1);
    assert_eq!(ce.total_input_tokens, 100);
    assert_eq!(ce.total_output_tokens, 50);
}
```

- [ ] **Step 4: 运行测试**

```bash
cargo nextest run -p cyber-jianghu-agent test_component_metrics
```

- [ ] **Step 5: Commit**

```bash
git add crates/agent/src/component/llm/token_tracking.rs crates/agent/tests/test_token_metrics.rs
git commit -m "feat(agent): add per-component token metrics to token_tracking"
```

### Task 2: 在 CognitiveEngine LLM 调用点插入计量

**Files:**
- Modify: `crates/agent/src/soul/actor/engine.rs:691-987` (think_direct 方法)

- [ ] **Step 1: 在 think_direct 的 LLM 调用后添加计量**

在 `think_direct()` 中 `self.llm_client.complete_json_with_conversation_and_tools()` 返回结果后，提取 usage 信息并调用 `record_token_usage("model", input, output, Some(LlmComponent::CognitiveEngine))`。

- [ ] **Step 2: 在 ReflectorSoul Layer 3 调用后添加计量**

在 `crates/agent/src/soul/reflector/validator.rs` 的 LLM 验证调用后添加 `record_token_usage(..., Some(LlmComponent::ReflectorLayer3))`。

- [ ] **Step 3: 编译验证**

```bash
cargo build -p cyber-jianghu-agent
```

- [ ] **Step 4: Commit**

```bash
git add crates/agent/src/soul/actor/engine.rs crates/agent/src/soul/reflector/validator.rs
git commit -m "feat(agent): instrument token usage in CognitiveEngine and ReflectorSoul"
```

### Task 3: 添加 token_optimization 配置类型

**Files:**
- Modify: `crates/protocol/src/types/rules.rs` (末尾追加)
- Modify: `crates/server/config/game_rules.yaml` (末尾追加)

- [ ] **Step 1: 在 rules.rs 添加配置类型**

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct TokenOptimizationConfig {
    pub enabled: bool,
    pub attention: AttentionConfig,
    pub delta: DeltaConfig,
    pub tool_preload: ToolPreloadConfig,
    pub reflector: ReflectorOptConfig,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AttentionConfig {
    pub max_focus_items: usize,
    pub first_tick_focus_cap: usize,
    pub critical_auto_include: bool,
    pub enable_llm_ranking: bool,
    pub llm_ranking_model: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct DeltaConfig {
    pub survival_thresholds: std::collections::HashMap<String, f32>,
    pub change_percentage_threshold: f32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ToolPreloadConfig {
    pub enabled: bool,
    pub critical_preload: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ReflectorOptConfig {
    pub self_correction: bool,
    pub chaos_on_double_reject: bool,
    pub chaos_on_llm_fail: u32,
}
```

为每个结构体实现 `Default`，值与 spec Section 8 一致。

- [ ] **Step 2: 在 GameRules 结构体中添加 token_optimization 字段**

找到 `GameRules` 结构体，添加 `pub token_optimization: Option<TokenOptimizationConfig>`。

- [ ] **Step 3: 在 game_rules.yaml 末尾添加配置块**

按 spec Section 8 的 YAML 内容添加。

- [ ] **Step 4: 写测试 — 配置反序列化**

```rust
#[test]
fn test_token_optimization_config_deserialize() {
    let yaml = r#"
token_optimization:
  enabled: true
  attention:
    max_focus_items: 5
"#;
    let rules: GameRules = serde_yaml::from_str(yaml).unwrap();
    let opt = rules.token_optimization.unwrap();
    assert!(opt.enabled);
    assert_eq!(opt.attention.max_focus_items, 5);
}
```

- [ ] **Step 5: 运行测试**

```bash
cargo nextest run -p cyber-jianghu-protocol test_token_optimization
```

- [ ] **Step 6: Commit**

```bash
git add crates/protocol/src/types/rules.rs crates/server/config/game_rules.yaml
git commit -m "feat(protocol): add TokenOptimizationConfig types and game_rules.yaml config"
```

---

## Chunk 2: Phase 1 — WorldStateStore + Delta Engine

### Task 4: 实现 WorldStateStore

**Files:**
- Create: `crates/agent/src/component/state_store.rs`
- Modify: `crates/agent/src/component/mod.rs:10` (追加 module 声明)
- Modify: `crates/agent/src/core/agent.rs` (添加字段)
- Modify: `crates/agent/src/core/builder.rs` (初始化)
- Test: `crates/agent/tests/test_state_store.rs` (新建)

- [ ] **Step 1: 写 WorldStateStore 测试**

```rust
use cyber_jianghu_agent::component::state_store::WorldStateStore;
use cyber_jianghu_protocol::types::world::WorldState;

fn make_ws(tick: i64) -> WorldState {
    let mut ws = WorldState::default();
    ws.tick_id = tick;
    ws
}

#[test]
fn test_update_sets_curr_and_prev() {
    let store = WorldStateStore::new(make_ws(1));
    assert_eq!(store.current().tick_id, 1);
    assert!(store.previous().is_none());

    store.update(make_ws(2));
    assert_eq!(store.current().tick_id, 2);
    assert_eq!(store.previous().unwrap().tick_id, 1);
}

#[test]
fn test_first_tick_delta() {
    let store = WorldStateStore::new(make_ws(1));
    let delta = store.compute_delta();
    assert!(delta.is_first_tick);
}
```

- [ ] **Step 2: 运行测试验证失败**

```bash
cargo nextest run -p cyber-jianghu-agent test_state_store
```

Expected: FAIL (module not found)

- [ ] **Step 3: 实现 WorldStateStore**

```rust
// crates/agent/src/component/state_store.rs
use std::sync::Arc;
use tokio::sync::RwLock;
use cyber_jianghu_protocol::types::world::WorldState;
use crate::component::delta_engine::StateDelta;

pub struct WorldStateStore {
    curr: Arc<RwLock<WorldState>>,
    prev: Arc<RwLock<Option<WorldState>>>,
}

impl WorldStateStore {
    pub fn new(initial: WorldState) -> Self {
        Self {
            curr: Arc::new(RwLock::new(initial)),
            prev: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn update(&self, new_state: WorldState) {
        let mut prev = self.prev.write().await;
        let mut curr = self.curr.write().await;
        *prev = Some(std::mem::replace(&mut *curr, new_state));
    }

    pub async fn current(&self) -> WorldState {
        self.curr.read().await.clone()
    }

    pub async fn previous(&self) -> Option<WorldState> {
        self.prev.read().await.clone()
    }

    pub async fn compute_delta(&self) -> StateDelta {
        let curr = self.curr.read().await;
        let prev = self.prev.read().await;
        crate::component::delta_engine::DeltaEngine::compute(&curr, prev.as_ref())
    }
}
```

- [ ] **Step 4: 在 component/mod.rs 追加声明**

```rust
pub mod state_store;
```

- [ ] **Step 5: 在 Agent struct 添加字段**

在 `agent.rs` 的 Agent struct 中添加:
```rust
pub(crate) world_state_store: Option<Arc<WorldStateStore>>,
```

添加 getter/setter 方法:
```rust
pub fn world_state_store(&self) -> Option<&Arc<WorldStateStore>> {
    self.world_state_store.as_ref()
}

pub fn set_world_state_store(&mut self, store: Arc<WorldStateStore>) {
    self.world_state_store = Some(store);
}
```

- [ ] **Step 6: 在 AgentBuilder 添加初始化**

```rust
pub fn with_world_state_store(mut self, store: Arc<WorldStateStore>) -> Self {
    self.agent.world_state_store = Some(store);
    self
}
```

- [ ] **Step 7: 运行测试验证通过**

```bash
cargo nextest run -p cyber-jianghu-agent test_state_store
```

- [ ] **Step 8: Commit**

```bash
git add crates/agent/src/component/state_store.rs crates/agent/src/component/mod.rs crates/agent/src/core/agent.rs crates/agent/src/core/builder.rs crates/agent/tests/test_state_store.rs
git commit -m "feat(agent): add WorldStateStore for local state persistence"
```

### Task 5: 实现 Delta Engine

**Files:**
- Create: `crates/agent/src/component/delta_engine.rs`
- Modify: `crates/agent/src/component/mod.rs` (追加 module 声明)
- Test: `crates/agent/tests/test_delta_engine.rs` (新建)

- [ ] **Step 1: 写 Delta Engine 类型定义和测试**

测试用例覆盖:
- 首轮 (无 prev): 全量 Important StateChange
- Survival: HP/hunger/thirst 超阈值 -> Critical
- Survival: 变化 < 10% -> Info
- Social: 新 entity 出现 -> Important
- Social: entity 消失 -> Important
- Social: 仅 action 变化 -> Info
- Environment: 新事件 -> Important
- Inventory: 物品数量变化 -> Important/Info
- Location: node_id 变化 -> Important
- Tool hint 生成正确性

- [ ] **Step 2: 运行测试验证失败**

```bash
cargo nextest run -p cyber-jianghu-agent test_delta_engine
```

- [ ] **Step 3: 实现 Delta Engine**

核心结构:

```rust
// crates/agent/src/component/delta_engine.rs
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use cyber_jianghu_protocol::types::world::WorldState;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ChangeCategory { Survival, Social, Environment, Inventory, Location }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Urgency { Critical, Important, Info }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateChange {
    pub category: ChangeCategory,
    pub urgency: Urgency,
    pub field: String,
    pub description: String,
    pub data: Value,
    pub tool_hint: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StateDelta {
    pub changes: Vec<StateChange>,
    pub is_first_tick: bool,
}

pub struct DeltaEngine;

impl DeltaEngine {
    pub fn compute(curr: &WorldState, prev: Option<&WorldState>) -> StateDelta {
        match prev {
            None => Self::first_tick_delta(curr),
            Some(prev) => Self::incremental_delta(curr, prev),
        }
    }

    fn first_tick_delta(curr: &WorldState) -> StateDelta { /* 全量生成 */ }
    fn incremental_delta(curr: &WorldState, prev: &WorldState) -> StateDelta { /* 逐字段对比 */ }

    // 5 个检测方法
    fn detect_survival_changes(...) -> Vec<StateChange> { /* hp/hunger/thirst */ }
    fn detect_social_changes(...) -> Vec<StateChange> { /* entities diff */ }
    fn detect_environment_changes(...) -> Vec<StateChange> { /* events_log diff */ }
    fn detect_inventory_changes(...) -> Vec<StateChange> { /* inventory diff */ }
    fn detect_location_changes(...) -> Vec<StateChange> { /* node_id diff */ }
}
```

- [ ] **Step 4: 在 component/mod.rs 追加声明**

```rust
pub mod delta_engine;
```

- [ ] **Step 5: 运行测试验证通过**

```bash
cargo nextest run -p cyber-jianghu-agent test_delta_engine
```

- [ ] **Step 6: Commit**

```bash
git add crates/agent/src/component/delta_engine.rs crates/agent/src/component/mod.rs crates/agent/tests/test_delta_engine.rs
git commit -m "feat(agent): add Delta Engine for WorldState change detection"
```

### Task 6: lifecycle.rs 集成 WorldStateStore

**Files:**
- Modify: `crates/agent/src/core/lifecycle.rs:784-785` (WorldState 更新)
- Modify: `crates/agent/src/infra/api/mod.rs:211-225` (HttpApiState 迁移)

- [ ] **Step 1: 替换 lifecycle.rs 中的 current_state 写入**

将 `lifecycle.rs:784-785` 的:
```rust
let mut current = api_state.current_state.write().await;
*current = Some(world_state.clone());
```

替换为:
```rust
if let Some(store) = self.world_state_store() {
    store.update(world_state.clone()).await;
}
```

- [ ] **Step 2: HttpApiState 迁移到 WorldStateStore**

在 `infra/api/mod.rs` 中:
- 添加 `world_state_store: Option<Arc<WorldStateStore>>` 字段
- `GET /api/v1/state` handler 改为从 `world_state_store.current()` 读取
- 保留 `current_state` 字段但标记 `#[deprecated]`，过渡期双写

- [ ] **Step 3: 编译验证**

```bash
cargo build -p cyber-jianghu-agent
```

- [ ] **Step 4: Commit**

```bash
git add crates/agent/src/core/lifecycle.rs crates/agent/src/infra/api/mod.rs
git commit -m "refactor(agent): integrate WorldStateStore into lifecycle and HTTP API"
```

---

## Chunk 3: Phase 2 — Attention Controller

### Task 7: 实现 Attention Controller

**Files:**
- Create: `crates/agent/src/component/attention.rs`
- Modify: `crates/agent/src/component/mod.rs` (追加 module 声明)
- Test: `crates/agent/tests/test_attention.rs` (新建)

- [ ] **Step 1: 写 Attention Controller 测试**

测试用例:
- Phase 1: Critical urgency 自动纳入
- Phase 1: Important + Survival -> 纳入
- Phase 1: Social + 在 social_targets 中 -> 纳入
- Phase 1: 其余 -> 候选集
- Phase 1: max_focus_items 限制
- Phase 1: first_tick_focus_cap 放宽
- Focus Summary 生成: 含 tool hints
- Focus Summary 生成: 叙事格式正确

- [ ] **Step 2: 运行测试验证失败**

```bash
cargo nextest run -p cyber-jianghu-agent test_attention
```

- [ ] **Step 3: 实现 Attention Controller**

```rust
// crates/agent/src/component/attention.rs
use std::collections::HashMap;
use crate::component::delta_engine::{StateChange, ChangeCategory, Urgency, StateDelta};
use crate::protocol::types::rules::AttentionConfig;

pub struct FocusSummary {
    pub narrative: String,
    pub changes: Vec<StateChange>,
    pub focus_areas: Vec<String>,
}

pub struct AttentionController {
    config: AttentionConfig,
    survival_thresholds: HashMap<String, f32>,
    social_targets: Vec<uuid::Uuid>,
}

impl AttentionController {
    pub fn new(config: AttentionConfig, thresholds: HashMap<String, f32>) -> Self { ... }

    /// Phase 1: 规则过滤 (零 token)
    pub fn filter(&self, delta: &StateDelta) -> FilterResult {
        let mut auto_focus = Vec::new();
        let mut candidates = Vec::new();
        let cap = if delta.is_first_tick { self.config.first_tick_focus_cap } else { self.config.max_focus_items };

        for change in &delta.changes {
            if self.should_auto_include(change) {
                auto_focus.push(change.clone());
            } else {
                candidates.push(change.clone());
            }
        }
        FilterResult { auto_focus, candidates, cap }
    }

    /// 生成 Focus Summary (叙事格式 + tool hints)
    pub fn generate_summary(&self, focused: Vec<StateChange>) -> FocusSummary { ... }

    fn should_auto_include(&self, change: &StateChange) -> bool { ... }
    fn format_change_narrative(&self, change: &StateChange) -> String { ... }
    fn generate_tool_hint(&self, change: &StateChange) -> Option<String> { ... }
}

pub struct FilterResult {
    pub auto_focus: Vec<StateChange>,
    pub candidates: Vec<StateChange>,
    pub cap: usize,
}
```

Phase 2 (轻量 LLM 排序) 暂不实现，先只实现 Phase 1 规则过滤 + Focus Summary 生成。candidates 超过 cap 时按 Urgency 优先级截断。

- [ ] **Step 4: 在 component/mod.rs 追加声明**

```rust
pub mod attention;
```

- [ ] **Step 5: 运行测试验证通过**

```bash
cargo nextest run -p cyber-jianghu-agent test_attention
```

- [ ] **Step 6: Commit**

```bash
git add crates/agent/src/component/attention.rs crates/agent/src/component/mod.rs crates/agent/tests/test_attention.rs
git commit -m "feat(agent): add Attention Controller with rule-based filtering"
```

### Task 8: Attention Controller 集成到 lifecycle

**Files:**
- Modify: `crates/agent/src/core/lifecycle.rs` (tick 循环中调用 Attention Controller)

- [ ] **Step 1: 在 lifecycle tick 循环中集成**

在 WorldState 更新后、prompt 组装前:
1. 从 WorldStateStore 获取 delta
2. 从 game_rules 读取 TokenOptimizationConfig
3. 如果 `enabled == true`: 调用 AttentionController.filter() + generate_summary()
4. 将 FocusSummary 传递给 CognitiveEngine

- [ ] **Step 2: 编译验证**

```bash
cargo build -p cyber-jianghu-agent
```

- [ ] **Step 3: Commit**

```bash
git add crates/agent/src/core/lifecycle.rs
git commit -m "feat(agent): integrate Attention Controller into tick lifecycle"
```

---

## Chunk 4: Phase 3 — EarthSoul Tools + Lean Prompt

### Task 9: 添加 5 个新 EarthSoul Tools

**Files:**
- Create: `crates/agent/src/soul/earth/state_tool.rs`
- Modify: `crates/agent/src/soul/earth/executor.rs:23-28` (EarthToolContext)
- Modify: `crates/agent/src/soul/earth/executor.rs:49-61` (tool_definitions)
- Modify: `crates/agent/src/soul/earth/executor.rs:65-196` (execute 路由)
- Modify: `crates/agent/src/soul/earth/mod.rs:14-26` (module 声明)

- [ ] **Step 1: 在 EarthToolContext 添加 WorldStateStore**

```rust
// executor.rs:23-28
pub struct EarthToolContext {
    pub skill_cache: HashMap<String, String>,
    pub memory_manager: Option<MemoryManager>,
    pub relationship_store: Option<RelationshipStore>,
    pub recipe_details: Vec<RecipeDetail>,
    pub world_state_store: Option<Arc<WorldStateStore>>,  // 新增
    pub action_cache: Option<Vec<ActionDescription>>,       // 新增
}
```

- [ ] **Step 2: 实现 state_tool.rs**

5 个工具:
1. `get_action_detail(action_type: String)` - 从 action_cache 获取完整 action 描述 + field hints
2. `query_inventory()` - 从 WorldStateStore.current() 获取 self_state.inventory
3. `nearby_entities(id: Option<String>)` - 从 WorldStateStore.current() 获取 entities
4. `query_environment()` - 从 WorldStateStore.current() 获取 location + gatherable_items + events_log
5. `get_state_detail(attribute: Option<String>)` - 从 WorldStateStore.current() 获取 attributes + derived_attributes + attribute_descriptions

每个工具:
- 实现 `tool_definition()` 返回 JSON Schema
- 实现 `execute()` 从 WorldStateStore 读取并格式化

- [ ] **Step 3: 注册新工具到 executor**

在 `tool_definitions()` 中追加 5 个定义。
在 `execute()` 的 match 中追加 5 个路由。

- [ ] **Step 4: 在 mod.rs 追加声明**

```rust
mod state_tool;
```

- [ ] **Step 5: 写测试**

测试每个 tool 的 definition 格式和 execute 输出。

```bash
cargo nextest run -p cyber-jianghu-agent test_state_tools
```

- [ ] **Step 6: Commit**

```bash
git add crates/agent/src/soul/earth/state_tool.rs crates/agent/src/soul/earth/executor.rs crates/agent/src/soul/earth/mod.rs
git commit -m "feat(agent): add 5 EarthSoul tools for on-demand state queries"
```

### Task 10: 实现 Lean Prompt Assembler

**Files:**
- Modify: `crates/agent/src/soul/actor/engine_prompts.rs:19-376`

- [ ] **Step 1: 添加 build_focus_summary_section() 方法**

```rust
pub fn build_focus_summary_section(summary: &FocusSummary) -> String {
    let mut section = String::from("## 本轮焦点\n\n");
    for line in summary.narrative.lines() {
        section.push_str(line);
        section.push('\n');
    }
    section
}
```

- [ ] **Step 2: 添加 build_action_index() 方法**

替代 `build_action_descriptions()` + `build_action_field_hints()`:
```rust
pub fn build_action_index(actions: &[ActionDescription]) -> String {
    let mut section = String::from("## 可用动作\n(查询详情: get_action_detail(action_name))\n\n");
    for action in actions {
        section.push_str(&format!("- {}: {} (体力-{})\n",
            action.action_type, action.brief_description, action.stamina_cost));
    }
    section
}
```

- [ ] **Step 3: 添加 build_lean_direct_prompt() 方法**

当 `token_optimization.enabled == true` 时使用的 lean 版 prompt:
```
System Prompt (cached)
+ Persona Summary (cached)
+ Focus Summary (dynamic)
+ Action Index (names only)
// 不含: full WorldState, full action descriptions, full skill instructions
```

- [ ] **Step 4: 修改 engine.rs think_direct()**

在 `build_direct_prompt()` 调用前检查 `token_optimization.enabled`:
- `true` -> 调用 `build_lean_direct_prompt()`
- `false` -> 调用原有 `build_direct_prompt()` (向后兼容)

- [ ] **Step 5: 编译验证**

```bash
cargo build -p cyber-jianghu-agent
```

- [ ] **Step 6: Commit**

```bash
git add crates/agent/src/soul/actor/engine_prompts.rs crates/agent/src/soul/actor/engine.rs
git commit -m "feat(agent): add lean prompt assembler with action index and focus summary"
```

### Task 11: 实现 Critical 焦点预加载

**Files:**
- Modify: `crates/agent/src/soul/actor/engine.rs` (think_direct 中)

- [ ] **Step 1: 在 Focus Summary 后添加预加载逻辑**

当 Focus Summary 含 Critical 项且 `tool_preload.critical_preload == true`:
- 识别 Critical 项的 category
- 自动调用对应 tool 获取详情
- 将结果附带在 Focus Summary 后面

例如: `饥饿 Critical` -> 自动执行 `query_inventory()` 获取食物相关物品 -> 附带结果。

- [ ] **Step 2: Commit**

```bash
git add crates/agent/src/soul/actor/engine.rs
git commit -m "feat(agent): add critical focus tool preload"
```

---

## Chunk 5: Phase 4 — ReflectorSoul 重构

### Task 12: 重构 lifecycle.rs retry 循环

**Files:**
- Modify: `crates/agent/src/core/lifecycle.rs:1280-1563`

- [ ] **Step 1: 替换 retry 循环为固定流程**

将 `for attempt in 0..=max_retries` (line 1294-1563) 替换为:

```rust
// 读取 ReflectorOptConfig
let opt_config = self.config.game_rules
    .as_ref()
    .and_then(|g| g.token_optimization.as_ref())
    .map(|t| &t.reflector);

// 1. ActorSoul 生成 Intent (一次)
let intent = self.generate_intent(/* ... */).await?;

// 2. ReflectorSoul 验证 (一次)
let result = self.validate_with_reflector(intent, &world_state, graded_config).await?;

match result {
    Approved(intent) => self.submit_intent(intent).await,
    Rejected { reason, .. } => {
        if opt_config.self_correction {
            // 3. 同轮自修正 (一次)
            match self.self_correct(intent, reason, &conversation).await {
                Ok(corrected) => {
                    let revalidation = self.validate_with_reflector(corrected, &world_state, graded_config).await?;
                    match revalidation {
                        Approved(intent) => self.submit_intent(intent).await,
                        Rejected { .. } => self.chaos_fallback(world_state).await,
                    }
                }
                Err(_) => self.chaos_fallback(world_state).await,
            }
        } else {
            self.chaos_fallback(world_state).await
        }
    }
}
```

- [ ] **Step 2: 实现 self_correct() 方法**

```rust
async fn self_correct(
    &self,
    rejected_intent: Intent,
    reason: String,
    conversation: &mut Conversation,
) -> Result<Intent> {
    // 注入拒绝原因作为 assistant message
    conversation.add_tool_result("reflector", format!("验证未通过: {reason}。请修正。"));
    // LLM 在同一 context 中重新生成
    let corrected = self.llm_client.complete_json(conversation).await?;
    Ok(corrected)
}
```

- [ ] **Step 3: 实现 LLM 失败计数器**

使用 `AtomicU32` 跟踪当前 tick 内 LLM 调用失败次数。达到 `chaos_on_llm_fail` 阈值时直接 chaos_fallback。

- [ ] **Step 4: 编译验证**

```bash
cargo build -p cyber-jianghu-agent
```

- [ ] **Step 5: 运行全量测试**

```bash
cargo nextest run --workspace
```

- [ ] **Step 6: Commit**

```bash
git add crates/agent/src/core/lifecycle.rs
git commit -m "refactor(agent): replace ReflectorSoul retry loop with single-turn + self-correction"
```

### Task 13: 集成验证 + 配置开关测试

**Files:**
- Test: `crates/agent/tests/test_token_optimization_e2e.rs` (新建)

- [ ] **Step 1: 写集成测试**

测试场景:
- `token_optimization.enabled = false` -> 走原有全量 prompt 路径
- `token_optimization.enabled = true` -> 走 lean prompt 路径
- 首轮 (无 prev) -> first_tick_focus_cap 生效
- Delta Engine 正确检测变化
- Attention Controller 过滤和排序正确
- 新 EarthSoul tools 可被调用

- [ ] **Step 2: 运行测试**

```bash
cargo nextest run --workspace
```

- [ ] **Step 3: clippy 检查**

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 4: fmt 检查**

```bash
cargo fmt --check
```

- [ ] **Step 5: Commit**

```bash
git add crates/agent/tests/test_token_optimization_e2e.rs
git commit -m "test(agent): add token optimization integration tests"
```

---

## 实施注意事项

1. **每 Task 完成后运行 `cargo nextest run --workspace`** 确保无回归
2. **Phase 0 (Instrumentation) 先上线**，采集 24h 基线后再实施 Phase 1-4
3. **`token_optimization.enabled = false` 是安全回退**，任何阶段出问题可立即关闭
4. **Phase 2 的轻量 LLM 排序 (Phase 2) 暂不实现**，先用规则引擎截断。后续有数据后再加
5. **Phase 3 的 prompt 重构是最大风险点**，需要实测对比行为质量
