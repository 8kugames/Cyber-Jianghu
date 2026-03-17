---
name: cyber-jianghu-bootstrap
description: "Bootstrap hook for Cyber-Jianghu agent - fetches WorldState and generates CONTEXT.md"
homepage: https://github.com/8kugames/Cyber-Jianghu
metadata:
  openclaw:
    emoji: "⚔️"
    events: ["agent:bootstrap"]
    requires:
      bins: ["cyber-jianghu-agent"]
---

# Cyber-Jianghu Bootstrap Hook

This hook runs when the agent starts or on cron/tick.

## What It Does

1. Connects to the Local HTTP API provided by crates/agent (headless mode)
2. Fetches the formatted context (Markdown)
3. Adds decision hints for the LLM
4. Writes to CONTEXT.md in the workspace

## Architecture

```
OpenClaw Gateway (Brain)
       |
       | HTTP (fetch)
       v
crates/agent (Body)
  - HTTP API: http://127.0.0.1:23340~23349
  - GET /api/v1/context - Formatted context
       |
       | WebSocket
       v
jianghu-server (World)
  - Tick Engine (20 TPS)
```

## Port Discovery

The hook automatically discovers the agent HTTP API port in the range 23340-23349.

## Configuration

Add to your OpenClaw agent configuration:

```json
{
  "hooks": {
    "internal": {
      "entries": {
        "cyber-jianghu-bootstrap": {
          "enabled": true
        }
      }
    }
  }
}
```
