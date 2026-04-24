# 自然语言状态映射

**级别**: P2 体验增强
**模块**: `crates/protocol`

## 1. 设计目标
解决大语言模型对绝对数值（如 HP=10，饥饿=80）敏感度低的问题。自动将底层数值状态转化为中文描述文本，提升其角色扮演的自然度和决策准确性。

## 2. 核心机制
### 2.1 阈值配置映射
在配置文件（如 `attributes.yaml` 或专门的映射表）中定义数值区间对应的自然语言描述。
- 例：`hunger > 80` 映射为“你现在饥肠辘辘，感觉眼前发黑”。
- 例：`hp < 20` 映射为“你身负重伤，鲜血染红了衣襟”。

### 2.2 动态替换与 Context 注入
- Agent 的 `CognitiveEngine` 在构建 Prompt 前，拦截解析到的 `WorldState`。
- 将状态向量数组通过映射引擎替换为生动的文本段落，拼接到 `DecisionContextSnapshot` 的“自身状态”模块中。

## 3. 架构约束
- 映射规则必须是数据驱动的（Data-Driven），不能在 Rust 代码中写死 `if hp < 20` 的逻辑。
- 必须支持多语言扩展（虽然当前主要为中文）。

## 4. 代码入口
- Agent 状态组装: `crates/agent/src/core/lifecycle.rs` (组装上下文逻辑)
- 配置解析: `crates/server/src/game_data/loaders/`
