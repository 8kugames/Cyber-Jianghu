# Agent Quick Start Guide

> **中文版本**: [QuickStart-Agent.md](./QuickStart-Agent.md)

This guide helps developers quickly deploy and run the Agent SDK.

## Prerequisites

Since the Agent is the bridge to the server, **the server must already be running** (see `crates/server/QuickStart-Server.en.md`).

## Run Modes and Startup

The agent's core logic (cognitive flow, three-tier memory, multi-persona) runs on the same architecture. The only difference is where the LLM call happens:

| Mode | Description | LLM Client | Startup Command |
|------|-------------|-----------|-----------------|
| **Cognitive** (default) | Fully autonomous. The Agent calls the LLM internally and generates Intents in a closed loop. | `FallbackLlmClient` | `cyber-jianghu-agent run` |
| **Claw** | External brain. The Agent bridges an external LLM via OpenClaw; the Agent itself only provides context, while the decision is injected by OpenClaw. | `OpenClawBridge` | `cyber-jianghu-agent run --mode claw` |

## Installation and Deployment

### 1. Local development (CLI)

```bash
# Install the CLI from source
cargo install --path crates/agent

# Start in default mode
cyber-jianghu-agent run

# Or specify a port
CYBER_JIANGHU_PORT=23340 cyber-jianghu-agent run
```

### 2. Docker deployment

```bash
cd crates/agent

# Configure environment variables
cp .env.example .env

# Start the container
docker compose up -d

# View logs
docker compose logs -f agent
```

### 3. One-click scripts

```bash
./install.sh agent start        # Start the Agent
./install.sh agent logs         # Tail logs in real time
./install.sh agent stop         # Stop the service
./install.sh agent reset        # Reset all local data
```

## Network and Ports

The Agent starts a local HTTP API service for the dashboard, state queries, and communication with OpenClaw (in Claw mode).

- **Default port range**: `23340-23999`
- **Specify a port**: set the environment variable `CYBER_JIANGHU_PORT=23340`. If set to `0` or unset, the Agent auto-allocates an available port in the range.
- **Server connection**: ensure `CYBER_JIANGHU_SERVER_WS_URL` correctly points to the server's WebSocket endpoint (e.g. `ws://localhost:23333/ws`).

## Multi-Agent Deployment (Device-Character Separation)

The Agent SDK supports hosting multiple characters on the same device (process). It also supports starting multiple Agent processes mapped to different ports.

```yaml
# docker-compose.multi.yml example
services:
  agent-linghu:
    extends:
      file: docker-compose.yml
      service: agent
    container_name: cyber-jianghu-agent-linghu
    environment:
      CYBER_JIANGHU_PORT: 23340
    ports:
      - "23340:23340"

  agent-renwoxing:
    extends:
      file: docker-compose.yml
      service: agent
    container_name: cyber-jianghu-agent-renwoxing
    environment:
      CYBER_JIANGHU_PORT: 23341
    ports:
      - "23341:23341"
```

## Core Workflow: How to Interact

Internally, the Agent **must use WebSocket** to submit Intents to the Server. HTTP API is only auxiliary.

1. **Device registration**: On first launch, a UUID v4 is auto-generated as the Device ID and registered with the Server.
2. **Character creation**:
   ```bash
   curl -X POST http://localhost:23340/api/v1/character/register \
     -H "Content-Type: application/json" \
     -d '{
       "name": "Linghu Chong",
       "gender": "male",
       "age": 24,
       "system_prompt": "You are the senior disciple of the Huashan Sect, open-minded by nature..."
     }'
   ```
3. **Tick loop**: The server pushes a `WorldState` every N seconds. The Agent receives it, triggers the `CognitiveEngine` (Human Soul), and uses the LLM to generate an Intent.
4. **Validation and execution**: The generated Intent is first reviewed by `ReflectorSoul` (Heaven Soul) through a three-layer rule check, and then submitted to the server's `IntentWorker`.

## Configuration Management (Agent.yaml)

The Agent supports a multi-level LLM fallback configuration (`FallbackLlmClient`). You can edit it in `~/.cyber-jianghu/agent.yaml`:

```yaml
llm:
  provider: ollama
  model: qwen2.5:14b
  # Fallback for resilience: automatically degrades when the primary model returns 403/429/timeout
  fallback_models:
    - qwen2.5:7b
    - qwen2.5:3b
```

You can also edit and hot-reload it directly from the Agent's built-in web panel (`http://localhost:23340/settings.html`).
