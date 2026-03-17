#!/bin/bash
# ============================================================================
# OpenClaw Agent Workspace Initialization Script
# ============================================================================
#
# Usage: ./init-agent-workspace.sh <agent-name> <http-port>
#
# Example:
#   ./init-agent-workspace.sh xiaoming 23333
#
# This creates:
#   ~/.openclaw/cyber-jianghu-agents/<agent-name>/
#   ├── SOUL.md           - Agent persona and character
#   ├── AGENTS.md         - OpenClaw agent configuration
#   ├── TOOLS.md          - Available tools documentation
#   └── CONTEXT.md        - Current game state (auto-generated)
# ============================================================================

set -e

AGENT_NAME="${1:-player}"
HTTP_PORT="${2:-23333}"
WORKSPACE="$HOME/.openclaw/cyber-jianghu-agents/$AGENT_NAME"

echo "=== Initializing OpenClaw Agent Workspace ==="
echo "Agent Name: $AGENT_NAME"
echo "HTTP Port: $HTTP_PORT"
echo "Workspace: $WORKSPACE"
echo ""

# Create workspace directory
mkdir -p "$WORKSPACE"

# Create SOUL.md - Agent persona
cat > "$WORKSPACE/SOUL.md" << 'EOF'
# Agent Soul: 赛博江湖侠客

## 基本信息

- **姓名**: {AGENT_NAME}
- **身份**: 江湖侠客
- **年龄**: 28岁

## 性格特征

1. **沉稳**: 遇事不慌，三思而后行
2. **重情义**: 重视朋友，讲究江湖道义
3. **谨慎**: 对陌生人和环境保持警惕

## 核心价值观

- 江湖道义为先
- 不欺凌弱小
- 知恩图报

## 语言风格

- 自称"在下"或"小可"
- 说话客气但有原则
- 遇到冲突时据理力争

## 行为准则

1. 优先保证自身安全
2. 在力所能及的范围内帮助他人
3. 不参与无谓的争斗
4. 合理利用资源

## 当前目标

在江湖中立足，探索这个世界，结交朋友，提升自身实力。
EOF

# Replace placeholder with actual agent name
sed -i.bak "s/{AGENT_NAME}/$AGENT_NAME/g" "$WORKSPACE/SOUL.md"
rm -f "$WORKSPACE/SOUL.md.bak"

# Create AGENTS.md - OpenClaw configuration reference
cat > "$WORKSPACE/AGENTS.md" << EOF
# OpenClaw Agent Configuration

## HTTP API Mode

This agent connects to crates/agent via HTTP API.

### Configuration

- **HTTP Port**: {HTTP_PORT}
- **API Base URL**: http://127.0.0.1:{HTTP_PORT}

### Available Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| \`/api/v1/health\` | GET | Health check |
| \`/api/v1/state\` | GET | Get WorldState |
| \`/api/v1/intent\` | POST | Submit Intent |
| \`/api/v1/validate\` | POST | Validate Intent |
| \`/api/v1/memory/recent\` | GET | Recent memories |
| \`/api/v1/memory/search\` | POST | Search memories |

### Hooks

- **agent:bootstrap**: Fetches WorldState and generates CONTEXT.md
- **agent_end**: Persists decision to memory, enforces jianghu_act usage

### Cron Schedule

- **game-tick**: Every second - main decision loop
- **connection-check**: Every 30 seconds - health check
EOF

# Replace placeholders
sed -i.bak "s/{HTTP_PORT}/$HTTP_PORT/g" "$WORKSPACE/AGENTS.md"
rm -f "$WORKSPACE/AGENTS.md.bak"

# Create TOOLS.md - Available tools
cat > "$WORKSPACE/TOOLS.md" << 'EOF'
# Available Tools

## jianghu_act (REQUIRED)

⚠️ **CRITICAL**: You MUST call this tool every tick. No exceptions.

### Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| action | string | Yes | Action type |
| target | string | No | Target entity/item/location ID |
| data | string | No | Extra data (speech, item ID, etc.) |
| reasoning | string | No | Your thinking process |

### Action Types

| Action | Description | Required Params |
|--------|-------------|-----------------|
| idle | No action (safe fallback) | None |
| speak | Speak to nearby entities | data = speech content |
| move | Move to new location | target = location ID |
| attack | Attack a target | target = entity ID |
| use | Use an item | data = item ID |
| pickup | Pick up ground item | data = item ID |
| drop | Drop item from inventory | data = item ID:quantity |
| give | Give item to someone | target = entity ID, data = item ID |
| steal | Steal from someone | target = entity ID, data = item ID |
| trade | Trade with someone | target = entity ID, data = item ID:price |
| gather | Gather resources | target = resource ID |
| craft | Craft an item | data = recipe ID |

### Example

```json
{
  "action": "speak",
  "data": "各位好，在下初来乍到，还请多多指教。",
  "reasoning": "刚到新地方，应该礼貌地打招呼"
}
```

### Validation

Actions are validated by crates/agent before submission:
1. Rule-based validation (fast, local)
2. LLM validation (if triggered by consecutive failures)

If validation fails, you'll receive feedback and should retry with a corrected action.
EOF

# Create initial CONTEXT.md
cat > "$WORKSPACE/CONTEXT.md" << 'EOF'
# 游戏状态上下文

> 此文件由 `agent:bootstrap` Hook 自动生成
> 每次 Tick 更新，包含当前游戏状态的完整信息

## 当前 Tick

- **Tick ID**: 0
- **Agent ID**: unknown

## 状态

等待 crates/agent HTTP API 连接...

---

*最后更新: 初始化*
EOF

echo ""
echo "=== Workspace Initialized ==="
echo "Workspace: $WORKSPACE"
echo ""
echo "Files created:"
echo "  - SOUL.md    - Agent persona"
echo "  - AGENTS.md  - OpenClaw configuration"
echo "  - TOOLS.md   - Available tools"
echo "  - CONTEXT.md - Initial context"
echo ""
echo "Next steps:"
echo "1. Start crates/agent: cargo run --bin cyber-jianghu-agent -- start --http-port $HTTP_PORT"
echo "2. Configure OpenClaw agent with the template"
echo "3. Start OpenClaw with the cyber-jianghu skill"
