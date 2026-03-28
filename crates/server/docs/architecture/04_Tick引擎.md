# Tick 引擎

## Tick 循环流程

Tick 引擎按固定顺序执行以下阶段：

```
Phase 1: 加载状态 --> 阶段2.1: 结算意图 --> 阶段2.2: 衰减处理 --> Phase 3: 统计 --> Phase 4: 持久化 --> Phase 5: 广播
```

### 各阶段说明

| 阶段 | 说明 |
|------|------|
| Phase 1 | 从 PostgreSQL 加载所有 Agent 状态 |
| 阶段2.1: 结算意图 | 收集意图 → 验证 → 冲突解析 → 执行动作 |
| 阶段2.2: 衰减处理 | 属性衰减、物品耐久、环境伤害（在意图执行之后） |
| Phase 3: 统计 | 汇总本 Tick 统计数据、超时跟踪 |
| Phase 4: 持久化 | 保存状态到 PostgreSQL |
| Phase 5: 广播 | 推送 WorldState |

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
