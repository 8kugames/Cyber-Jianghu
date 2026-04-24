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
- **执行**：负责将人魂产出的结构化 Intent 序列化为 WebSocket Payload 发往 Server。
- **检索与查阅**：提供 `search_memory`（检索情景/语义记忆）、`recall_archived` 和 `skill_view`（查阅武功等长文本技能详情）工具，供 LLM 在决策中途按需调用，避免撑爆 System Prompt。

### 2.3 天魂 (ReflectorSoul)：三段式审查官
- **Layer 1 动作校验**：基础 ActionType 与参数合法性验证（如动作名是否在词典中，参数是否缺漏）。
- **Layer 2 物理规则 (RuleEngine)**：YAML 配置驱动的世界观刚性规则和物理可行性检验（如防穿墙、防虚空造物、距离校验）。
- **Layer 3 角色 OOC 审查**：将暂定动作与角色的 Persona（性格标签）送入专属的 LLM 审查 Prompt 中，拦截出戏行为（如赛博精神病、说现代网络语），并按严重程度分类 OOC 等级，生成失败的 ExecutionResult 供下一次人魂反思。

## 3. 架构约束
- 三魂之间严禁越权调用。人魂只管“想”，地魂只管“做”和“查”，天魂只管“审”。
- 天魂拦截必须快，Layer 1 和 2 是纯本地运算，Layer 3 仅针对高风险动作（分级验证）或低 San 状态开启。

## 4. 代码入口
- 人魂引擎: `crates/agent/src/soul/actor/engine.rs`
- 地魂工具: `crates/agent/src/soul/earth/executor.rs`
- 天魂拦截: `crates/agent/src/core/reflector_ext.rs`
