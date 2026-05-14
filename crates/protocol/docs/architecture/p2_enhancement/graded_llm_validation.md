# 分级 LLM 验证机制

**级别**: P2 体验增强
**模块**: `crates/protocol`

## 1. 设计目标
在天魂（ReflectorSoul）的审查机制中，根据行为的出戏 (OOC) 风险等级，动态决定是否调用大模型进行耗时的语义审核，从而在“防止人设崩塌的安全性”与“系统算力成本”之间取得平衡。

## 2. 核心机制
### 2.1 分级定义 (Validation Level)
- **Always (总是审核)**：高风险行为（如复杂的对话内容、非常规动作、特殊的技能释放），必须经过 Layer 3 大模型校验。
- **Adaptive (自适应审核)**：基于动作字段映射与风险关键词动态判断。典型检查包括 `target_location` 是否命中限制区域关键词、`item_id` 是否命中高价值物品关键词。
- **Skip (跳过审核)**：低风险且标准化的基础动作（如移动、拾取、进食），直接通过天魂的 Layer 1 和 Layer 2 物理规则校验，跳过大模型。

### 2.2 配置化与执行
验证等级由 Server 基于动作配置生成后通过协议同步给 Agent。
天魂在运行时统一入口中读取该配置，决定是否执行 Layer 3；Layer 1 和 Layer 2 始终由 ReflectorSoul 执行。

## 3. 架构约束
- 默认动作采用 Skip 或 Adaptive。
- 只有包含不可控自由文本的交互（如 `speak` 的内容参数）必须采用 Always，以防止大模型生成现代词汇或违规内容。

## 4. 代码入口
- 审查逻辑: `crates/agent/src/soul/reflector/validator.rs`
- Agent 编排: `crates/agent/src/core/reflector_ext.rs`
- 分级配置来源: `crates/server/config/actions.yaml` -> 协议下发 `GradedValidationConfig`
