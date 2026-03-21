# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**Cyber-Jianghu (赛博江湖)** is a Rust workspace implementing an MMO-MAS game where every character is an AI agent. The architecture follows **data-driven COI (Composition Over Inheritance)** principles.

## Common Commands

### Development

```bash
# Start development environment (Docker)
./scripts/cyber-jianghu.sh start

# Start production environment
./scripts/cyber-jianghu.sh start --prod

# Build server (debug)
cargo build -p cyber-jianghu-server

# Build server (release)
cargo build -p cyber-jianghu-server --release

# Build agent
cargo build -p cyber-jianghu-agent

# Run tests
cargo test --workspace

# Run single test
cargo test -p cyber-jianghu-server test_name

# Build agent with cargo install
cargo install --path crates/agent

# Run agent in Claw mode (for OpenClaw integration)
cyber-jianghu-agent run --mode claw --port 23340
```

### Service Management

```bash
# View status
./scripts/cyber-jianghu.sh status

# View logs
./scripts/cyber-jianghu.sh logs

# Stop services
./scripts/cyber-jianghu.sh stop

# Reset all data (destructive)
./scripts/cyber-jianghu.sh reset
```

### Database

```bash
# Connect to PostgreSQL
docker compose exec db psql -U cyberjianghu -d cyberjianghu

# Run migrations (handled automatically on startup)
# Migration files: crates/server/migrations/*.sql
```

## Architecture

### Workspace Structure

```
crates/
├── protocol/        # Communication protocol (ServerMessage, ClientMessage, WorldState)
├── server/          # Game server ("天道" - physics engine)
└── agent/           # Agent SDK with HTTP API for OpenClaw integration

integration/
└── openclaw/        # OpenClaw hooks and templates
    ├── hooks/       # TypeScript hooks (bootstrap, validator, memory)
    ├── tools/       # TypeScript tools (jianghu_act action execution)
    ├── plugins/     # OpenClaw plugins (memory integration)
    └── skills/      # OpenClaw skill definitions
```

### Server Architecture

The server is the authoritative "physics engine" of the world:

- **Tick Engine**: Runs at configurable TPS, collecting and executing Agent intents
- **WebSocket/HTTP**: Handles Agent connections via Axum
- **Game Data System**: Loads YAML configs from `crates/server/config/*.yaml` (JSON fallback)
- **Action System**: Data-driven action validation and execution

Key server modules:
- `src/tick/` - Tick loop and intent processing
- `src/actions/` - Action execution with data-driven ActionType
- `src/game_data/` - Config loading and caching
- `src/websocket/` - WebSocket connection management
- `src/handlers/` - HTTP API endpoints

### Agent Architecture

The agent crate provides two integration modes:

1. **HTTP Mode** (recommended for OpenClaw):
   - Runs headless with HTTP API on port 23340-23349
   - OpenClaw communicates via `fetch()` calls
   - No FFI compilation needed

2. **Cognitive Mode** (built-in AI):
   - Multi-stage cognitive pipeline: perception → motivation → planning → decision
   - Built-in memory systems (working, episodic, semantic)

Key agent modules:
- `src/core/` - WebSocket client to game server
- `src/runtime/decision/` - Decision modes (claw / cognitive)
- `src/ai/` - AI components:
  - `cognitive/` - Narrative engine for attribute descriptions
  - `memory/` - Working, episodic, semantic memory with SQLite backends
  - `relationship/` - Relationship store with AI narrative descriptions
  - `persona/` - Dynamic persona with trait evolution
  - `validator/` - Intent validation against persona
  - `lifespan/` - Age and aging effects calculation

### Protocol Layer

The `protocol` crate defines all shared types:
- `ServerMessage` - Messages from server to agents (registered, world_state, game_rules_update, dialogue)
- `ClientMessage` - Messages from agents to server (intent, dialogue)
- `WorldState` - Complete world snapshot sent each tick
- `Intent` - Agent decision structure
- `NarrativeConfig` - Attribute threshold descriptions (shared between server and agent)

### Data-Driven Design

All game mechanics are configured via YAML files in `crates/server/config/` (JSON fallback supported):
- `actions.yaml` - Action definitions with parameters and validation rules
- `attributes.yaml` - Attribute definitions (primary, status, derived)
- `items.yaml` - Item definitions with properties
- `locations.yaml` - Location graph with nodes and edges
- `narrative_config.yaml` - Threshold-based attribute descriptions
- `recipes.yaml` - Crafting recipes
- `game_rules.yaml` - Core game rules (real time → tick conversion)
- `time.yaml` - Time system (tick → game time conversion)
- `inventory.yaml` - Inventory limits
- `initial_inventory.yaml` - Starting items for new agents
- `network.yaml` - WebSocket and network settings
- `world-building-rules.yaml` - World setting constraints

## Code Style Conventions

### Import Organization

```rust
// External crates first
use anyhow::{Context, Result};
use std::sync::Arc;
use tokio::sync::RwLock;

// Internal modules second
use crate::config::Config;
use crate::db::DbPool;

// Parent module last
use super::builder::AgentBuilder;
```

### Naming Conventions

| Element | Convention | Example |
|---------|------------|---------|
| Structs/Enums | `PascalCase` | `TickScheduler`, `ActionType` |
| Functions/Variables | `snake_case` | `execute_tick()`, `agent_states` |
| Constants | `SCREAMING_SNAKE_CASE` | `MAX_RETRY_ATTEMPTS` |
| Type aliases | `PascalCase` | `GameRulesCallback` |

### Error Handling

- Application code: Use `anyhow::Result` with `.context("中文错误信息")?`
- Library code: Use `thiserror::Error` with `#[error("...")]` attributes

### Async Patterns

- Shared state: `Arc<RwLock<T>>` with `.read().await` / `.write().await`
- Use `#[async_trait]` for async traits

### Serde Patterns

```rust
#[serde(rename_all = "lowercase")]  // for enums
#[serde(skip_serializing_if = "Option::is_none")]  // optional fields
```

## Important Rules

1. **Server is authoritative**: Clients submit intents, server validates and executes
2. **Data-driven**: Configure via `crates/server/config/*.yaml`, not hardcoded values
3. **No type suppression**: Never use `as any` or suppress errors
4. **Bugfix Rule**: Fix minimally, NEVER refactor while fixing bugs
5. **File size limit**: Keep .rs files under 500 lines
6. **No emoji** in code or documentation

## Key Configuration Files

| Purpose | Path |
|---------|------|
| Environment variables | `.env` |
| Server configuration | `crates/server/config/*.yaml` |
| Database migrations | `crates/server/migrations/*.sql` |
| Docker stack | `docker-compose.yml`, `docker-compose.prod.yml` |
| OpenClaw integration | [8kugames/Cyber-Jianghu-Openclaw](https://github.com/8kugames/Cyber-Jianghu-Openclaw) |

## API Endpoints

### Server (port 23333)

- `GET /health` - Health check
- `POST /api/v1/agent/connect` - Connect device (register/get auth token)
- `POST /api/v1/agent/register` - Register new agent (returns `narrative_config`)
- `POST /api/v1/agent/rebirth` - Delete agent (CASCADE delete states/inventory)
- `GET /api/dashboard/stats` - Dashboard statistics (requires admin token)
- `GET /api/config` - List configurations
- `WS /ws?token={auth_token}` - WebSocket connection

### Agent HTTP API (port 23340-23349 in HTTP mode)

- `GET /api/v1` - API discovery endpoint (returns all available APIs with examples)
- `GET /api/v1/health` - Health check
- `GET /api/v1/state` - Get current WorldState
- `GET /api/v1/context` - Get narrative context (Markdown format, recommended for LLM)
- `GET /api/v1/attributes` - Dream glimpse: get attribute values (forbidden to store in memory)
- `POST /api/v1/intent` - Submit intent to game server
- `POST /api/v1/validate` - Validate action before submission
- `GET /api/v1/relationship/list` - Get all relationships
- `GET /api/v1/relationship/{id}` - Get specific relationship
- `POST /api/v1/relationship` - Update relationship
- `GET /api/v1/lifespan` - Get lifespan status
- `GET /api/v1/memory/recent` - Get recent memories
- `POST /api/v1/memory/search` - Search memories
- `POST /api/v1/memory` - Store memory

#### Character Management (Web Panel)

- `GET /api/v1/character` - Get character info (name, age, gender, status, registered_at, birth_attributes, attributes, inventory)
- `GET /api/v1/character/experiences?page=1&limit=20` - Get experience logs (paginated, with intent_summary and observer_thought)
- `GET /api/v1/character/dream` - Get dream status (thought, remaining_ticks, can_use_today)
- `POST /api/v1/character/dream` - Inject dream (limited to 1 per game day)
- `POST /api/v1/character/rebirth` - Rebirth (delete character, redirect to creation)
- `POST /api/v1/character/register` - Register new character (forward to server)

### Agent Web Panel

- `GET /` or `GET /index.html` - Character creation page
- `GET /character.html` - Character info page (displays status, registered_at, intent_summary, observer_thought)
- `GET /manage.html` - Management page (dream injection, rebirth)

## Narrative Config Delivery

The `narrative_config` is delivered from server to agent during registration:

1. Server loads `narrative_config.yaml` on startup via `GameDataLoader`
2. Agent registration response includes `narrative_config` field
3. Agent stores config to `~/.cyber-jianghu/config/narrative_config.yaml`
4. Agent's `NarrativeEngine` loads from local config directory

This ensures agent can function in production without accessing server's development files.

## Development Notes

- The project uses PostgreSQL for persistence
- All game mechanics are configurable via YAML files (JSON fallback)
- WebSocket is used for real-time communication (use WSS in production)
- The tick system drives game time forward
- Server Admin dashboard is available at `http://localhost:23333/admin` (requires token from logs or .env)
