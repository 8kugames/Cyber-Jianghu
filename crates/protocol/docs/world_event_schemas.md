# WorldEvent Schemas

## WorldEventType Enum

| Variant | String Value | Description |
|---------|-------------|-------------|
| `PublicMessage` | `"public_message"` | 公开说话（speak） |
| `PrivateDialogue` | `"private_dialogue"` | 密语通知（whisper 摘要，不含内容） |
| `ActionResult` | `"action_result"` | 动作结果 |
| `EnvironmentalChange` | `"environmental_change"` | 环境变化 |
| `StateChange` | `"state_change"` | 状态变更（如死亡、复活） |
| `TimeUpdate` | `"time_update"` | 时间更新 |
| `SystemNotification` | `"system_notification"` | 系统通知 |
| `DeathNotification` | `"death_notification"` | 死亡通知 |
| `SocialInteraction` | `"social_interaction"` | 社交互动 |

## WorldEvent Structure

```json
{
  "event_type": "public_message",  // Variant name (snake_case)
  "tick_id": 123,
  "description": "有人说: 你好",
  "metadata": {
    "from_agent_id": "uuid",
    "content": "你好",
    "channel": "local",
    "location": "village_center"
  }
}
```

### Metadata by EventType

#### PublicMessage (公开说话)
```json
{
  "from_agent_id": "uuid",
  "content": "说话内容",
  "channel": "local",
  "location": "当前节点ID"
}
```

#### PrivateDialogue (密语会话)
```json
{
  "session_id": "uuid",
  "agent_a_id": "uuid",
  "agent_b_id": "uuid",
  "message_count": 3
}
```

#### ActionResult (动作结果)
```json
{
  "action": "动作类型",
  "target": "目标ID",
  "item_id": "物品ID",
  "quantity": 1,
  "result": "成功/失败"
}
```

#### EnvironmentalChange (环境变化)
```json
{
  "type": "hunger|thirst|...",
  "value": -10,
  "current": 20
}
```

#### StateChange (状态变更)
```json
{
  "attribute": "hp",
  "old_value": 100,
  "new_value": 0,
  "cause": "战斗/饥饿/..."
}
```

#### TimeUpdate (时间更新)
```json
{
  "season": "春",
  "day": 1,
  "hour": 8,
  "is_daytime": true
}
```

#### SystemNotification (系统通知)
```json
{
  "type": "notification_type",
  "message": "通知内容"
}
```

#### DeathNotification (死亡通知)
```json
{
  "type": "death_notification",
  "cause": "hunger|战斗|...",
  "message": "你已死亡"
}
```

## PrivateDialogueRecord Structure

密语记录（不含内容，仅索引）

```json
{
  "session_id": "uuid",
  "agent_a_id": "uuid",
  "agent_a_name": "角色A",
  "agent_b_id": "uuid",
  "agent_b_name": "角色B",
  "message_count": 5,
  "last_message_from": "角色A"
}
```

## RecentAction Structure

最近动作记录（用于 Entity.recent_actions）

```json
{
  "tick_id": 123,
  "action_type": "PublicMessage",
  "content": "说话内容（如果有）",
  "result": "事件描述"
}
```

## ImmediateEvent (立即事件)

`speak` 等需要立即广播的事件通过 `ServerMessage::ImmediateEvent` 推送，而非完整的 WorldState。

```json
{
  "type": "immediate_event",
  "event": {
    "event_type": "public_message",
    "tick_id": 123,
    "description": "有人说: 你好",
    "metadata": {
      "from_agent_id": "uuid",
      "content": "你好",
      "channel": "speak",
      "location": "village_center"
    }
  },
  "deadline_ms": 18446744073709551615
}
```

**特点**:
- 只包含单个事件，带宽占用最小
- `deadline_ms = u64::MAX` 表示无截止时间（立即事件）
- 通过 WebSocket 实时推送，不等待 Tick 周期

## events_log Filtering

`events_log` 只包含当前 Agent 所在场景的事件。判断规则：
- 如果 `metadata.location` 存在，则只保留 `location == agent.current_node_id` 的事件
- 如果 `metadata.location` 不存在（全局事件如系统通知），则对所有 Agent 可见

## WorldState.private_dialogue_log

每个 Tick 结束时，所有进行中的密语会话被强制关闭，并在下一轮 WorldState 中通过 `private_dialogue_log` 通知双方：

```json
"private_dialogue_log": [
  {
    "session_id": "...",
    "agent_a_id": "...",
    "agent_a_name": "角色A",
    "agent_b_id": "...",
    "agent_b_name": "角色B",
    "message_count": 3,
    "last_message_from": "角色A"
  }
]
```

注意：密语内容不会通过 WorldState 传递，内容仅由对话双方持有。
