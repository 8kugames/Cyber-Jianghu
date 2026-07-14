# A 阶段：数据诚实化 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让数据在源头就诚实——8 个真闭集枚举化、哨兵值消除、重复类型合并、权威状态写入原子化、关系协议类型定义。

**Architecture:** 以 protocol crate 为唯一真相源，server/game_data 消费侧适配。枚举定义放跨进程契约所在的 crate（protocol），纯 server 内部的放 server。TDD 风格，每步独立 commit，每步编译+测试验证。

**Tech Stack:** Rust 2024 edition, sqlx 0.8 (PostgreSQL), serde, axum, tokio

**前置基线:** `pure` 分支 HEAD `32b7d006`（在途工作已整理为 4 commit），工作树干净，全编译全测试通过。

**Spec:** `docs/superpowers/specs/2026-07-14-data-honesty-stage-a-design.md`

---

## 文件结构总览

**protocol crate（枚举定义的主战场）：**
- `crates/protocol/src/types/entities.rs` — OocRisk 枚举；item_type/effect_type/requirement_type 的 Info struct 字段改枚举
- `crates/protocol/src/types/locations.rs` — LocationNodeData 合并到 LocationNode（node_type 已是枚举）
- `crates/protocol/src/messages.rs` — ConfigUpdate.config_type 改枚举
- `crates/protocol/src/sqlx_types.rs` — ItemType 枚举扩展到 6 变体（权威定义）
- `crates/protocol/src/types/relationship.rs`（新建）— 关系协议契约类型

**server crate（消费侧适配）：**
- `crates/server/src/game_data/types/actions.rs` — validation_type/operation/effect_type/requirement_type 枚举化
- `crates/server/src/game_data/types/unified_config.rs` — LocationNodeData 字段对齐
- `crates/server/src/game_data/types/items.rs` — ItemEffect.operation 枚举化
- `crates/server/src/game_data/cache.rs` — 删除手写 node_type 转换
- `crates/server/src/game_data/registry/action_registry.rs` — 桥接点插 From 转换
- `crates/server/src/actions/validator.rs` — validation_type/effect_type/requirement_type dispatch 改枚举穷尽匹配
- `crates/server/src/actions/executor/` — operation/effect_type/requirement_type dispatch 改枚举
- `crates/server/src/items/registry.rs` — 修复 Material/Tool 降级 bug
- `crates/server/src/inventory/manager.rs` — 修复 armor 幽灵变体
- `crates/server/src/models/items.rs` — ItemType 改为引用 protocol 定义
- `crates/server/src/websocket/types.rs` — ooc_risk dispatch 改枚举
- `crates/server/src/tick/processor/processor.rs` — upsert_agent_state 纳入 tx
- `crates/server/src/tick/realtime.rs` — 传 tx 给 processor

**agent crate（消费侧适配，改动少）：**
- `crates/agent/src/infra/transport/websocket.rs` — config_type dispatch 改枚举
- `crates/agent/src/runtime/claw/protocol.rs` — config_type dispatch 改枚举

---

## 第一组：真闭集枚举化（Item 1）

### Task 1: OocRisk 枚举化（protocol 层）

**Files:**
- Modify: `crates/protocol/src/types/entities.rs:277-291`
- Modify: `crates/server/src/game_data/types/actions.rs:78-79, 130-132`
- Modify: `crates/server/src/websocket/types.rs:76-87`
- Modify: `crates/server/src/game_data/registry/action_registry.rs:100`
- Test: `crates/protocol/src/types/entities.rs`（inline mod tests）

- [ ] **Step 1: 定义 OocRisk 枚举**

在 `crates/protocol/src/types/entities.rs` 顶部（import 区后）添加：

```rust
/// 动作的 OOC（Out-of-Character）风险等级，决定天魂分级审核策略
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OocRisk {
    /// 低风险，跳过审核
    Low,
    /// 中风险，自适应审核
    Medium,
    /// 高风险，必须审核
    High,
}

impl Default for OocRisk {
    fn default() -> Self {
        Self::Low
    }
}
```

- [ ] **Step 2: 改 AvailableAction.ooc_risk 字段类型**

`crates/protocol/src/types/entities.rs:277-278`，把：
```rust
#[serde(default = "default_ooc_risk")]
pub ooc_risk: String,
```
改为：
```rust
#[serde(default)]
pub ooc_risk: OocRisk,
```
删除 `default_ooc_risk()` 函数（entities.rs:289-291）。

- [ ] **Step 3: 改 server ActionConfig.ooc_risk 字段类型**

`crates/server/src/game_data/types/actions.rs:78-79`，把 `pub ooc_risk: String` 改为 `pub ooc_risk: OocRisk`（需 `use cyber_jianghu_protocol::types::OocRisk;`）。删除 actions.rs:130-132 的 `default_ooc_risk()`。

- [ ] **Step 4: 改 registry 桥接点**

`crates/server/src/game_data/registry/action_registry.rs:100`，`ooc_risk: config.ooc_risk.clone()` 不变（枚举支持 Clone）。确认编译。

- [ ] **Step 5: 改 websocket/types.rs dispatch**

`crates/server/src/websocket/types.rs:82`，把：
```rust
match action.ooc_risk.as_str() {
    "high" => always_types.push(action.action.clone()),
    "medium" => adaptive_types.push(action.action.clone()),
    _ => skip_types.push(action.action.clone()),
}
```
改为：
```rust
match action.ooc_risk {
    OocRisk::High => always_types.push(action.action.clone()),
    OocRisk::Medium => adaptive_types.push(action.action.clone()),
    OocRisk::Low => skip_types.push(action.action.clone()),
}
```
（需 `use cyber_jianghu_protocol::types::OocRisk;`）

- [ ] **Step 6: 编译验证 + 修复 fixture**

Run: `cargo check --workspace`
修复所有测试 fixture 里的 `ooc_risk: "low".to_string()` → `ooc_risk: OocRisk::Low`（grep 全仓 `ooc_risk` 定位）。
Expected: 编译通过。

- [ ] **Step 7: 写枚举序列化测试**

在 `crates/protocol/src/types/entities.rs` 的 `mod tests` 添加：
```rust
#[test]
fn test_ooc_risk_serde() {
    let json = serde_json::to_string(&OocRisk::High).unwrap();
    assert_eq!(json, "\"high\"");
    let parsed: OocRisk = serde_json::from_str("\"medium\"").unwrap();
    assert_eq!(parsed, OocRisk::Medium);
    // 低风险是默认值
    let parsed: OocRisk = serde_json::from_str("null").unwrap_or_default();
    assert_eq!(parsed, OocRisk::Low);
}
```

- [ ] **Step 8: 运行测试**

Run: `cargo test -p cyber-jianghu-protocol -- ooc_risk`
Expected: PASS

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "refactor(protocol): OocRisk 枚举化——消除 String 闭集"
```

---

### Task 2: node_type 枚举化（消除 region 吞没 bug）

LocationNodeType 枚举**已存在**于 protocol（`locations.rs:9-21`），LocationNode.node_type 已用枚举。问题在 `LocationNodeData`（game_data）仍用 String，cache.rs 有损 match。本 task 是 Item 3（重复类型合并）的一部分，但与 node_type 枚举化紧耦合，放这里一起做。

**Files:**
- Modify: `crates/server/src/game_data/types/unified_config.rs:599-600`
- Modify: `crates/server/src/game_data/cache.rs:223-227`

- [ ] **Step 1: 改 LocationNodeData.node_type 为枚举**

`crates/server/src/game_data/types/unified_config.rs:599-600`，把：
```rust
#[serde(rename = "type")]
pub node_type: String,
```
改为：
```rust
#[serde(rename = "type")]
pub node_type: LocationNodeType,
```
（需 `use cyber_jianghu_protocol::types::LocationNodeType;`）

- [ ] **Step 2: 删除 cache.rs 手写转换**

`crates/server/src/game_data/cache.rs:223-227`，把整个 match：
```rust
node_type: match node.node_type.as_str() {
    "map" => LocationNodeType::Map,
    "sub_scene" => LocationNodeType::SubScene,
    _ => LocationNodeType::Map,
},
```
改为：
```rust
node_type: node.node_type,
```

- [ ] **Step 3: 编译验证**

Run: `cargo check --workspace`
Expected: 编译通过（LocationNodeType 已有 `rename_all = "snake_case"`，YAML 的 `region`/`map`/`sub_scene` 可直接反序列化）。

- [ ] **Step 4: 写回归测试——region 不再被吞**

在 `crates/server/src/game_data/cache.rs` 的测试模块（或新建）添加：
```rust
#[test]
fn test_location_node_type_region_not_swallowed() {
    // locations.yaml 的 region 节点不应被降级为 Map
    let yaml_data = LocationNodeData {
        node_id: "test_region".into(),
        name: "测试区域".into(),
        node_type: LocationNodeType::Region,
        parent_id: None,
        description: None,
        environmental_damage: None,
        gatherable_items: None,
    };
    // 转换后应保留 Region
    assert_eq!(yaml_data.node_type, LocationNodeType::Region);
}
```

- [ ] **Step 5: 运行测试**

Run: `cargo test -p cyber-jianghu-server -- location_node_type`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "fix(game_data): LocationNodeData.node_type 枚举化——修复 region 被吞成 Map 的 bug"
```

---

### Task 3: validation_type 枚举化（server 内部，含 ItemExists 新增项）

**Files:**
- Modify: `crates/server/src/game_data/types/actions.rs:215-234`
- Modify: `crates/server/src/actions/validator.rs:192-293`

- [ ] **Step 1: 定义 ValidationType 枚举**

在 `crates/server/src/game_data/types/actions.rs` 的 FieldValidation 定义前添加：
```rust
/// 字段校验类型（闭集，代码 dispatch 决定）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationType {
    NotEmpty,
    MinValue,
    MaxValue,
    MinLength,
    MaxLength,
    ItemExists,
}
```

- [ ] **Step 2: 改 FieldValidation.validation_type 字段**

`crates/server/src/game_data/types/actions.rs:220`，把 `pub validation_type: String` 改为 `pub validation_type: ValidationType`。删除 TYPE_* 常量（lines 227-234 的整个 impl 块）。

- [ ] **Step 3: 改 validator.rs dispatch 为穷尽匹配**

`crates/server/src/actions/validator.rs:192-293`，把：
```rust
match fv.validation_type.as_str() {
    FieldValidation::TYPE_NOT_EMPTY => { ... }
    FieldValidation::TYPE_MIN_VALUE => { ... }
    ...
    _ => {}
}
```
改为：
```rust
match &fv.validation_type {
    ValidationType::NotEmpty => { ... }
    ValidationType::MinValue => { ... }
    ValidationType::MaxValue => { ... }
    ValidationType::MinLength => { ... }
    ValidationType::MaxLength => { ... }
    ValidationType::ItemExists => { ... }
}
```
（去掉 `_ => {}` 兜底，让穷尽匹配强制覆盖所有变体）

- [ ] **Step 4: 编译验证**

Run: `cargo check --workspace`
Expected: 编译通过（actions.yaml 里已配 `item_exists` 等值，serde rename_all="snake_case" 可匹配）

- [ ] **Step 5: 写穷尽匹配回归测试**

在 `crates/server/src/actions/validator.rs` 测试模块添加测试，验证每种 ValidationType 都被处理（构造一个 FieldValidation 跑 apply_field_validations，确认无静默跳过）。

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(server): ValidationType 枚举化——穷尽匹配消除静默跳过"
```

---

### Task 4: operation 枚举化（ItemEffect，先统一 3 个 struct 字段名）

**注意：** 有 3 个同名 ItemEffect struct（game_data/items.rs `operation`、models/items.rs `operation`、actions/types.rs `operator`）。先统一字段名为 `operation` + 枚举。

**Files:**
- Modify: `crates/server/src/game_data/types/items.rs:77`
- Modify: `crates/server/src/models/items.rs:51-61`
- Modify: `crates/server/src/actions/types.rs:48-53`
- Modify: `crates/server/src/actions/executor/basic.rs:227-238`
- Modify: `crates/server/src/tick/processor/executor.rs:127-157`

- [ ] **Step 1: 定义 Operation 枚举**

在 `crates/server/src/game_data/types/items.rs` 添加：
```rust
/// 属性修改操作类型（数学语义闭集）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Operation {
    /// 加法：current + value
    Add,
    /// 设定：current = value
    Set,
    /// 乘法：current * value
    Multiply,
}
```

- [ ] **Step 2: 统一 3 个 struct 的字段名 + 类型**

- `game_data/types/items.rs:77`：`pub operation: String` → `pub operation: Operation`
- `models/items.rs:57`：`pub operation: String` → `pub operation: Operation`
- `actions/types.rs:51`：`pub operator: String` → 改名为 `pub operation: Operation`

- [ ] **Step 3: 改 executor/basic.rs（game_data→actions 转换）**

`crates/server/src/actions/executor/basic.rs:227-238`，把 if 链过滤改为直接传递（枚举已约束）：
```rust
// operation 已是 Operation 枚举，无需过滤非法值
ItemEffect {
    attribute: effect.attribute.clone(),
    operation: effect.operation,  // 直接传递
    value: effect.value,
}
```

- [ ] **Step 4: 改 processor/executor.rs dispatch**

`crates/server/src/tick/processor/executor.rs:127-157`，把：
```rust
let value_to_apply = match effect.operator.as_str() {
    "set" => { ... }
    "multiply" => { ... }
    _ => effect.value,
};
```
改为（注意字段名 operator→operation）：
```rust
let value_to_apply = match effect.operation {
    Operation::Set => { /* effect.value - current_value */ }
    Operation::Multiply => { /* current_value * effect.value - current_value */ }
    Operation::Add => effect.value,
};
```

- [ ] **Step 5: 编译验证 + 修复 fixture**

Run: `cargo check --workspace`
修复所有 `operation: "add".to_string()` / `operator: "set".to_string()` 等 fixture。

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(server): Operation 枚举化——统一 3 个 ItemEffect struct 字段名"
```

---

### Task 5: item_type 枚举统一（修复 Material/Tool 降级 + armor 幽灵变体）

**变体集合决策（代码事实决定）：** 6 变体 Consumable/Weapon/Currency/Material/Tool/Armor——覆盖 YAML 实际值 + agent dispatch 实际认的值。以 protocol/sqlx_types.rs 为权威定义。

**Files:**
- Modify: `crates/protocol/src/sqlx_types.rs:110-146`
- Modify: `crates/server/src/models/items.rs:12-46`
- Modify: `crates/server/src/items/registry.rs:57-62`
- Modify: `crates/server/src/inventory/manager.rs:347`

- [ ] **Step 1: 扩展 protocol ItemType 到 6 变体**

`crates/protocol/src/sqlx_types.rs:110-146`，把：
```rust
pub enum ItemType {
    Consumable,
    Weapon,
    Currency,
}
```
改为：
```rust
pub enum ItemType {
    Consumable,
    Weapon,
    Currency,
    Material,
    Tool,
    Armor,
}
```
更新 Display impl（sqlx_types.rs:125-133）和 FromStr impl（sqlx_types.rs:135-146）补上 Material/Tool/Armor 分支。

- [ ] **Step 2: 删除 server models/items.rs 的 ItemType 重复定义**

`crates/server/src/models/items.rs:12-46`，删除整个 `pub enum ItemType` + `parse()` 方法。改为 re-export：
```rust
pub use cyber_jianghu_protocol::sqlx_types::ItemType;
```

- [ ] **Step 3: 修复 registry.rs Material/Tool 降级 bug**

`crates/server/src/items/registry.rs:57-62`，把 3 分支 match + 默认 Consumable：
```rust
let item_type = match item.item_type.as_str() {
    "consumable" => ItemType::Consumable,
    "currency" => ItemType::Currency,
    "weapon" => ItemType::Weapon,
    _ => ItemType::Consumable,
};
```
改为直接用 serde 反序列化（ItemConfigEntry.item_type 改为 ItemType 枚举），或用 FromStr：
```rust
let item_type: ItemType = item.item_type.parse().unwrap_or(ItemType::Consumable);
```
（FromStr 在 Step 1 已补全所有变体，不再吞 Material/Tool）

- [ ] **Step 4: 修复 manager.rs armor 幽灵变体**

`crates/server/src/inventory/manager.rs:347`，把：
```rust
if matches!(config.item_type.as_str(), "weapon" | "armor") {
```
改为（config.item_type 现在是 ItemType 枚举）：
```rust
if matches!(config.item_type, ItemType::Weapon | ItemType::Armor) {
```

- [ ] **Step 5: 编译验证**

Run: `cargo check --workspace`
Expected: 编译通过

- [ ] **Step 6: 写回归测试——Material 不再降级**

```rust
#[test]
fn test_material_not_downgraded_to_consumable() {
    let item_type: ItemType = "material".parse().unwrap();
    assert_eq!(item_type, ItemType::Material);
}
```

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "fix(protocol): ItemType 统一为 6 变体——修复 Material/Tool 降级 + armor 幽灵变体"
```

---

### Task 6: config_type 枚举化（跨进程契约，7 值）

**Files:**
- Modify: `crates/protocol/src/messages.rs:162-180`
- Modify: `crates/agent/src/infra/transport/websocket.rs:804-934`
- Modify: `crates/agent/src/runtime/claw/protocol.rs:406-438`
- Modify: server 端所有构造 ConfigUpdate 的地方（handler.rs/main.rs/realtime.rs/scheduler.rs 等）

- [ ] **Step 1: 定义 ConfigType 枚举**

在 `crates/protocol/src/messages.rs` 的 ServerMessage 定义前添加：
```rust
/// 配置更新类型（7 值闭集，对应 agent 端 dispatch 分支）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigType {
    Skills,
    Actions,
    GameRules,
    WorldBuildingRules,
    PromptTemplates,
    PersonaEventRules,
    NarrativeConfig,
}
```

- [ ] **Step 2: 改 ConfigUpdate.config_type 字段**

`crates/protocol/src/messages.rs:164`，把 `config_type: String` 改为 `config_type: ConfigType`。

- [ ] **Step 3: 改 server 端所有构造点**

grep 全 server crate `ConfigUpdate` 和 `config_type`，把字面量 `"skills".to_string()` 等改为 `ConfigType::Skills` 等。位置：handler.rs:365/402/439/494、main.rs:355、realtime.rs:293、scheduler.rs:238/303/365/478/551/633、action_evolution.rs:409。

- [ ] **Step 4: 改 agent websocket.rs dispatch**

`crates/agent/src/infra/transport/websocket.rs:804-934`，把 7 个 if-else 链改为 match：
```rust
match &config_type {
    ConfigType::Skills => { ... }
    ConfigType::Actions => { ... }
    ConfigType::GameRules => { ... }
    ConfigType::WorldBuildingRules => { ... }
    ConfigType::PromptTemplates => { ... }
    ConfigType::PersonaEventRules => { ... }
    ConfigType::NarrativeConfig => { ... }
}
```

- [ ] **Step 5: 改 agent claw/protocol.rs dispatch**

`crates/agent/src/runtime/claw/protocol.rs:406-438`，同样改 match。

- [ ] **Step 6: 编译验证 + 修复 fixture**

Run: `cargo check --workspace`

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "refactor(protocol): ConfigType 枚举化——7 值闭集穷尽匹配"
```

---

### Task 7: effect_type 枚举化（ActionEffect，消除 teleport/remove_item 死分支）

**Files:**
- Modify: `crates/protocol/src/types/entities.rs:319-330`（ActionEffectInfo）
- Modify: `crates/server/src/game_data/types/actions.rs:310-330, 387-394`
- Modify: `crates/server/src/actions/executor/mod.rs:135-174`
- Modify: `crates/server/src/game_data/registry/action_registry.rs:121`

- [ ] **Step 1: 定义 EffectType 枚举（protocol）**

在 `crates/protocol/src/types/entities.rs` 添加：
```rust
/// 动作效果类型（5 值闭集）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectType {
    AttributeChange,
    AttributeMaxChange,
    AddItem,
    RemoveItem,
    Teleport,
}
```

- [ ] **Step 2: 改 ActionEffectInfo.effect_type（protocol）**

`crates/protocol/src/types/entities.rs:322`：`pub effect_type: String` → `pub effect_type: EffectType`

- [ ] **Step 3: 改 server ActionEffect.effect_type + 删常量**

`crates/server/src/game_data/types/actions.rs:315`：`pub effect_type: String` → `pub effect_type: EffectType`。删除 EFFECT_TYPE_* 常量（lines 387-394）。

- [ ] **Step 4: 改 executor/mod.rs dispatch——暴露死分支**

`crates/server/src/actions/executor/mod.rs:135-174`，改为穷尽匹配。**关键**：teleport 和 remove_item 当前是死分支（`_=>{}` 吞掉），改为显式分支后需决定实现或标注 `todo!()`/注释说明未实现：
```rust
match &effect.effect_type {
    EffectType::AttributeChange => { /* 现有逻辑 */ }
    EffectType::AddItem => { /* 现有逻辑 */ }
    EffectType::AttributeMaxChange => { /* 现有逻辑 */ }
    EffectType::RemoveItem => {
        // 已定义但未实现——枚举化后编译器强制面对
        tracing::warn!("effect_type remove_item 已定义但执行器未实现，跳过");
    }
    EffectType::Teleport => {
        tracing::warn!("effect_type teleport 已定义但执行器未实现，跳过");
    }
}
```

- [ ] **Step 5: 改 registry 桥接点**

`crates/server/src/game_data/registry/action_registry.rs:121`：`effect_type: e.effect_type.clone()` 不变（枚举 Clone）。

- [ ] **Step 6: 编译验证 + Commit**

Run: `cargo check --workspace`
```bash
git add -A
git commit -m "refactor(protocol): EffectType 枚举化——暴露 teleport/remove_item 死分支"
```

---

### Task 8: requirement_type 枚举化（ActionRequirement）

**Files:**
- Modify: `crates/protocol/src/types/entities.rs:301-313`
- Modify: `crates/server/src/game_data/types/actions.rs:269-289, 337-342`
- Modify: `crates/server/src/actions/executor/mod.rs:103-120`
- Modify: `crates/server/src/actions/validator.rs:133-160`

- [ ] **Step 1: 定义 RequirementType 枚举（protocol）**

```rust
/// 动作前置条件类型（4 值闭集）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequirementType {
    Attribute,
    Item,
    Location,
    Skill,
}
```

- [ ] **Step 2: 改 ActionRequirementInfo + ActionRequirement 字段 + 删常量**

protocol entities.rs:304 和 server actions.rs:274：`String` → `RequirementType`。删除 REQUIREMENT_TYPE_* 常量（actions.rs:337-342）。

- [ ] **Step 3: 改 executor/mod.rs + validator.rs dispatch**

两处 match 改穷尽匹配，location/skill 的空实现改为 warn 日志（显式标注未实现）。

- [ ] **Step 4: 编译验证 + Commit**

```bash
git add -A
git commit -m "refactor(protocol): RequirementType 枚举化——暴露 location/skill 死分支"
```

---

## 第二组：哨兵值 + 时间戳（Item 2）

### Task 9: parent_id 哨兵→Option + 时间戳统一

**注意：** Attribute.metadata 的 `#[serde(skip)]` 经查证是**合理传输设计**（Attribute 不进 WS 传输，传输用 HashMap<String,i32>），剔除出范围。

**Files:**
- Modify: `crates/server/src/game_data/types/unified_config.rs:602`
- Modify: `crates/server/src/game_data/cache.rs:228-232`
- Modify: `crates/protocol/src/messages.rs:419`（TraceEntry.wall_clock）

- [ ] **Step 1: 改 LocationNodeData.parent_id 为 Option**

`crates/server/src/game_data/types/unified_config.rs:602`：`pub parent_id: String` → `pub parent_id: Option<String>`

- [ ] **Step 2: 删除 cache.rs 哨兵判定**

`crates/server/src/game_data/cache.rs:228-232`，把 `is_empty()` 判定改为直传：
```rust
parent_id: node.parent_id.clone(),  // 已是 Option，无需哨兵转换
```

- [ ] **Step 3: 统一权威路径时间戳（protocol 层）**

`crates/protocol/src/messages.rs:419`：`TraceEntry.wall_clock: Option<String>` → `Option<i64>`（Unix 毫秒，与 Pong.timestamp:183、AgentDied.died_at:220 对齐）。同步改 agent 端填充 wall_clock 的代码（grep `wall_clock`）。

- [ ] **Step 4: 编译验证 + Commit**

```bash
git add -A
git commit -m "refactor: parent_id 哨兵消除 + wall_clock 时间戳统一为 i64 毫秒"
```

---

## 第三组：重复类型合并（Item 3，ItemType 已在 Task 5 处理）

### Task 10: LocationNode/LocationNodeData 合并

**注意：** node_type 已在 Task 2 枚举化，parent_id 已在 Task 9 改 Option。剩余字段差异：gatherable_items（Vec vs Option）、description（仅 game_data）、implicit_travel_cost（仅 protocol）。

**Files:**
- Modify: `crates/server/src/game_data/types/unified_config.rs:595-608`
- Modify: `crates/server/src/game_data/cache.rs:215-251`

- [ ] **Step 1: 评估字段差异的合并策略**

LocationNodeData 独有 `description`；LocationNode 独有 `implicit_travel_cost`。决策：LocationNodeData 加 `implicit_travel_cost: Option<u32>`（当前 cache 硬编码 None，说明该字段未被配置驱动——保持 None 或移除该字段）。description 加入 LocationNode（协议层也需要描述）。

- [ ] **Step 2: 对齐字段 + 简化 cache 转换**

根据 Step 1 决策对齐两个 struct 字段，cache.rs 转换从逐字段 match 改为直接结构体转换或 `From` impl。

- [ ] **Step 3: 编译验证 + Commit**

```bash
git add -A
git commit -m "refactor: LocationNode/LocationNodeData 合并——消除手写转换"
```

---

### Task 11: 其余重复类型合并（NarrativeThreshold/ActionRequirement/ActionEffect/AttributeDrive）

这些类型在 protocol 和 game_data 各有一份近全同定义。合并策略：game_data 侧改为 `pub use protocol::...` 或定义 `From`。

**Files:**
- Modify: `crates/server/src/game_data/types/unified_config.rs`（ThresholdData/AttributeDriveData）
- Modify: `crates/server/src/game_data/types/actions.rs`（ActionRequirement/ActionEffect——已在 Task 7/8 枚举化，检查是否已对齐）

- [ ] **Step 1: 逐组对比字段，确认全同后删除 game_data 侧重复**

- [ ] **Step 2: 编译验证 + Commit**

```bash
git add -A
git commit -m "refactor: 合并 protocol↔game_data 重复类型——protocol 为唯一真相源"
```

---

## 第四组：权威状态写入原子化（Item 4）

### Task 12: 审计 tx 外旁路 + upsert_agent_state 纳入 tx

**Files:**
- Modify: `crates/server/src/tick/processor/processor.rs:58-283`
- Modify: `crates/server/src/tick/realtime.rs:247-263, 463-480`

- [ ] **Step 1: 审计 process_single_intent 全路径 tx 外直写**

已查证的 3 处旁路：
- `processor.rs:72` update_agent_online（best-effort，可保留）
- `processor.rs:343` record_recipe_observation（配方观察计数）
- `processor.rs:361` 配方习得 INSERT

决策：update_agent_online 保留 tx 外（best-effort 语义正确）；配方观察/习得纳入 tx（与 action_log 同生命周期）。

- [ ] **Step 2: 写回归测试——原子性（参照 P0-2 测试 processor.rs:438-476）**

测试场景：processor tx commit 成功后、upsert_agent_state 前模拟失败，验证 agent_states 与 inventory 不会出现不一致。

- [ ] **Step 3: 把 upsert_agent_state 纳入 processor tx**

改 `processor.rs` 的 `process_single_intent` 签名，接收 `&mut tx` 或返回需持久化的 state 让 realtime 在同一 tx 内 upsert。改 `realtime.rs:247-263` 传 tx 而非 pool。

- [ ] **Step 4: 编译 + 运行回归测试**

Run: `cargo test -p cyber-jianghu-server -- atomicity`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "fix(processor): upsert_agent_state 纳入 tx——消除跨表部分提交窗口"
```

---

## 第五组：关系协议类型定义（Item 5）

### Task 13: 在 protocol 定义关系契约类型

**Files:**
- Create: `crates/protocol/src/types/relationship.rs`
- Modify: `crates/protocol/src/types.rs`（mod 注册）

- [ ] **Step 1: 创建 relationship.rs，复制 agent 的关系类型**

从 `crates/agent/src/component/social/relationship_types.rs` 复制 KeyEvent（:48-60）和 RelationshipMemory（:83-101）到 protocol，字段集严格对齐。时间戳用 i64 毫秒（与 Item 2 时间戳统一对齐，不引入 DateTime<Utc> 到 protocol）。

- [ ] **Step 2: 在 types.rs 注册模块**

`crates/protocol/src/types.rs` 添加 `pub mod relationship;` 和 re-export。

- [ ] **Step 3: 编译验证 + Commit**

Run: `cargo check --workspace`
```bash
git add -A
git commit -m "feat(protocol): 定义关系契约类型——为 C 阶段关系存储预留契约"
```

---

## 收尾

### Task 14: 全量验证

- [ ] **Step 1: 全量编译**

Run: `cargo check --workspace`
Expected: 0 errors

- [ ] **Step 2: 全量测试**

Run: `cargo test --workspace`
Expected: 全绿

- [ ] **Step 3: 更新 CLAUDE.md（如有 API/类型变更需文档同步）**

- [ ] **Step 4: 最终 commit**

```bash
git add -A
git commit -m "docs: A 阶段数据诚实化完成"
```
