# 动态角色演化 (Dynamic Persona)

在虚境：江湖中，Agent 的人设不是一成不变的文本，而是会随着游戏事件的发生而产生动态演化（Evolution）。这种演化机制保证了角色的成长和性格转变。

相关代码路径：`crates/agent/src/component/persona/`

## 核心数据结构

### `DynamicPersona`

`DynamicPersona` 是 Agent 当前身份的唯一真相源 (Source of Truth)。它包含：
- **静态部分**：`base_description` (即初始设定的背景故事)
- **动态特质**：`traits`，包含 `Social` (社交), `Moral` (道德), `Capability` (能力), `Emotional` (情绪), `Survival` (生存) 五个维度。
- **状态快照**：`PersonaState`，记录了当前的情绪 (Emotion)、目标 (Goal)、压力值 (Stress) 以及核心情感坐标 (CoreAffect，即效价与唤醒度)。

### 线程安全包装

为了支持 Web 控制面板、HTTP API 与内部 `CognitiveEngine` 的并发访问，使用了 `ThreadSafePersona`：
```rust
pub struct ThreadSafePersona {
    inner: Arc<RwLock<DynamicPersona>>,
}
```
这使得我们可以在 `WebSocket` 接收到事件时实时修改人设，并在下一次 Tick 触发时立刻生效。

## 事件驱动的演化机制 (Event-Trait Mapping)

Agent 每次从 Server 接收到 `WorldEvent` 列表时，会触发演化引擎。

1. **规则加载**：启动时，`rules_loader.rs` 会加载 `persona_event_rules.yaml`（此文件由 Server 下发/同步）。
2. **模式匹配**：`EventTraitMapper` 会逐个检查事件，若事件类型和过滤条件匹配，则触发对应的 `TraitChange`。
3. **特质修正**：特质的 `value` (0-100) 根据 `delta` 发生偏移，并产生一条带 `reason` 和 `timestamp` 的历史记录。

```rust
// crates/agent/src/core/agent.rs -> process_events
for event in events {
    self.persona.write(|p| {
        mapper_guard.apply_to_persona(event, p, event.tick_id);
    });
}
```

## 与 CognitiveEngine 的整合

每次大模型推理前，`CognitiveEngine` 都会通过 `persona_ref` 提取出当前 `DynamicPersona` 的状态，并生成一个整合后的描述（包含基础设定 + 当前特质评价 + 当前状态），作为 `System Prompt` 注入。

这种机制使得如果一个 Agent 连续经历失败导致 `Survival` 特质极低，或者压力值 `stress_level` 极高，他在下一次 LLM 生成时的 `System Prompt` 就会带有“你当前感到极度恐慌和无助”的上下文，进而影响最终决策。
