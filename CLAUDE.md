# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**Cyber-Jianghu (赛博江湖)** is a Rust workspace implementing an MMO-MAS game where every character is an AI agent. The architecture follows **data-driven COI (Composition Over Inheritance)** principles.

### Core Philosophy: Body-Mind Separation (身心分离)

- **Server ("天道" / Physics Engine)**: Objective world state, authoritative game logic, tick-based time progression
- **Agent ("众生" / Consciousness)**: Subjective AI decision-making with memory, persona, and cognitive capabilities
- **"天道无为，万物自化"**: The server provides objective physics; agents create emergent behavior through autonomous decisions

## Common Commands

### Development

```bash
# Start development environment (Docker)
./install.sh all start

# Start production environment
./install.sh all start --prod

# Build server (debug)
cargo build -p cyber-jianghu-server

# Build server (release)
cargo build -p cyber-jianghu-server --release

# Build agent
cargo build -p cyber-jianghu-agent

# Run tests (CI uses nextest)
cargo test --workspace

# Run tests with nextest (faster, used in CI)
cargo nextest run --workspace

# Run single test
cargo test -p cyber-jianghu-server test_name

# Format check (CI enforces this)
cargo fmt --check

# Run clippy linter (CI treats warnings as errors)
cargo clippy --workspace --all-targets -- -D warnings

# Run clippy with auto-fix
cargo clippy --workspace --all-targets --fix --allow-dirty

# Build agent with cargo install
cargo install --path crates/agent

# Run agent in Cognitive mode (default, uses built-in LLM)
cyber-jianghu-agent run

# Run agent in Claw mode (for OpenClaw integration)
cyber-jianghu-agent run --mode claw --port 23340

# Run with debug logging
RUST_LOG=debug cargo run -p cyber-jianghu-server
```

### Service Management

```bash
# View status
./install.sh all status

# View logs
./install.sh all logs

# Stop services
./install.sh all stop

# Reset all data (destructive)
./install.sh all reset
```

### Database

```bash
# Connect to PostgreSQL
docker compose exec db psql -U cyberjianghu -d cyberjianghu

# Run migrations (handled automatically on startup)
# Migration files: crates/server/migrations/*.sql
```

### CI/CD Requirements

PR checks enforce these before merge:
1. `cargo fmt --check` - Format verification
2. `cargo clippy --all-targets -- -D warnings` - Lint with warnings as errors
3. `cargo nextest run --workspace` - All tests pass

CI builds for 5 platforms: linux-x86_64, linux-arm64, macos-arm64, windows-x86_64.
Docker images published to `ghcr.io/8kugames/cyber-jianghu-server`.

## Architecture

### Workspace Structure

```
crates/
├── protocol/        # Communication protocol (ServerMessage, ClientMessage, WorldState)
├── server/          # Game server ("天道" - physics engine)
└── agent/           # Agent SDK (WebSocket + HTTP API for OpenClaw integration)

docs/                # Architecture docs and whitepapers
scripts/             # Utility scripts
```

**OpenClaw Integration**: See separate repository [8kugames/Cyber-Jianghu-Openclaw](https://github.com/8kugames/Cyber-Jianghu-Openclaw)

### Server Architecture

The server is the authoritative "physics engine" of the world:

- **Tick Engine**: Runs at configurable TPS, collecting and executing Agent intents
- **WebSocket/HTTP**: Handles Agent connections via Axum
- **Game Data System**: Loads YAML configs from `crates/server/config/*.yaml` (JSON fallback)
- **Action System**: Data-driven action validation and execution
- **Formula Engine**: Dynamic expression evaluation using `evalexpr` crate for attribute calculations

**Tick Processing Flow** (60-second configurable cycle):
```
意图收集 --> 验证 --> 冲突解析 --> 执行 --> 状态更新 --> 衰减处理 --> 广播 --> 持久化
```
1. **Collect intents** - Only accept intents with current tick_id
2. **Validate** - Check agent alive, action legal, resources sufficient
3. **Resolve conflicts** - Priority ordering, position/resource conflicts
4. **Execute** - Apply actions in deterministic order
5. **Update state** - Apply attribute changes, generate events
6. **Decay** - Hunger, thirst, item durability
7. **Broadcast** - Push WorldState to all agents
8. **Persist** - Save to PostgreSQL

Key server modules:
- `src/tick/` - Tick loop and intent processing
- `src/actions/` - Action execution with data-driven ActionType
- `src/game_data/` - Config loading, caching, and formula evaluation
- `src/websocket/` - WebSocket connection management
- `src/handlers/` - HTTP API endpoints
- `src/state.rs` - Shared AppState and rate limiting

### Agent Architecture

The agent crate provides WebSocket + HTTP API for OpenClaw integration:

> ⚠️ **CRITICAL: WebSocket is REQUIRED for intent submission**
>
> OpenClaw **must** use WebSocket (`ws://localhost:23340/ws`) to submit intents.
> HTTP API `POST /api/v1/intent` is for debugging only and has timing issues.
>
> **Why**: Server only accepts intents with the *current* tick_id. HTTP polling
> cannot guarantee real-time tick synchronization. WebSocket provides immediate
> tick notifications.

1. **WebSocket (Required)**:
   - OpenClaw **must** connect via WebSocket to ensure Tick synchronization
   - Agent provides WebSocket server at `ws://localhost:23340/ws`
   - Real-time Tick notifications and Intent submission

2. **HTTP API (Auxiliary)**:
   - Runs with HTTP API on port 23340-23349
   - Used for data queries, Web panel, debugging
   - **NOT** a replacement for WebSocket

**Memory System** (Three-Tier Architecture):
- **Working Memory**: Short-term context, recent events
- **Episodic Memory**: Event-based memories with timestamps
- **Semantic Memory**: Vector-based knowledge store using HNSW indexing (instant-distance)
- All tiers use SQLite backends with Ebbinghaus forgetting curve implementation

Key agent modules:
- `src/core/` - WebSocket client to game server
- `src/runtime/decision/` - Decision modes (ws required, http auxiliary)
- `src/transport/` - WebSocket communication layer
- `src/ai/` - AI components:
  - `cognitive/` - Narrative engine for attribute descriptions
  - `memory/` - Three-tier memory system with SQLite backends
  - `relationship/` - Relationship store with AI narrative descriptions
  - `persona/` - Dynamic persona with trait evolution
  - `validator/` - Intent validation against persona
  - `lifespan/` - Age and aging effects calculation
  - `llm/` - LLM client for AI decision-making

**Runtime modes** (decision module):
- `cognitive` (default) - Multi-stage cognitive engine with built-in LLM for autonomous decision-making
- `claw` - WebSocket server for OpenClaw/external scheduler integration

### Protocol Layer

The `protocol` crate defines all shared types:
- `ServerMessage` - Messages from server to agents (registered, world_state, game_rules_update, dialogue)
- `ClientMessage` - Messages from agents to server (intent, dialogue)
- `WorldState` - Complete world snapshot sent each tick
- `Intent` - Agent decision structure
- `NarrativeConfig` - Attribute threshold descriptions (shared between server and agent)

**OpenClaw WebSocket Message Format** (Agent <-> OpenClaw):
```json
// Downstream: Tick notification
{"type": "tick", "tick_id": 123, "deadline_ms": 50000, "state": {...}, "context": "..."}

// Upstream: Intent submission (MUST match current tick_id)
{"type": "intent", "tick_id": 123, "action_type": "idle", "action_data": {}, "thought_log": "..."}

// Downstream: Server error
{"type": "server_error", "code": "agent_dead", "message": "...", "tick_id": 123}
```

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
- `display_messages.yaml` - UI display message templates

**Formula Engine**: Dynamic calculations use `evalexpr` syntax:
```yaml
# Example: damage calculation formula
damage: "base_damage + strength * 0.5 + weapon_bonus"
```

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

### Rust Best Practices

- **Zero-cost abstractions**: Prefer compile-time abstractions over runtime checks
- **No panic in library code**: Use explicit error handling with `Result`
- **Iterators over loops**: Use iterator methods instead of manual loops
- **Follow clippy**: Run `cargo clippy` before commits
- **Doc comments**: Include examples in documentation comments

### Testing Conventions

- Integration tests go in `crates/*/tests/` directories
- Shared test fixtures in `tests/common/fixtures.rs`
- Unit tests in `#[cfg(test)] mod tests` within source files
- CI uses `cargo-nextest` for faster parallel test execution

## Important Rules

1. **Server is authoritative**: Clients submit intents, server validates and executes
2. **Data-driven**: Configure via `crates/server/config/*.yaml`, not hardcoded values
3. **No type suppression**: Never use `as any` or suppress errors
4. **Bugfix Rule**: Fix minimally, NEVER refactor while fixing bugs
5. **File size limit**: Keep .rs files under 500 lines
6. **No emoji** in code or documentation
7. **No backwards compatibility**: This project does not need to maintain backwards compatibility - make breaking changes freely

## Key Dependencies

| Crate | Purpose |
|-------|---------|
| `axum` | Web framework with WebSocket support |
| `sqlx` | PostgreSQL async driver |
| `tokio` | Async runtime |
| `evalexpr` | Formula/expression evaluation |
| `tokio-tungstenite` | WebSocket client (agent) |
| `rusqlite` | Local SQLite storage (agent memory) |
| `candle-*` | Local ML/embeddings (agent) |
| `instant-distance` | HNSW vector index (agent semantic memory) |

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

### Agent HTTP API (port 23340-23349, auxiliary to WebSocket)

> ⚠️ **重要**: OpenClaw **必须**通过 WebSocket (`ws://localhost:23340/ws`) 提交意图，HTTP API 仅用于调试和数据查询。

- `GET /api/v1` - API discovery endpoint (returns all available APIs with examples)
- `GET /api/v1/health` - Health check
- `GET /api/v1/state` - Get current WorldState
- `GET /api/v1/context` - Get narrative context (Markdown format, recommended for LLM)
- `GET /api/v1/attributes` - Dream glimpse: get attribute values (forbidden to store in memory)
- `POST /api/v1/intent` - ⚠️ Submit intent (debugging only, use WebSocket for production)
- `POST /api/v1/validate` - Validate action before submission
- `GET /api/v1/relationship/list` - Get all relationships
- `GET /api/v1/relationship/{id}` - Get specific relationship
- `POST /api/v1/relationship` - Update relationship
- `GET /api/v1/lifespan` - Get lifespan status
- `GET /api/v1/memory/recent` - Get recent memories
- `POST /api/v1/memory/search` - Search memories
- `POST /api/v1/memory` - Store memory
- `GET /api/v1/tick` - Get tick status (for polling, returns tick_id, tick_duration_secs, last_update)

#### Character Management (Web Panel)

- `GET /api/v1/character` - Get character info (name, age, gender, status, registered_at, birth_attributes, attributes, inventory)
- `GET /api/v1/character/experiences?page=1&limit=20` - Get experience logs (paginated, with intent_summary and observer_thought)
- `GET /api/v1/character/dream` - Get dream status (thought, remaining_ticks, can_use_today)
- `POST /api/v1/character/dream` - Inject dream (limited to 1 per game day)
- `POST /api/v1/character/rebirth` - Rebirth (delete character, redirect to creation)
- `POST /api/v1/character/register` - Register new character (forward to server)

#### Review System (Observer Agent)

- `GET /api/v1/review/pending` - Get pending reviews (Observer Agent polls this endpoint)
- `POST /api/v1/review/{intent_id}` - Submit review result (approved/rejected with reason)
- `GET /api/v1/review/{intent_id}/status` - Get review status

#### Configuration Management

- `GET /api/v1/config` - Get current configuration (server URLs, runtime mode, port)
- `POST /api/v1/config/reload` - Hot reload configuration from file
- `POST /api/v1/config/server` - Set server address (triggers WebSocket reconnection)

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
- Agent config stored at `~/.cyber-jianghu/agent.yaml` (or `$CYBER_JIANGHU_CONFIG_DIR/agent.yaml`)
- Agent auto-detects server URL changes and re-registers identity

## Quick Start Guides

- Server: `crates/server/QuickStart-Server.md`
- Agent: `crates/agent/QuickStart-Agent.md`
- Architecture docs: `crates/server/docs/architecture/` and `crates/agent/docs/architecture/`
