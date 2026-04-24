# AI 过程性技能系统 (Procedural Skills)

**级别**: P1 重要特性
**模块**: `crates/server`

## 1. 设计目标
基于 Markdown 的行为指令系统，体现“身心分离”架构的核心设计。将复杂的技能规则以自然语言说明书的形式下发给 LLM 阅读，而非在后端写死技能执行代码。

## 2. 核心机制
### 2.1 Server 注册表
- 技能文件位于 `config/skills/{category}/{skill_id}/SKILL.md`。
- 文件采用 YAML Frontmatter 定义基础属性（消耗、冷却、目标），Markdown Body 定义详细的释放流程、光影效果和对目标的具体要求。
- Server 启动时加载并构建 `SkillRegistry`。

### 2.2 习得链路
- Agent 发送 `practice` 动作，尝试学习某项技能。
- Server 校验前置条件后，通过 `SkillMutator` 将该技能 ID 注入 Agent 的 `AgentState.skills`（JSONB 存储）中。

### 2.3 认知集成
- Agent 的地魂实现 `skill_view` 工具。
- LLM 在需要释放技能时，通过工具调用检索该技能的长文本行为指令，理解后构造精准的自然语言 `Intent`，避免将庞大技能规则硬塞入基础 System Prompt。

## 3. 架构约束
- 技能的实现不依赖后端写死的 Switch-Case 逻辑，而是依赖 LLM 对 Markdown 说明书的理解与推理（即过程性生成）。

## 4. 代码入口
- 技能加载: `crates/server/src/game_data/loaders/skills_loader.rs`
- 技能习得: `crates/server/src/actions/executor/interaction.rs`
- 地魂检索工具: `crates/agent/src/soul/earth/skill_tool.rs`
