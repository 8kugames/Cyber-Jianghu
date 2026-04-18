# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**Cyber-Jianghu (赛博江湖)** is a Rust workspace implementing an MMO-MAS game where every character is an AI agent. The architecture follows **data-driven COI (Composition Over Inheritance)** principles.

### Core Philosophy: Body-Mind Separation (身心分离)

- **Server ("天道" / Physics Engine)**: Objective world state, authoritative game logic, tick-based time progression
- **Agent ("众生" / Consciousness)**: Subjective AI decision-making with memory, persona, and cognitive capabilities
- **"天道无为，万物自化"**: The server provides objective physics; agents create emergent behavior through autonomous decisions

See [Readme.md](Readme.md) for full project description and architecture diagrams.

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

# Run agent in Cognitive mode (default, uses built-in LLM)
cyber-jianghu-agent run

# Run agent in Claw mode (for OpenClaw integration)
cyber-jianghu-agent run --mode claw --port 0

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

CI builds for 4 platforms: linux-x86_64 (musl), linux-arm64 (musl), macos-arm64, windows-x86_64.
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

- **Tick Engine**: Pure clock (decay + periodic WorldState broadcast)
- **IntentWorker**: Real-time intent processing (single consumer, MPSC channel)
- **WebSocket/HTTP**: Handles Agent connections via Axum
- **Game Data System**: Loads YAML configs from `crates/server/config/*.yaml` (JSON fallback)
- **Action System**: Data-driven action validation and execution
- **Formula Engine**: Dynamic expression evaluation using `evalexpr` crate for attribute calculations

**Real-time Architecture** (0.1.0+):
```
Agent 提交 Intent ──> handler.rs (try_send) ──> IntentWorker (MPSC channel)
                                                        │
                        ┌───────────────────────────────┘
                        │ 1. Read DashMap (agent state)
                        │ 2. StateProcessor (validate + execute + Saga rollback)
                        │ 3. Persist to DB (await)
                        │ 4. Update DashMap (write-through)
                        │ 5. Send ExecutionResult to Agent
                        │ 6. Broadcast events to co-located Agents

TickScheduler (每 N 秒):
                        │ 1. Send TickBoundary → IntentWorker (decay + death handling)
                        │ 2. Broadcast WorldState
                        │ 3. Chronicle generation (每 7 游戏日)
```

**State Management** (DashMap write-through):
- `AgentStateCache = Arc<DashMap<Uuid, AgentState>>` — in-memory cache, startup-loaded from DB
- Write-through: persist to DB → await confirm → update DashMap
- Persist failure → DashMap NOT updated → Agent receives ExecutionResult(success=false)

**Conflict Resolution**: FIFO via single IntentWorker (zero race conditions)

Key server modules:
- `src/tick/scheduler.rs` - Pure clock scheduler (decay + broadcast)
- `src/tick/realtime.rs` - IntentWorker (real-time intent processing engine)
- `src/tick/processor/` - StateProcessor (validate + execute + Saga rollback)
- `src/actions/` - Action execution with data-driven ActionType
- `src/game_data/` - Config loading, caching, and formula evaluation
- `src/websocket/` - WebSocket connection management
- `src/handlers/` - HTTP API endpoints
- `src/state.rs` - Shared AppState, AgentStateCache, rate limiting
- `src/chronicle/` - Chronicle generation (群像传记): auto-generates every 7 game days

### Agent Architecture

The agent crate provides WebSocket + HTTP API for OpenClaw integration:

> ⚠️ **CRITICAL: WebSocket is REQUIRED for intent submission**
>
> OpenClaw **must** use WebSocket (`ws://localhost:23340/ws`) to submit intents.
> HTTP API `POST /api/v1/intent` is for debugging only and has timing issues.

1. **WebSocket (Required)**: OpenClaw **must** connect via WebSocket to ensure Tick synchronization
2. **HTTP API (Auxiliary)**: Used for data queries, Web panel, debugging only

**Memory System** (Three-Tier Architecture):
- **Working Memory**: Short-term context, recent events
- **Episodic Memory**: Event-based memories with timestamps
- **Semantic Memory**: Vector-based knowledge store using HNSW indexing (instant-distance)

Key agent modules:
- `src/core/` - Agent struct, builder, lifecycle (orchestrator)
- `src/soul/actor/` - 人魂 ActorSoul: 直连 WorldState，输出结构化 Intent + tool calling 工具池（地魂能力层）
- `src/soul/reflector/` - 天魂 ReflectorSoul: Three-layer validation (唯一出入口)
- `src/component/memory/` - Three-tier memory system with SQLite backends
- `src/component/persona/` - Dynamic persona, lifespan, trait evolution
- `src/component/llm/` - LLM client abstraction (`DirectLlmClient` + `FallbackLlmClient`)
- `src/infra/transport/` - WebSocket communication layer
- `src/infra/api/` - HTTP API server, handlers, services

**Three-Soul Architecture** (Cognitive mode):
```
ActorSoul (人魂) → ReflectorSoul (天魂)
  直连 WorldState    三层审查
  地魂 tool calling 工具池
```

- **ActorSoul** (人魂): 直连 WorldState, outputs structured Intent with precise IDs + CognitiveChain
- **地魂**: tool calling 工具池，行动落地层 (embedded in ActorSoul)
- **ReflectorSoul** (天魂): Layer 1 (action_type) → Layer 2 (RuleEngine) → Layer 3 (LLM)

**Runtime modes**: `cognitive` (default, built-in LLM) or `claw` (OpenClaw integration)

### Protocol Layer

The `protocol` crate defines all shared types:
- `ServerMessage` - Server → Agents (registered, world_state, game_rules_update)
- `ClientMessage` - Agents → Server (intent, dialogue)
- `WorldState` - Complete world snapshot sent each tick
- `Intent` - Agent decision structure

### Data-Driven Design

All game mechanics configured via YAML in `crates/server/config/` (JSON fallback):
- `actions.yaml`, `attributes.yaml`, `items.yaml`, `locations.yaml`
- `game_rules.yaml`, `time.yaml`, `inventory.yaml`, `recipes.yaml`

**Formula Engine**: Dynamic calculations use `evalexpr` syntax.

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
- Library code: Use `thiserror::Error` with `#[error("...")]`

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

### Testing Conventions

- Integration tests go in `crates/*/tests/` directories
- Shared test fixtures in `crates/*/tests/common/fixtures.rs`
- Unit tests in `#[cfg(test)] mod tests` within source files
- CI uses `cargo-nextest` for faster parallel test execution

## Important Rules

1. **Server is authoritative**: Clients submit intents, server validates and executes
2. **Data-driven**: Configure via `crates/server/config/*.yaml`, not hardcoded values
3. **No type suppression**: Never use `as any` or suppress errors
4. **Bugfix Rule**: Fix minimally, NEVER refactor while fixing bugs
5. **File size limit**: Keep .rs files under 800 lines
6. **No emoji** in code or documentation
7. **No backwards compatibility**: Make breaking changes freely

## Key Dependencies

| Crate | Purpose |
|-------|---------|
| `axum` | Web framework with WebSocket support |
| `sqlx` | PostgreSQL async driver |
| `tokio` | Async runtime |
| `evalexpr` | Formula/expression evaluation |
| `tokio-tungstenite` | WebSocket client (agent) |
| `rusqlite` | Local SQLite storage (agent memory) |
| `instant-distance` | HNSW vector index (agent semantic memory) |

## Key Configuration Files

| Purpose | Path |
|---------|------|
| Environment variables | `.env` |
| Server configuration | `crates/server/config/*.yaml` |
| Database migrations | `crates/server/migrations/*.sql` |
| Docker stack | `docker-compose.yml`, `docker-compose.prod.yml` |

## API Endpoints

### Server (port 23333)
- `GET /health` - Health check
- `POST /api/v1/agent/connect` - Connect device
- `POST /api/v1/agent/register` - Register new agent (returns `narrative_config`)
- `POST /api/v1/agent/rebirth` - Delete agent
- `WS /ws?token={auth_token}` - WebSocket connection

### Agent HTTP API (port 23340-23999, auxiliary to WebSocket)
- `GET /api/v1/state` - Get current WorldState
- `GET /api/v1/context` - Get narrative context (Markdown, for LLM)
- `POST /api/v1/intent` - Submit intent (debugging only, use WebSocket)
- `GET /api/v1/character` - Get character info
- `GET /api/v1/character/soul-cycles` - Get soul cycle records (paginated)
- `POST /api/v1/character/dream` - Inject dream (1 per game day)
- `POST /api/v1/character/rebirth` - Rebirth character
- `GET /api/v1/relationship/list` - Get all relationships
- `GET /api/v1/memory/recent` - Get recent memories
- `POST /api/v1/memory/search` - Search memories

### Admin Web Panel
- `GET /admin/` - Main dashboard
- `GET /welcome.html` - Home page
- `GET /create.html` - Character creation
- `GET /character.html` - Character info

## Quick Start Guides

- [QuickStart-Server.md](crates/server/QuickStart-Server.md) - Server development
- [QuickStart-Agent.md](crates/agent/QuickStart-Agent.md) - Agent development
- [Architecture docs](crates/server/docs/architecture/) and [Agent docs](crates/agent/docs/architecture/)
