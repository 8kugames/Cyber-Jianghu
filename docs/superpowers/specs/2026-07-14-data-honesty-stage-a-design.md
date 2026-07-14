# A 阶段：数据诚实化设计

> 日期：2026-07-14
> 分支：`pure`
> 状态：3-agent 冗余审查表决通过（3/3 批准，附强制修订已吸收）
> 前置基线：`32b7d006`（在途工作 4 commit 已整理）

## 1. 使命与定义

**使命**：让数据在源头就诚实，为后续 B 阶段（不丢）、C 阶段（可达）打地基。

**"正确性"的三个正交维度**（五件事一一对应，无交叉）：

| 维度 | 含义 | 对应事项 |
|------|------|----------|
| 值域诚实 | 值域被类型约束，非法值编译期不可表达 | item 1（枚举化） |
| 跨表自洽 | 一次状态变更在 DB 里不互相矛盾 | item 4（原子化） |
| 可往返 | 序列化无损，反序列化复原 | item 2（serde + Option） |

item 3（重复类型合并）消除"诚实性的 bug 工厂"——手写转换是正确性的结构性敌人。
item 5（关系协议）为 C 阶段正确性预留契约，不污染 A。

## 2. 物理约束（不可变事实，已代码验证）

1. **PostgreSQL 是唯一权威存储**，DashMap 是写穿缓存：DB upsert `.await` 成功后才更新 DashMap（`tick/realtime.rs:248→268`）。
2. **单消费者 FIFO**（`realtime.rs:116`），无并发竞态。
3. **YAML 数据驱动是根基**：`cause`/`category`/`state`/`weather` 是配置可扩展字段，代码不 match 固定值。
4. **Rust 编译器是最强正确性防线**。
5. **无向后兼容包袱**（CLAUDE.md 规则 7），可随意 breaking。

## 3. 已纠偏的误诊（本次设计明确排除）

| 误诊 | 证伪 | 处置 |
|------|------|------|
| 消息流丢 ExecutionResult/WorldState = 数据完整性问题 | 权威状态写穿且每 tick flush，推送丢失是咨询性通道抖动，agent 重连拉一次即自洽 | 归 B，不进 A |
| 对话历史 WS-only 不可查 = 最大缺口 | 发言内容已落 `agent_action_logs.action_data` JSONB，`/api/dashboard/experiences` 原样返回 | 归 C（仅需聚合视图），不进 A |
| 15+ String 字段都该枚举化 | 逐个查 dispatch 后只有 8 个真闭集 | 只枚举化真闭集，数据驱动字段保持 String |

## 4. 五件事

### Item 1：真闭集枚举化（8 个）+ 修复已炸 bug

**判据**：值域是否被代码逻辑写死 match/dispatch（加新值必须改代码 = 真闭集）。锚定在 dispatch 代码，不看注释自述。

**应枚举化的 8 个真闭集**：

| 字段 | dispatch 证据 | 当前问题 |
|------|--------------|----------|
| `ooc_risk` | `websocket/types.rs:82` 3 分支 match → GradedValidationConfig 三桶 | String，无约束 |
| `node_type` | `game_data/cache.rs:223` 3 分支 match | **已炸 bug**：region 被吞成 Map |
| `config_type` | `agent/infra/transport/websocket.rs:822-934` 7 分支 if-else + `runtime/claw/protocol.rs:411` match | String |
| `validation_type` | `actions/validator.rs:192` 6 分支 match（TYPE_* 常量） | String + `_=>{}` 静默吞 |
| `operation`(ItemEffect) | `executor/basic.rs:227` + `processor/executor.rs:127` | String |
| `item_type` | 枚举已存在 `models/items.rs:17`（5 变体）但多处漂移 | **已炸 bug**：registry.rs:57 把 Material/Tool 降级成 Consumable。5 变体（Consumable/Weapon/Currency/Material/Tool）均由 items.yaml 配置支撑。"armor" 是 manager.rs:347 的永不命中死代码，items.yaml 零配置，**删除不立法** |
| `effect_type`(ActionEffect) | `executor/mod.rs:135` | 3 变体（attribute_change/attribute_max_change/add_item）均由 actions.yaml 配置支撑。remove_item/teleport 是 `#[allow(dead_code)]` 预留常量，**0 配置 0 实现 0 调用，删除不枚举化** |
| `requirement_type`(ActionRequirement) | `validator.rs:133` | 2 变体（attribute/item）有配置或实现支撑。location/skill 是 `#[allow(dead_code)]` 预留常量，**0 配置 0 实现 0 调用，删除不枚举化** |

**保持 String 的数据驱动字段**（枚举化是破坏根基的伪需求）：

| 字段 | 理由 |
|------|------|
| `cause`（死因） | `cache.rs:131` clone 自 YAML；`cause_advice_map: HashMap` 查找 |
| `category`（动作分类） | `processor.rs:121` HashMap key；`trigger_categories: Vec<String>` 可扩展 |
| `state`（Entity 显示串） | `broadcaster.rs:360` 值来自 display_messages.yaml；逻辑用 `is_alive: bool` |
| `weather` | `time_registry.rs:134` 4 快速分支 + `weather_events` HashMap 扩展口；key 来自 weather_pool YAML |

**注意**：`weather`/`category` 不是"零 match"，是"match 但带可扩展兜底"。枚举化它们的危害是破坏配置层已设计的扩展点。

**已炸 bug 修复清单**（枚举化时顺带修复）：
1. `items/registry.rs:57`：Material/Tool 降级成 Consumable（真 bug，items.yaml 有配置）
2. `game_data/cache.rs:223`：region 节点吞成 Map（真 bug，locations.yaml 有配置）

**臆造能力删除清单**（`#[allow(dead_code)]` 预留常量，0 配置 0 实现 0 调用，删除而非枚举化）：
3. `inventory/manager.rs:347` + agent `engine_prompts.rs:371`：删除 "armor" 幽灵匹配（items.yaml 零 armor 物品；agent 认 armor 只是 prompt 兜底过度泛化）
4. `actions.rs:393-394`：删除 EFFECT_TYPE_REMOVE_ITEM/EFFECT_TYPE_TELEPORT 常量（actions.yaml 零配置）
5. `actions.rs:341-342`：删除 REQUIREMENT_TYPE_LOCATION/REQUIREMENT_TYPE_SKILL 常量（actions.yaml 零配置）

> **反对虚假实现**：绝不用 `tracing::warn!("未实现，跳过")` 给臆造分支盖实现戳。臆造的删除，真缺失的实现。

### Item 2：哨兵值→Option + serde 可往返 + 权威路径时间戳统一

**哨兵值→Option**（仅真实哨兵，剔除臆测症状）：

| 字段 | 当前 | 目标 | 验证状态 |
|------|------|------|----------|
| `parent_id`（LocationNodeData） | 空串表"无父节点"（`cache.rs:228` `is_empty()`） | `Option<String>` | ✅ 真实（locations.yaml:31 确用空串） |

**剔除的臆测症状**（三 agent 审查发现，不进 A）：
- ~~`rebirth_delay=-1` 表"不可重生"~~：全仓库不存在，字段实为 `rebirth_delay_ticks: i32`，生产配置 `delay_ticks: 5`
- ~~`max_durability=-1` 表"无限"~~：仅存在于测试夹具，生产 YAML 无此键

**serde 可往返**：经查证，权威路径无 serde 可往返缺陷。

**剔除的误判**（审查查证后确认非缺陷）：
- ~~`Attribute.metadata` 的 `#[serde(skip)]`~~：`Attribute` 结构体从不进 WS/DB 序列化路径（传输用 `HashMap<String,i32>`，`entities.rs:47`；DB 用 `get_attributes_for_protocol()` 拍平为 i32）。agent 端零 import `protocol::Attribute`。skip 是冗余但正确的防御标注，不丢数据。

**剔除的误判**：
- ~~`SkillDefinition.content` 的 skip~~：是 SKILL.md 文件加载结构，不进 WS 传输路径（传输用 `SkillContent`）。配置加载型 skip，合理。

**时间戳统一**（仅 server+protocol 权威路径）：
- 当前混用：`DateTime<Utc>`（主流）、`i64` millis、`String`
- 统一范围：**仅 server+protocol 权威路径**
- agent crate 内部 DTO 的 `String` 时间戳归 C 阶段（API 边界），不进 A

### Item 3：重复类型合并（以 protocol 为唯一真相源）

**6 组重复类型 + ItemType 三胞胎**：

| protocol 侧 | game_data/server 侧 | 转换 bug |
|-------------|---------------------|----------|
| `LocationNode`（locations.rs:35） | `LocationNodeData`（unified_config.rs:596） | cache.rs:223 有损 match（region 吞没） |
| `NarrativeThreshold`（narrative.rs:30） | `ThresholdData`（unified_config.rs:738） | 字段全同，纯重复 |
| `ActionRequirement`（actions.rs:271） | `ActionRequirementInfo`（entities.rs:302） | 近全同 |
| `ActionEffect`（actions.rs:312） | `ActionEffectInfo`（entities.rs:320） | 近全同 |
| `AttributeDriveConfig`（narrative.rs:44） | `AttributeDriveData`（unified_config.rs:730） | 字段全同 |
| `ItemType`（protocol/sqlx_types.rs:114） | `ItemType`（server/models/items.rs:17） | 三胞胎（经 items/types 引用），registry.rs 漂移 bug 的根源 |

**目标**：protocol 为唯一真相源，game_data/server 改为 `pub use` 或 `From` 转换，消除手写 match 转换。

### Item 4：权威状态写入原子化

**问题**：`processor.rs:274` commit tx（inventory/ground_items/action_log）与 `realtime.rs:248` upsert `agent_states` 不在同一事务。两者之间崩溃 → DB 里 inventory 已改、agent_states 没改、DashMap 回退 = 跨表自相矛盾（药吃了 HP 没变）。

**归类论据**（判断甲，3/3 批准）：跨表矛盾是"数据互相撒谎"属正确性，不是"丢数据"属可靠性。最强论据：项目自己已在 `processor.rs:256` 把同类问题命名为"Saga 原子性"并写了 P0-2 修复+回归测试——真问题 2 是 P0-2 漏网的同族 bug。

**方案**（3/3 推荐）：把 `upsert_agent_state` **纳入 processor tx**（同一事务包住全部，崩溃即全回滚）。

> 不用"调换顺序先 upsert 主表再 commit 副表"——那仍是两步，调换后窗口方向变成"HP 变了但药没消耗"，矛盾依旧，只是换方向。

**验收门槛**（3/3 强制要求）：落地时必须先审计 `process_single_intent` 全路径所有 tx 外 `db_pool.execute` 旁路（如 `processor.rs:361` 存在旁路直写），纳入 tx 或显式论证无害。否则原子化是假的。

**范围限定**：tick 末尾 `persist_states`（realtime.rs:581）用批量 INSERT 重写 agent_states（state_version 重置为 0），该路径不走 CAS——在单消费者 FIFO 模型下无并发竞争，不在本次原子化范围。

### Item 5：关系协议类型定义

**问题**：`crates/agent/src/component/social/relationship.rs` 的 `RelationshipStore` 是每个 agent 的本地 SQLite（rusqlite），server crate 零关系代码。前端永远查不到好感度/认识关系。

**归类论据**（判断乙，3/3 批准）：新建关系存储 = 新建一条有自己正确性/可靠性/暴露需求的数据流，塞进 A 会破坏"让现有数据诚实"的单一使命。但契约必须在 A 定，否则 C 阶段无契约可对齐。

**A 阶段范围**：仅在 protocol 定义关系契约类型，字段集**严格对齐** agent 现有 `relationship_types.rs`（favorability i32、key_events、self_description 等）。不建服务端存储、不建同步链路（归 C）。

## 5. 明确不做（排除清单）

| 排除项 | 归属 | 理由 |
|--------|------|------|
| 消息交付可靠性 / outbox | B | 咨询性通道抖动，非权威数据丢失 |
| 读取 API / 客户端鉴权层 | C | 数据可达性，非正确性 |
| 关系存储与同步 | C | 新建写入路径，有自己的三重问题 |
| Chronicle 事务/幂等/回补 | B | 派生数据可靠性 |
| SoulCycleRecorder 返回 Result | B | 可靠性 |
| 进程内迁移器（sqlx::migrate!） | B | 部署可靠性 |
| 数据驱动字段枚举化 | 伪需求 | 破坏 YAML 扩展根基 |

## 6. 验证策略

- **编译期为主**：枚举化后非法值编译不过；`_=>{}` 改穷尽匹配让漏分支编译期告警。
- **DB 约束按需**：只补明显该有的（NOT NULL、必要 CHECK），不把每个枚举镜像成 DB 约束——避免 schema 与代码双重维护。
- **回归测试**：item 4 原子化补回归测试（参照已有 P0-2 测试 `processor.rs:439`）。
- **每 commit 编译验证**：拆分实施步骤，每步 `cargo check --workspace` + 关键测试。

## 7. 实施顺序（高层，细节由 writing-plans 展开）

1. Item 1 枚举化（protocol 层定义 → game_data/server/agent 消费侧适配 → 修 bug）
2. Item 3 重复类型合并（与 item 1 交错，ItemType 三胞胎优先）
3. Item 2 哨兵值 + serde + 时间戳
4. Item 4 原子化（先审计旁路 → 纳入 tx → 回归测试）
5. Item 5 关系协议类型定义

每步独立 commit，每步编译 + 测试验证。
