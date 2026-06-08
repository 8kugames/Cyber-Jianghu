# 认知流转引擎 (Cognitive Engine)

`CognitiveEngine` 是 Agent 的“大脑”（属于**人魂 ActorSoul**），负责接收来自服务端的客观世界状态 (`WorldState`)，结合自身记忆和人设，生成结构化的决策意图 (`Intent`)。

相关代码路径：`crates/agent/src/soul/actor/engine.rs`

## 核心架构演进

在当前版本中，认知引擎采用了**单次调用、端到端生成**的架构。彻底消除了旧版本中从“叙事意图”（自然语言）到“精确意图”（ID）的中间翻译步骤。

1. **输入**：`WorldState`（包含实体 UUID、物品 ID、节点 ID）。
2. **处理**：`CognitiveEngine` 单次调用大模型，要求直接输出可执行的结构化 `Intent`（包含精确的 `action_type` 和 `action_data`）。
3. **输出**：多原子 Intent 队列结构 (`DirectCognitiveAction` 数组)。

## 四阶段统一结构 (Four-Stage Structured Output)

引擎内部在逻辑上分为四个阶段（感知、动机、规划、决策），但在实际执行时，为了降低延迟和 Token 消耗，被合并为一次 LLM 调用。大模型需返回一个满足 `DirectCognitiveResponse` 结构的 JSON：

```json
{
  "self_status": {...},            // [感知] 自身状态总结
  "environment": {...},            // [感知] 环境状态总结
  "key_observations": ["..."],     // [感知] 关键观察
  "primary_drive": "...",          // [动机] 主要驱动力
  "drive_intensity": 8,            // [动机] 驱动强度
  "thought_process": "...",        // [规划] 思考过程
  "actions": [                     // [决策] 结构化输出（支持 1~3 个动作组成 Pipeline）
    {
      "action_type": "move",
      "action_data": { "target_node": "c001_inn" }
    }
  ],
  "should_remember": true,         // 是否需要写入记忆
  "memory_content": "...",         // 记忆内容
  "constructed_emotion": {         // 构建出的核心情绪 (CoreAffect)
    "label": "警惕",
    "reasoning": "...",
    "intensity": 0.7
  }
}
```

## 动态上下文组装

为了让 LLM 做出正确的决策，`CognitiveEngine` 每次调用都会构建一个庞大的上下文环境：

1. **基础人设 (Persona)**: 从 `Agent.persona` 中获取。
2. **规则缓存 (Rule Cache)**: 基于 `prompt_template` 和 Server 推送的规则动态组装。
3. **记忆上下文 (Memory Context)**: 
   - **工作记忆 (Working Memory)**: 最近发生的事情。
   - **情景记忆 (Episodic Memory)**: 按重要性排序的核心事件。
   - **经验记忆 (Outcome Memory)**: 之前做过类似事情的成败经验（"Hermes" 系统）。
4. **注意力焦点 (Focus Summary)**: 由 `DeltaEngine` 零 Token 检测出的状态增量，通过 `AttentionController` 过滤后的核心焦点。
5. **滑动窗口 (Narrative Summary Window)**: 存储过去几轮的 `(Intent, 执行结果)`，让 LLM 知道刚才发生了什么。
6. **可用动作清单**: 直接在 Prompt 中注入合法的 `action_type` 及其参数 Schema (`action_field_hints`)。

## 地魂的嵌入式调用 (Tool-Calling / EarthSoul)

**地魂 (EarthSoul) 不是独立的 Pipeline 步骤，而是内嵌于认知引擎中。**
如果底层模型（如 `qwen2.5`）支持 Tool Calling，`CognitiveEngine` 会在向大模型发送 Prompt 时携带地魂工具列表。

在推理过程中，模型可以决定暂停推理并调用工具，例如：
- `search_memory`: 查询遥远的语义记忆
- `skill_tool`: 获取某项特长技能的具体执行知识
- `relationship_tool`: 查询与某个角色的社交关系

系统在执行工具后，将结果拼接回消息列表，LLM 继续推理，直至最终输出 `DirectCognitiveResponse`。

## 混沌降级 (Chaos Generator)

为了保证 Agent 在高压或 LLM 故障时不至于卡死，内置了 `ChaosGenerator` (`crates/agent/src/soul/actor/chaos.rs`)。
- 当连续 N 次 LLM 失败（如限流、解析 JSON 失败）时触发。
- 或当角色的理智值 (Sanity) 极低时触发。
- 混沌状态下，Agent 将退化为本能驱动（饥饿时随机进食、受击时逃跑或反击），不依赖 LLM 即可生成合法的 Intent。
