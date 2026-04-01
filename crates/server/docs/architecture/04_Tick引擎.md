# Tick 引擎

## Tick 循环时序

Tick 引擎主循环时序（每个周期）：

```
1. 开单 + 广播: 设置 accepting_tick_id，广播 WorldState（基于上次持久化的状态）
2. 收集窗口: sleep(collection_window_secs)，Agent 提交意图
3. 关单: accepting_tick_id 归零，拒收新意图
4. 结算:
   Phase 1: 加载状态 --> 阶段2.1: 结算意图 --> 阶段2.2: 衰减处理 --> Phase 3: 统计 --> Phase 4: 持久化
```

### 各阶段说明

| 阶段 | 说明 |
|------|------|
| 广播 | 新 Tick 开始时立即推送 WorldState，`deadline_ms` 为绝对 Unix 毫秒时间戳 |
| 收集窗口 | 等待 `collection_window_secs` 秒，仅接受当前 `accepting_tick_id` 的意图 |
| Phase 1 | 从 PostgreSQL 加载所有 Agent 状态 |
| 阶段2.1: 结算意图 | 收集意图 -> 验证 -> 冲突解析 -> 执行动作 |
| 阶段2.2: 衰减处理 | 属性衰减、物品耐久、环境伤害（在意图执行之后） |
| Phase 3: 统计 | 汇总本 Tick 统计数据、超时跟踪 |
| Phase 4: 持久化 | 保存状态到 PostgreSQL（结算事件在下一个 Tick 的广播中推送） |

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
- 意图窗口为 `collection_window_secs`（可配置），过期或关单后拒收
- `accepting_tick_id` 为内存原子变量，非 DB 查询，零 IO 开销
- `deadline_ms` 为关单时刻的绝对 Unix 毫秒时间戳
