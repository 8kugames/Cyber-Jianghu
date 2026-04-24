# 统一错误码体系 (GameError)

**级别**: P0 核心基石
**模块**: `crates/protocol`

## 1. 设计目标
建立全局统一的错误类型和状态码，确保异常信息在前后端、Server 与 Agent 之间流转时的明确性和可追溯性。这对 AI Agent 的经验学习（Outcome Memory）至关重要。

## 2. 核心机制
### 2.1 错误分层结构
基于 `thiserror` 派生，将错误分为以下几大类：
- **NetworkError**: 网络连接、序列化/反序列化错误。
- **ValidationError**: 动作校验错误（如目标不存在、超出交互距离、冷却中）。
- **StateError**: 状态异常（如体力不足、HP不足、死亡状态下执行动作）。
- **RuleError**: 违反物理或世界观刚性规则（RuleEngine 拦截）。
- **InternalError**: 服务器内部错误（如数据库连接失败）。

### 2.2 错误信息反馈循环
1. Agent 提交非法 Intent。
2. Server 校验失败，返回 `ExecutionResult { success: false, error: GameError::StateError("体力不足") }`。
3. Agent 天魂截获错误，转入 **经验结果记忆 (Outcome Memory)**。
4. 下次 LLM 规划时，Prompt 注入：“你上次因为体力不足动作失败，请先休息。”

### 2.3 中文化支持
为了使 LLM 能够直接理解错误原因，`GameError` 的 `Display` 实现必须是清晰的中文描述，不能是纯技术栈的堆栈信息。

## 3. 架构约束
- **防御性编程**：捕获所有潜在的业务异常，禁止静默吞没错误（如遇到 `unwrap()` panic）。
- 错误信息必须具有明确的中文描述，以供 LLM 直接阅读和理解。
- `GameError` 必须实现 `serde::Serialize` 和 `serde::Deserialize`。

## 4. 代码入口
- 错误定义: `crates/protocol/src/error.rs`
- 执行结果构造: `crates/server/src/tick/processor/executor.rs`
