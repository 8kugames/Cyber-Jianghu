# Tick 引擎

## Tick 循环流程

Tick 引擎按固定顺序执行以下阶段：

```
意图收集 --> 验证 --> 冲突解析 --> 执行 --> 状态更新 --> 衰减处理 --> 广播 --> 持久化
```

### 各阶段说明

| 阶段 | 说明 |
|------|------|
| 意图收集 | 收集所有存活 Agent 的 Intent |
| 验证 | 检查动作合法性 |
| 冲突解析 | 处理多意图冲突 |
| 执行 | 按优先级执行动作 |
| 状态更新 | 应用状态变更，生成事件 |
| 衰减处理 | 属性衰减、物品耐久等 |
| 广播 | 推送 WorldState |
| 持久化 | 保存到 PostgreSQL |

## 意图处理

### 意图收集

```rust
struct IntentCollector {
    pending_intents: HashMap<AgentId, Vec<Intent>>,
}
```

### 意图验证

```rust
fn validate_intent(intent: &Intent, state: &WorldState) -> Result<(), ValidationError> {
    // 1. 检查 Agent 是否存活
    // 2. 检查动作类型是否有效
    // 3. 检查动作参数是否合法
    // 4. 检查资源是否足够
}
```

### 冲突解析

```rust
fn resolve_conflicts(intents: &[Intent]) -> Vec<Intent> {
    // 1. 按优先级排序
    // 2. 处理位置冲突
    // 3. 处理资源冲突
    // 4. 返回执行列表
}
```

## Tick 配置

- 持续时间 60 秒（可配置）
- 意图窗口为当前 Tick 关闭前，拒收过期 Tick
