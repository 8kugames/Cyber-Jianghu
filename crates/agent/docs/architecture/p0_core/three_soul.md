# 三魂架构 (Three-Soul)

**级别**: P0 核心基石
**模块**: `crates/agent`

## 1. 设计目标
Agent 决策的哲学分层模型，彻底隔离认知推演、物理执行与自我审查，实现高度解耦。使得大模型（LLM）的创造力与游戏系统的确定性得以安全融合。

## 2. 核心机制
### 2.1 人魂 (ActorSoul)：感性与理性大脑
- **职能**：主导动机推演与规划。
- **直连环境**：接收并解析 Server 下发的 `WorldState`，结合工作记忆和社交关系生成最终的动作意图（Intent）。
- **混沌注入器**：内置低 San 值（理智值）混沌行为检测。当 San 值低于阈值时，人魂可能无视正常逻辑，强制生成非理性行为（如发疯、喃喃自语）。

### 2.2 地魂 (EarthSoul)：工具执行池
- **职能**：对接物理世界的桥梁与大模型工具箱（Tool-Calling）。
- **执行**：负责提供工具调用能力（如记忆检索、技能查阅等）并返回结构化结果，不负责最终 Intent 发包。
- **检索与查阅**：提供 `search_memory`（检索情景/语义记忆）、`recall_archived` 和 `skill_view`（查阅武功等长文本技能详情）工具，供 LLM 在决策中途按需调用，避免撑爆 System Prompt。

### 2.3 天魂 (ReflectorSoul)：三段式审查官
- **统一入口**：运行时三层审查统一由 `ReflectorSoul` 执行，`lifecycle` 只负责把人魂产出的 Intent 送入天魂，并在驳回后把原因回灌给人魂重提。
- **Layer 1 动作校验**：基础 ActionType 合法性验证，拦截非法动作名和格式哨兵值。
- **Layer 2 物理规则 (RuleEngine)**：本地确定性规则校验，如连续 `follow` 限制、物品/地点 ID 可达性校验。当前实现是本地 RuleEngine 默认规则，不是 YAML 热加载的通用物理引擎。
- **Layer 3 角色 OOC 审查**：将暂定动作与角色 Persona 送入专属 LLM 审查 Prompt 中，拦截出戏行为。被天魂驳回的原因会直接反馈给人魂，驱动同一轮决策闭环内的重新提交。

## 3. 架构约束
- 三魂之间严禁越权调用。人魂只管“想”，地魂只管“做”和“查”，天魂只管“审”。
- 天魂拦截必须快，Layer 1 和 2 是纯本地运算；Layer 3 是否开启由分级验证配置决定。`Skip`/`Adaptive`/`Always` 的路由判断发生在天魂内部。

## 4. 代码入口
- 人魂引擎: `crates/agent/src/soul/actor/engine.rs`
- 地魂工具: `crates/agent/src/soul/earth/executor.rs`
- 天魂本体: `crates/agent/src/soul/reflector/validator.rs`
- Agent 编排: `crates/agent/src/core/reflector_ext.rs`
