# 世界观设定边界 (WorldBuilding)

**级别**: P2 体验增强
**模块**: `crates/protocol`

## 1. 设计目标
规定游戏所属的时代背景及允许/禁止的概念，从根源限制 LLM 生成不符合“赛博武侠”背景的现代词汇或脱戏内容。

## 2. 核心机制
### 2.1 词汇黑名单与白名单
- 在 `world_building_rules.yaml` 配置中定义禁用的现代词汇或概念（如“手机”、“电脑”、“AI”、“大模型”）。
- 定义特有的专有名词解释（如将“服务器”概念替换为“天道”）。

### 2.2 核心指令植入
- 作为 System Prompt 的最高优先级指令（System Rules），在 `CognitiveEngine` 初始化时固定写入上下文中。
- 持续影响 Agent 的语风与认知边界。

## 3. 架构约束
- 作为一种前置的预防机制，它与天魂的 OOC 拦截（后置校验）形成互补。
- 规则描述必须简明扼要，避免占用过多系统提示词 Token。

## 4. 代码入口
- 提示词模板缓存: `crates/agent/src/soul/actor/prompt_cache.rs`
- 规则配置: `crates/server/config/world_building_rules.yaml`
