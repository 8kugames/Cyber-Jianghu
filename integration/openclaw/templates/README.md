# OpenClaw Agent Configuration Templates

This directory contains template configurations for different types of agents.

## Directory Structure

```
integration/openclaw/
├── hooks/       # TypeScript Hooks
├── templates/   # Configuration templates (you are here)
├── SKILL.md     # OpenClaw Skill definition
└── init-agent-workspace.sh  # Workspace initialization script
```

## Templates

### player-agent.json5

Template for active player agents that participate in the game world.

**Features:**
- 30-second heartbeat for responsive gameplay
- Full hook integration (bootstrap, validator, memory)
- Optimized for decision-making and action execution

**Use cases:**
- Player characters
- NPCs with autonomous behavior
- Game testing agents

### observer-agent.json5

Template for passive observer agents that watch and record.

**Features:**
- 60-second heartbeat (less resource intensive)
- Bootstrap and memory hooks only (no action validation)
- Event recording and daily summaries
- Sandbox isolation enabled by default

**Use cases:**
- World event loggers
- Combat analyzers
- Economic observers
- Social interaction researchers

## Usage

### Using the Init Script (Recommended)

The easiest way to create a new agent workspace:

```bash
./integration/openclaw/init-agent-workspace.sh my-agent 23333
```

This will:
1. Create a workspace at `~/.openclaw/cyber-jianghu-agents/my-agent/`
2. Generate SOUL.md, AGENTS.md, TOOLS.md
3. Create agent.json with default configuration

### Manual Setup

1. Copy a template:
```bash
cp integration/openclaw/templates/player-agent.json5 ~/.openclaw/agents/my-agent.json5
```

2. Replace placeholders:
- `{AGENT_NAME}` - Your agent's name
- `{AGENT_DESCRIPTION}` - Brief description
- `{TCP_PORT}` - Local API port (unique per agent)
- `{YOUR_AUTH_TOKEN}` - Game server auth token

3. Configure OpenClaw to use the configuration

## Multi-Agent Setup

When running multiple agents, each needs:
- Unique workspace directory
- Unique TCP port (23333, 23334, 23335, ...)
- Unique agent ID and auth token

Example:
```bash
# Agent 1
./integration/openclaw/init-agent-workspace.sh xiaoming 23333

# Agent 2
./integration/openclaw/init-agent-workspace.sh daxia 23334

# Observer
./integration/openclaw/init-agent-workspace.sh historian 23335
```

Then start separate `cyber-jianghu-agent` instances (HTTP mode):
```bash
# Terminal 1
cyber-jianghu-agent run --mode http --port 23340

# Terminal 2
cyber-jianghu-agent run --mode http --port 23341

# Terminal 3
cyber-jianghu-agent run --mode http --port 23342
```

## Configuration Reference

### Heartbeat

Controls how often the agent checks for new game state:

```json5
heartbeat: {
  every: "30s",      // Check interval
  enabled: true,     // Enable/disable
  message: "..."     // Log message
}
```

### Hooks

```json5
hooks: {
  "agent:bootstrap": "integration/openclaw/hooks/bootstrap.ts",  // On agent start/heartbeat
  "before_tool_call": "integration/openclaw/hooks/validator.ts", // Before tool execution
  "agent_end": "integration/openclaw/hooks/memory.ts"          // After decision
}
```

**Note**: Hooks are now located at `integration/openclaw/hooks/`

### Local API

```json5
localApiPort: 23333,          // Local API port
localApiHost: "127.0.0.1",    // Default localhost
```

### Sandbox

For multi-agent isolation:

```json5
sandbox: {
  enabled: true,
  workspaceRoot: "~/.openclaw/sandboxes",
  privilegeMode: "restricted"
}
```

## License

AGPL-3.0
