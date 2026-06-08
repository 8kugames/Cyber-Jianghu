# AI 过程性技能系统 (Procedural Skills)

**级别**: P1 重要特性
**模块**: `crates/server`

## 1. 设计目标
基于 Markdown 的行为指令系统，体现“身心分离”架构的核心设计。将复杂的技能规则以自然语言说明书的形式下发给 Agent 的 LLM 认知引擎阅读，而非在后端（Server）写死具体的技能执行代码。这些技能通常代表一种“元认知”或高级社交策略，而非传统的 RPG 伤害技能。

## 2. 核心机制
### 2.1 Server 端注册表
- 技能文件存放在 `config/skills/{category}/{skill_id}/SKILL.md`。
- 文件采用 YAML Frontmatter 定义基础属性（名称、分类等），Markdown Body 定义详细的认知推理流程、应用场景和限制要求。
- Server 启动和热重载时加载这些文件，构建出内部的 `SkillRegistry`。

### 2.2 基于经验的自动习得
- 当 Agent 执行动作（Action）时，`StateProcessor` 会记录各类动作（如社交、生存、战斗）的执行次数 (`action_counts`)。
- `StateProcessor` 会根据 `game_rules.yaml` 中的 `skill_acquisition` 阈值配置进行检查。
- 当特定类别的动作执行次数达到阈值，Server 会自动生成 `SkillLearned` 的状态变更（`StateChange`），将该技能 ID 注入到 Agent 的 `AgentState.skills` 中。

### 2.3 技能说明书下发 (ConfigUpdate)
- 技能习得后，Server 会构建包含技能完整 Markdown 文本的 `SkillContent`，并通过 WebSocket 的 `ConfigUpdate` (增量更新) 立刻推送给对应的 Agent。
- Agent 端接收后将其存入本地缓存。在构建认知决策上下文（Decision Context）时，若 Agent 拥有该技能，技能内容会被注入 Prompt 中指导 LLM 做出更符合该策略意图的决策。

## 3. 架构约束
- Server 只负责阈值统计、习得判定和文本分发，绝对不介入基于该技能的逻辑执行（除了可能的基础动作拦截）。具体的“如何运用技能”完全交由 Agent 的大模型依据 SKILL.md 去推理和实施。

## 4. 代码入口
- 技能加载: `crates/server/src/game_data/loaders/skills_loader.rs`
- 经验与习得结算: `crates/server/src/tick/processor/processor.rs` (`check_skill_acquisition` 和 `SkillLearned` mutator)
