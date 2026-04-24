# 数据驱动的动作系统 (ActionType)

**级别**: P0 核心基石
**模块**: `crates/protocol`

## 1. 设计目标
将游戏内所有可执行的交互动作抽象化、数据化，彻底消灭硬编码逻辑，实现完全的配置驱动（Data-Driven）。使得新增动作（如增加一个“潜行”动作）只需修改 YAML 配置，无需重新编译核心引擎。

## 2. 核心机制
### 2.1 动作标识符 (ActionType)
动作不再被定义为 Rust 枚举的强类型成员，而是被统一抽象为字符串标识符（如 `attack`, `move`, `practice`）。

### 2.2 参数动态解析 Schema
每个动作的执行参数通过 JSON Schema 或 YAML 字典定义，包含：
- `target_id` (目标 UUID)
- `item_id` (物品 ID)
- `node_id` (目标地点节点)
- `amount` (数量)
- `text` (对话或自定义文本)

### 2.3 解耦设计流程
1. **Agent 认知层**：读取 `actions.yaml` 了解可用动作及其参数格式，LLM 决定行动后构造对应的 JSON。
2. **Protocol 传输层**：使用 `Intent { action: String, parameters: Map }` 进行无差别传输。
3. **Server 验证层**：`StateProcessor` 根据 `actions.yaml` 中定义的规则（如消耗体力、冷却时间、前置条件）动态验证该动作是否合法。
4. **Server 执行层**：路由到对应的 `ActionExecutor` 处理具体逻辑。

## 3. 架构约束
- 严禁在代码中写死具体的动作处理逻辑，必须通过通用的 `Executor` 管线结合配置文件执行。
- 任何新增的动作必须在 `actions.yaml` 中有明确的定义，否则在 Server 的 Layer 1 校验阶段会被拒绝。

## 4. 代码入口
- 协议定义: `crates/protocol/src/action.rs`
- 动作执行器映射: `crates/server/src/actions/executor/mod.rs`
- 动作校验规则: `crates/server/src/actions/validator.rs`
