# Token 优化设计：注意力门控 + Tool-First 架构

**日期**: 2026-05-14
**状态**: Draft
**问题**: ~58M tokens/agent/24h (LongCat-Flash 级别模型), 成本过高
**目标**: 削减 60-70% token 消耗

## 1. 根因分析

当前架构将所有数据**全量嵌入 prompt**，LLM 每轮接收完整 WorldState、Action Descriptions、Memory、Skills 等内容。核心问题：

1. **全量 prompt 注入**: 每 tick 4-8K input tokens，大部分数据未发生变化
2. **ReflectorSoul 重试循环**: max_retries=12, 循环 `for attempt in 0..=12` (含首尾 = 13 轮), 每轮含 ActorSoul 生成 (~4-8K input) + ReflectorSoul Layer 3 (~0.8-1.5K input), 最坏 26 次 LLM 调用
3. **Action Descriptions 全量**: 30-50 个动作的完整描述始终嵌入 prompt (~2-4K tokens)
4. **Skill Instructions 全量**: 已掌握技能的行为指南始终嵌入 prompt
5. **无量化数据**: 缺乏 per-component token 消耗统计，优化无基线

Token 消耗估算 (per tick, 当前):

| 组件 | Input tokens | 占比 |
|------|-------------|------|
| WorldState 全量转写 | 1K-3K | ~30% |
| Action Descriptions + Field Hints | 2K-4K | ~40% |
| Memory Context | 500-2K | ~15% |
| Skill Instructions | 200-1.5K | ~10% |
| Persona + Summary | 300-600 | ~5% |
| ReflectorSoul Layer 3 (per call) | 800-1.5K | 额外 |

**最坏场景** (全部 13 轮重试, 每轮 ActorSoul + ReflectorSoul): ~62-123K input tokens/tick

## 2. 设计决策

| 维度 | 决策 |
|------|------|
| 架构方案 | 注意力门控 + Tool-First (方案 A) |
| 注意力模型 | 混合 (Delta + Focus) |
| Focus 生成 | 规则引擎 + 轻量 LLM |
| Server 端 | 全量推送不变, Agent 本地落存 |
| ReflectorSoul | 单轮 + 同轮自修正 |
| 拒绝策略 | 拒绝 2 次或 LLM 失败 2 次 → chaos_fallback |
| 实施顺序 | 先 Instrumentation → 再逐步优化 |

## 3. 整体架构

```
                    Server (不变)
                    全量 WorldState broadcast/tick
                           │ WebSocket
                           ▼
                    WorldState Store
                    - curr_state: WorldState
                    - prev_state: WorldState
                    零 LLM token 消耗, 纯内存
                           │
                    Delta Engine (纯规则, 零 token)
                    逐字段对比 prev vs curr
                    输出: Vec<StateChange>
                    分类: Critical/Important/Info
                           │
                    Attention Controller
                    Phase 1 (规则): 过滤生存威胁+社交目标+活跃任务
                    Phase 2 (轻量LLM): 候选集排序+Focus Summary
                    输出: FocusSummary (~200-500 tok, 含 tool hints)
                           │
                    Lean Prompt (~1.5-2.5K tokens)
                    System Prompt (cached)
                    + Persona Summary (cached)
                    + Focus Summary (dynamic, 含 tool hints)
                    + Action Index (names only)
                    + Tool Definitions
                           │
                    LLM Decision Cycle
                    1. 主 LLM 接收 Lean Prompt
                    2. 可选 tool calls 获取详情
                    3. 输出 Intent
                    4. ReflectorSoul (1 次)
                    5. 拒绝 -> 同轮自修正 (最多 1 次)
                    6. 二次拒绝 -> chaos_fallback
```

## 4. 组件详细设计

### 4.1 WorldState Store

**文件**: `crates/agent/src/component/state_store.rs` (新建)

**与现有存储的关系**: 当前 `HttpApiState.current_state: Arc<RwLock<Option<WorldState>>>` 存储 WorldState 用于 HTTP API。WorldStateStore 替代此职责, `HttpApiState` 改为持有 `Arc<WorldStateStore>` 引用。迁移后 `current_state` 字段移除, 所有 WorldState 访问统一走 WorldStateStore。

```rust
pub struct WorldStateStore {
    curr: Arc<RwLock<WorldState>>,
    prev: Arc<RwLock<Option<WorldState>>>,
}

impl WorldStateStore {
    pub fn update(&self, new_state: WorldState) { /* prev <- curr, curr <- new */ }
    pub fn current(&self) -> WorldState { /* read curr */ }
    pub fn previous(&self) -> Option<WorldState> { /* read prev */ }
    pub fn delta(&self) -> StateDelta { /* compute delta */ }
}
```

- 零 LLM token 消耗
- 纯内存操作
- 提供 WorldState 查询接口供 EarthSoul tools 和 HTTP API 使用
- 通过 `Arc<WorldStateStore>` 共享, 消除现有双 RwLock 重复

### 4.2 Delta Engine

**文件**: `crates/agent/src/component/delta_engine.rs` (新建)

```rust
pub struct StateChange {
    pub category: ChangeCategory,
    pub urgency: Urgency,
    pub field: String,
    pub description: String,
    pub data: serde_json::Value,
    pub tool_hint: Option<String>,  // "nearby_entities --id 张三"
}

pub enum ChangeCategory { Survival, Social, Environment, Inventory, Location }
pub enum Urgency { Critical, Important, Info }

pub struct StateDelta {
    pub changes: Vec<StateChange>,
    pub is_first_tick: bool,  // 首轮无 prev, 全量生成
}
```

**Delta 检测规则** (纯规则, 零 token):

| Category | 检测逻辑 | Urgency 判定 |
|----------|---------|-------------|
| Survival | `self_state.attributes` 中 `hp`/`hunger`/`thirst` 值变化超过阈值 | 超过危险线 (可配置) -> Critical; 超过 10% 变化 -> Important |
| Social | `entities` 列表变化 (新人 agent_id 出现/离开) | 出现/消失 -> Important; 仅 recent_actions 变化 -> Info |
| Environment | `events_log` 新增项 | 新事件 -> Important |
| Inventory | `self_state.inventory` 物品数量变化 | 减少关键物品 -> Important; 其他 -> Info |
| Location | `location.node_id` 变化 | 移动 -> Important |

**属性键名参考**: `WorldState.self_state.attributes: HashMap<String, i32>`, 键名如 `"hp"`, `"hunger"`, `"thirst"`, `"stamina"`。阈值在 `game_rules.yaml` 的 `token_optimization.delta.survival_thresholds` 中配置。

**特殊处理**: 首轮无 prev 时, 全量生成所有 category 的 StateChange, urgency 全部标记为 Important。首轮 `max_focus_items` 上限放宽为 `first_tick_focus_cap` (默认 15), 避免信息截断过猛。

**Tool Hint 生成**: 每个 StateChange 根据其 category 自动生成对应的 tool hint。格式为 `tool_name(arg1=val1)`:
- Survival + hunger Critical -> `nearby_items(type=food)`
- Social + new entity -> `nearby_entities(id={agent_id})`
- Inventory + item change -> `query_inventory()`
- Environment + event -> `query_environment()`

### 4.3 Attention Controller

**文件**: `crates/agent/src/component/attention.rs` (新建)

**两阶段架构**: 规则过滤 -> 轻量 LLM 排序

#### Phase 1: 规则过滤 (零 token)

```rust
pub struct AttentionFilter {
    pub survival_thresholds: HashMap<String, f32>,  // game_rules.yaml
    pub social_targets: Vec<Uuid>,                  // RelationshipStore
    pub active_tasks: Vec<Task>,                    // SummaryWindow
}

pub struct FocusConfig {
    pub max_focus_items: usize,       // 默认 5
    pub critical_auto_include: bool,  // Critical 自动纳入
    pub survival_thresholds: HashMap<String, f32>,
}
```

规则过滤逻辑:
1. 所有 `Urgency::Critical` -> 直接进入焦点
2. `Urgency::Important` + `Category::Survival` -> 进入焦点
3. `Category::Social` 且目标在 social_targets 中 -> 进入焦点
4. 与 active_tasks 相关的变化 -> 进入焦点
5. 其余 -> 进入候选集 (交给 Phase 2)

输出: `Vec<StateChange>` 分为 `auto_focus` (自动纳入) 和 `candidates` (需 Phase 2 判断)

#### Phase 2: 轻量 LLM 排序

仅在候选集非空且 auto_focus 数量 < max_focus_items 时调用。

- 模型: haiku 级轻量模型
- Input: ~200-400 tokens (候选集 JSON + 当前活动上下文)
- Output: ~50-100 tokens (选中的 indices)
- 每 tick 成本: ~300-500 tokens (可忽略)

Prompt 示例:
```
以下是本轮状态变化候选集。请选出最多 {remaining_slots} 项最需要角色关注的变化。
角色当前在 {location}, 正在 {current_activity}。
候选: {candidates_json}
输出格式: JSON array of indices
```

#### Focus Summary 生成

最终焦点项格式化为叙事摘要, 每条附带 tool hint:

```
[紧迫] 饥饿度升至72 (查询食物来源: nearby_items(type=food))
[变化] 铁匠铺附近出现商人张三 (查询详情: nearby_entities(id=张三))
[社交] 李四向你发起对话 (查询关系: get_relationship(target=李四))
[环境] 远处传来打斗声 (查询事件: query_environment())
```

### 4.4 Lean Prompt Assembler

**文件**: 修改 `crates/agent/src/soul/actor/engine.rs` + `engine_prompts.rs`

**新 Prompt 结构** (~1.5-2.5K tokens):

```
+-- System Prompt (cached, ~200 tok)
+-- Persona Summary (cached, ~100 tok)
+-- Focus Summary (dynamic, ~200-500 tok, 含 tool hints)
+-- Action Index (names only, ~200-300 tok)
+-- Tool Definitions (~650-1300 tok, 13 tools)
总计: ~1,150-2,200 input tokens (不含 tool definitions)
```

**Action Index 替代全量描述**:
```
可用动作:
- attack: 攻击附近目标 (体力-15)
- gather: 采集资源 (体力-10)
- craft: 制作物品 (体力-20)
... (约 30-50 个动作)
查询动作详情: get_action_detail(action_name)
```

**Critical 焦点预加载**: Focus Summary 中有 Critical 项时, 自动执行相关 tool 并将结果附带在 Focus Summary 后, 避免多一轮 tool call。

### 4.5 扩展 EarthSoul Tools

**文件**: `crates/agent/src/soul/earth/` (新增 tool 文件)

**新增工具** (从 prompt 嵌入迁移到 tool calling):

| Tool | 替代内容 | 预估节省 | 数据源 |
|------|---------|---------|--------|
| `get_action_detail(action_type)` | 全量 action descriptions + field hints | 2-4K tok/tick | `load_available_actions_from_file()` 返回的 action 列表 (prompt_cache.rs 缓存) |
| `query_inventory(filter?)` | prompt 中背包物品列表 | 200-500 tok | WorldStateStore |
| `nearby_entities(filter?)` | prompt 中附近实体详情 | 300-600 tok | WorldStateStore |
| `query_environment()` | prompt 中位置/采集/事件 | 200-400 tok | WorldStateStore |
| `get_state_detail(attribute?)` | prompt 中全量属性描述 | 100-300 tok | WorldStateStore |

**保留已有工具**: skill_view, search_memory, recall_archived, get/list_relationships, record_social_event, list_known_recipes, view_recipe_detail

**Tool 数据源**: 所有新 tool 从 `WorldStateStore` 读取数据, 不从 prompt 解析。

**Context Threading**: `EarthToolContext` 增加 `world_state_store: Arc<WorldStateStore>` 字段。`EarthToolExecutor` 构造时 (engine.rs:747-755) 传入此引用。新 tool 的 execute() 方法通过 `self.ctx.world_state_store.current()` 获取最新 WorldState 数据。现有 tool 不受影响 (它们的数据来自 skill_cache/memory_manager 等已有字段)。

### 4.6 ReflectorSoul 重构

**文件**: 修改 `crates/agent/src/core/reflector_ext.rs` + `lifecycle.rs`

**新流程**:

```
LLM 输出 Intent
       |
ReflectorSoul (1 次调用)
  Layer 1: action_type 确定性验证 (零 token)
  Layer 2: RuleEngine 验证 (零 token)
  Layer 3: LLM 验证 (~800-1.5K tokens, 仅在必要时)
       |
       +-- Approved -> 提交 Server
       |
       +-- Rejected
              |
              注入拒绝原因为 assistant message (复用已有 context)
              |
              LLM 自修正 (同轮追加 ~500-1K tokens)
              |
              +-- Approved -> 提交
              +-- Rejected -> chaos_fallback
              +-- LLM 失败 -> chaos_fallback
```

**失败策略**:
- 拒绝 2 次 (初验 + 自修正后) -> chaos_fallback
- LLM 调用失败累计 2 次 (任何步骤) -> chaos_fallback
- 不再存在 12 次重试循环

**配置外部化**:
```yaml
# game_rules.yaml
reflector:
  self_correction: true         # 启用同轮自修正
  chaos_on_double_reject: true  # 二次拒绝 -> chaos_fallback
  chaos_on_llm_fail: 2          # LLM 失败 2 次 -> chaos_fallback
```

**注意**: `max_retries` 不再用于此流程。lifecycle.rs 中的 retry 循环 (`for attempt in 0..=max_retries`) 将被替换为固定流程: generate -> validate -> (optional) self_correct -> submit/chaos_fallback。旧 `max_retries` 配置字段废弃。

**Graded Validation 保留**: Skip/Adaptive/Always 策略不变, 但:
- Skip: 完全跳过 ReflectorSoul -> 直接提交
- Adaptive: 规则过滤通过 -> 跳过 Layer 3 -> 直接提交
- Always: Layer 1 -> Layer 2 -> Layer 3 -> 单次验证

### 4.7 Instrumentation

**文件**: 扩展 `crates/agent/src/component/llm/token_tracking.rs` (已有)

**与现有系统的关系**: `token_tracking.rs` 已实现 `PerModelStats` 按 provider-model 维度追踪 prompt/completion tokens, 每 tick 持久化到磁盘。新增 `TokenMetrics` 扩展此系统, 在 per-model 统计基础上增加 per-component 维度 (cognitive_engine, attention_controller, reflector_layer3 等)。底层复用 `token_tracking.rs` 的数据收集机制, 新增 component 标签到每次 LLM 调用。

```rust
pub struct TokenMetrics {
    pub cognitive_engine: ComponentMetrics,
    pub attention_controller: ComponentMetrics,
    pub reflector_layer3: ComponentMetrics,
    pub tool_calling: ComponentMetrics,
    pub social_processing: ComponentMetrics,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub ticks_measured: u64,
}

pub struct ComponentMetrics {
    pub call_count: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
}
```

**计量采集点**:

| 位置 | 计量内容 |
|------|---------|
| CognitiveEngine::build_direct_prompt() | prompt 组装后的 input tokens |
| AttentionController::rank_candidates() | 轻量 LLM 的 input/output tokens |
| ReflectorSoul::validate_llm() | Layer 3 的 input/output tokens |
| DirectLlmClient::complete_json_with_tools() | 每轮 tool call 的 input/output tokens |
| SocialProcessor::evaluate() | 社交评估的 input/output tokens |

**输出**:
- 每 tick: log 总 token 消耗 + 各组件占比
- 每小时: 聚合统计写入 metrics.json
- API: GET /api/v1/metrics 返回当前会话 token 统计

**实施顺序**: Instrumentation 先上线, 采集 24h 基线数据, 然后逐步实施优化。

## 5. 预估效果

| 指标 | 当前 (正常) | 当前 (最坏) | 优化后 | 降幅 |
|------|-----------|------------|-------|------|
| Input tokens/tick | 4-8K | 62-123K | 1.5-2.5K | 60-70% (正常) |
| ReflectorSoul 调用/tick | 1-13 次 | 13 次 | 1-2 次 | 70-90% |
| Tool calling tokens/tick | 变动 | 变动 | 0-2K | 按需 |
| 总 tokens/tick | ~40K | ~130K+ | ~10-15K | 60-75% (正常) |
| **24h 总消耗** | **~58M** | **~187M+** | **~14-22M** | **60-75%** |

**注**: "当前 (最坏)" 列反映全部 13 轮重试场景 (每轮 ActorSoul + ReflectorSoul)。优化后最坏场景为 ActorSoul + ReflectorSoul + 1 次自修正 ≈ ~5-8K tokens, 相比当前最坏降幅 >95%。

## 6. 风险与缓解

| 风险 | 缓解 |
|------|------|
| LLM 因信息不足做次优决策 | Focus Summary + Critical 预加载确保关键信息不丢失 |
| Tool calling 轮次过多增加延迟 | max_tool_rounds=3 + budget 限制 |
| 轻量 LLM 排序质量不足 | Phase 1 规则引擎兜底, Phase 2 可降级为纯规则 |
| Delta 检测遗漏重要变化 | 首轮全量 + Critical 自动纳入 + 低阈值触发 |
| 迁移期间功能回归 | Instrumentation 先行, 逐步迁移, 每步 A/B 验证 |

## 7. 实施顺序

1. **Phase 0: Instrumentation** (1-2 天)
   - 实现 TokenMetrics
   - 在所有 LLM 调用点插入计量
   - 采集 24h 基线数据

2. **Phase 1: WorldState Store + Delta Engine** (2-3 天)
   - 实现 WorldStateStore
   - 实现 Delta Engine
   - 不改变 prompt 流程, 仅新增数据层

3. **Phase 2: Attention Controller** (2-3 天)
   - 实现 Phase 1 规则过滤
   - 实现 Phase 2 轻量 LLM 排序
   - Focus Summary 生成 (含 tool hints)

4. **Phase 3: Lean Prompt + Tool 化** (3-5 天)
   - 新增 EarthSoul tools
   - 重构 prompt assembler
   - Action Index 替代全量描述
   - A/B 对比验证

5. **Phase 4: ReflectorSoul 重构** (2-3 天)
   - 单轮验证 + 同轮自修正
   - 配置外部化
   - chaos_fallback 条件调整

## 8. 配置外部化

所有新增参数通过 `game_rules.yaml` 或 `agent.yaml` 配置:

```yaml
# game_rules.yaml 新增
token_optimization:
  enabled: true                      # 总开关, false 则回退全量 prompt 模式
  attention:
    max_focus_items: 5
    first_tick_focus_cap: 15          # 首轮焦点上限放宽
    critical_auto_include: true
    enable_llm_ranking: true
    llm_ranking_model: "haiku"        # 需支持中文输出
  delta:
    survival_thresholds:              # 用于 Delta Engine 和 Attention Controller 共享
      hunger: 0.7
      thirst: 0.7
      hp: 0.3
    change_percentage_threshold: 0.1
  tool_preload:
    enabled: true
    critical_preload: true
  reflector:
    self_correction: true
    chaos_on_double_reject: true
    chaos_on_llm_fail: 2
```

## 9. 向后兼容

- Server 端无任何改动
- Agent 的 WebSocket 通信协议不变
- 现有 tool calling 基础设施扩展, 不破坏已有 tools
- 所有优化通过配置开关控制, 可逐步启用
- 若优化导致行为退化, 可通过配置回退到全量 prompt 模式
