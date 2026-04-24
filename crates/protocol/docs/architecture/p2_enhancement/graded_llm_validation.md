# 分级 LLM 验证机制

**级别**: P2 体验增强
**模块**: `crates/protocol`

## 1. 设计目标
在天魂（ReflectorSoul）的审查机制中，根据行为的出戏 (OOC) 风险等级，动态决定是否调用大模型进行耗时的语义审核，从而在“防止人设崩塌的安全性”与“系统算力成本”之间取得平衡。

## 2. 核心机制
### 2.1 分级定义 (Validation Level)
- **Always (总是审核)**：高风险行为（如复杂的对话内容、非常规动作、特殊的技能释放），必须经过 Layer 3 大模型校验。
- **Adaptive (自适应审核)**：基于当前 Agent 的 San 值和环境压力动态判断。正常状态跳过，低 San 混沌状态强制审核。
- **Skip (跳过审核)**：低风险且标准化的基础动作（如移动、拾取、进食），直接通过天魂的 Layer 1 和 Layer 2 物理规则校验，跳过大模型。

### 2.2 配置化与执行
各动作的验证等级在 Server 端的 YAML 动作字典中定义，通过协议同步给 Agent。
天魂在执行 `reflector_check` 时，优先读取该配置进行短路判断。

## 3. 架构约束
- 默认动作采用 Skip 或 Adaptive。
- 只有包含不可控自由文本的交互（如 `speak` 的内容参数）必须采用 Always，以防止大模型生成现代词汇或违规内容。

## 4. 代码入口
- 审查逻辑: `crates/agent/src/core/reflector_ext.rs`
- 规则加载: `crates/server/config/actions.yaml` (验证级别字段)
