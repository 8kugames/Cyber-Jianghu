# COI 属性组件 (AttributeComponent)

**级别**: P1 重要特性
**模块**: `crates/protocol`

## 1. 设计目标
采用组合优于继承 (Composition Over Inheritance, COI) 的设计模式，将 Agent 的属性模块化，避免庞大僵化的实体类。

## 2. 核心机制
### 2.1 属性模块化分类
在 `AgentState` 中，属性被拆分为多个解耦的组件结构：
- **基础属性 (Base Attributes)**：力量、敏捷、智力、体质等静态值（通常由创建时决定，较少变动）。
- **动态状态 (Dynamic States)**：HP、内力、体力、饥饿度、口渴度、San值等随时间或行为频繁增减的值。
- **派生属性 (Derived Attributes)**：攻击力、防御力、暴击率等，不直接存储，而是基于公式引擎（`evalexpr`）动态计算。
- **技能组件 (Skills Component)**：掌握的武功、生活技能列表。

### 2.2 协议结构与 JSON 扁平化
- 为了方便网络传输和 LLM 解析，这些组件在 `WorldState` 中被序列化为相对扁平的 JSON/Struct。
- Agent 端收到后可以直接解析，并结合 Natural Language Mapping 转化为 Prompt 文本。

## 3. 架构约束
- 严禁在代码中硬编码属性名称或计算公式。所有属性字段必须与 `attributes.yaml` 数据字典对齐。
- 修改动态状态必须通过 `StateProcessor` 的受控方法（防止数值溢出或产生负数）。

## 4. 代码入口
- 协议定义: `crates/protocol/src/agent.rs` (AgentState)
- Server 端应用: `crates/server/src/game_data/formula_engine/`
