# Agent 生命周期

## 注册流程

```
1. 设备连接: POST /api/v1/agent/connect --> { device_id, auth_token }
2. WebSocket 连接: ws://server/ws?device_id=xxx&token=yyy
3. Agent 注册: POST /api/v1/agent/register --> { agent_id, narrative_config }
4. 接收 Registered 消息（包含游戏规则）
5. 开始 Tick 循环
```

## 死亡处理

当 Agent 死亡后提交意图：

1. Server 在 WebSocket handler 中检测 Agent 存活状态
2. 如果死亡，返回 `ServerMessage::Error`：
   ```json
   {
     "type": "error",
     "code": "agent_dead",
     "message": "Agent 已死亡，无法执行此动作。请重新转生入世。",
     "tick_id": 123
   }
   ```
3. Agent 需要调用 `/api/v1/agent/rebirth` 删除旧角色
4. 重新注册新角色

### 重要说明

- 错误消息通过 WebSocket 传递
- **OpenClaw 必须通过 WebSocket 提交意图**，不能使用 HTTP API
- Agent 的 HTTP API `POST /api/v1/intent` **仅用于调试**，存在时序问题
- OpenClaw 通过 Agent 的 WebSocket 接收 `server_error` 消息感知错误

## 转生流程

```
POST /api/v1/agent/rebirth
{
  "device_id": "uuid",
  "auth_token": "token"
}

Response:
{
  "retired_agent_id": "uuid",
  "retired_name": "角色名"
}
```
