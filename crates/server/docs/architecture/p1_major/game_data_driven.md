# 游戏数据驱动系统

**级别**: P1 重要特性
**模块**: `crates/server`

## 1. 设计目标
将所有业务规则抽离为外部配置文件，实现修改即生效的“零硬编码”。降低游戏平衡性调整的成本，并让系统运行时状态和规则相解耦。

## 2. 核心机制
### 2.1 YAML / JSON 统一加载
通过 `game_data/loaders` 模块解析位于 `config/` 目录下的各种数据字典：
- `actions.yaml`：定义所有动作及其所需前置条件、分类。
- `attributes.yaml`：定义基础属性名和派生属性的计算公式。
- `game_rules.yaml`：统管衰减速度、观察学习阈值、Tick 时长等。
- `world_building_rules.yaml`、`prompt_templates.yaml`、`recipes.yaml` 等。

解析后统缓存于 `GameDataCache` 并在全局注册（Registry）。

### 2.2 自动热重载 (Hot Reload)
- `TickScheduler` 在主循环中轮询检查配置文件的最后修改时间（`mtime`）。
- 一旦发生修改，触发无缝重载。成功解析后更新 `GameDataCache`。
- 通过 WebSocket 发送 `ConfigUpdate` 消息，将变更推送到所有在线 Agent 端，Agent 收到后可实时调整认知模型。

### 2.3 公式引擎 (evalexpr)
- 对于状态结算（如伤害、消耗、衰减），不采用硬编码。
- 使用 `evalexpr` 库，在运行时动态计算配置中的表达式（如 `(strength * 2) + agility`），并在计算时注入当前的上下文属性字典，实现高度自由的数据平衡。

## 3. 架构约束
- Server 核心业务代码（如 Mutator）不得写死任何数值阈值、固定伤害或特定物品 ID。
- 热重载过程采用 `Arc<RwLock>` 进行保护，必须确保加载和替换的原子性，避免出现只替换了一半规则的中间态。

## 4. 代码入口
- 加载器与注册表: `crates/server/src/game_data/loaders/` 和 `crates/server/src/game_data/registry/`
- 配置缓存总管: `crates/server/src/game_data/cache.rs`
- 公式引擎: `crates/server/src/game_data/formula_engine.rs`
