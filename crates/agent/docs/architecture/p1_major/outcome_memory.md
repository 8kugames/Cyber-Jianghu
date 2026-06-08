# 经验结果记忆 (Outcome Memory / Hermes)

Outcome Memory（内部代号 Hermes）是 Agent 独有的一套**行动结果反馈学习系统**。它的核心目标是解决大模型“在同一个坑里反复摔倒”的问题。

相关代码路径：`crates/agent/src/component/memory/outcome.rs`

## 设计动机

在 MMO 环境中，Agent 可能会做出不符合当前物理规则的行动（例如“给李四 100 文钱”，但实际上余额不足，或者李四不在旁边）。当服务端驳回（Reject）这个意图时，如果不加以记录，下一次遇到类似情况时，大模型由于其无状态的本质，极大概率会**再次尝试相同的错误操作**。

Hermes 系统通过记录每一条 `Intent` 的最终执行结果 (`Success` 或 `Failed`)，并结合当时的环境哈希 (`context_hash`)，让大模型在决策前就能“想起”以前的教训。

## 数据结构：OutcomeRecord

每一条行动记录都被保存在 SQLite 中：

```rust
pub struct OutcomeRecord {
    pub action_type: String,             // 动作类型 (如 "give")
    pub action_data: Option<Value>,      // 核心参数 (精简版)
    pub result: OutcomeResult,           // 成功或失败(带原因)
    pub target_agent_id: Option<String>, // 交互目标 (如果是社交动作)
    pub context_hash: String,            // 环境哈希指纹
    pub tick_id: i64,                    // 发生时间
}
```

### Context Hash (环境指纹)

为了匹配“相似场景”，Hermes 在记录时会生成当前 `WorldState` 的指纹 `context_hash`。
指纹提取了：当前位置 (Location ID)、周围的实体类型概览等核心要素。
当 Agent 再次处于相同的 `context_hash` 时，系统会优先检索该环境下的失败记录。

## 记忆注入管线

每次大模型推理前，系统会自动向 Prompt 中注入两类 Outcome 记录：

1. **同场景经验**：根据当前的 `context_hash` 查询该场景下最近发生的成功/失败经验。
2. **同动作经验**：大模型可以通过 EarthSoul 的 tool calling 主动查询某个特定 `action_type` 的过往经验。

### 示例

注入给 LLM 的 Prompt 可能是这样的：

```markdown
## 历史经验 (Outcome Memory)
- [Tick 105] 你尝试在当前场景执行 `trade` 失败了。原因：你没有足够的碎银。
- [Tick 108] 你尝试对 `npc_002` 执行 `whisper` 成功了。
```

这种硬性反馈机制能极大地收敛大模型的幻觉，迫使其在反复失败后主动寻找其他 `action_type`（比如去赚钱，而不是继续强行购买）。
