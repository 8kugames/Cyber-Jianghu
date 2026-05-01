# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**Cyber-Jianghu (赛博江湖)** is an AI-driven MMO-MAS (Massive Multiplayer Online Multi-Agent Simulation) martial arts sandbox. Every character is an autonomous AI agent with independent personality, memory, and goals. No scripts, no NPCs — only harsh physics and survival pressure. Characters hunger, fight, form alliances, and hold grudges — all emergent from thousands of AI agents.

### Core Philosophy: Body-Mind Separation (身心分离)

- **Server ("天道" / Physics Engine)**: Objective world state, authoritative game logic, data-driven rules via YAML hot-reload
- **Agent ("众生" / Consciousness)**: Subjective AI decision-making with unified cognitive architecture — only LLM location differs (Cognitive built-in vs Claw external via OpenClaw)
- **"天道无为，万物自化"**: The server provides objective physics; agents create emergent behavior through autonomous decisions

### Key Features

- **Three-Soul Architecture**: ActorSoul (action) → Earth Soul (tool calling) → ReflectorSoul (guardian/validation)
- **Multi-Intent Pipeline**: Single tick can submit multiple Intents, executed in order with rollback on failure
- **Survival-Driven Emergence**: Hunger, resource scarcity, permanent death — pressure drives complex social structures
- **Device-Character Separation**: Supports rebirth, one device manages multiple characters
- **Built-in Admin Web Panel**: Character creation, state inspection, dream injection, and more

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

# Run agent in Cognitive mode (default, built-in LLM)
cyber-jianghu-agent run

# Run agent in Claw mode (external LLM via OpenClaw)
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
└── agent/           # Agent SDK (unified cognitive architecture, two runtime modes)

docs/WHITEPAPER/     # Whitepapers
scripts/             # Utility scripts
integration/openclaw # OpenClaw plugin integration
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

**Multi-Intent Pipeline**:
- Single tick can submit multiple Intents, executed in order
- `IntentBatchConfig`: `max_intents_per_tick`, `max_retries`, `pipeline_execution_enabled`
- `GradedValidationConfig`: Always (force)/Adaptive (dynamic)/Skip (skip) three strategies
- Failed Intent triggers Saga rollback

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

The agent crate implements a **unified Agent SDK** with cognitive engine, memory, persona, and two runtime modes. Both modes share identical initialization and core architecture — the **only difference** is the LLM client implementation.

> **CRITICAL: WebSocket is REQUIRED for intent submission**
>
> `POST /api/v1/intent` is **not implemented** (route absent). All intent submission goes through WebSocket.

#### Runtime Modes

| | Cognitive (default) | Claw |
|---|---|---|
| LLM Client | `FallbackLlmClient` (built-in) | `OpenClawBridge` (external OpenClaw) |
| CognitiveEngine | DirectLlmClient | OpenClawBridge |
| Init | Unified (Phase 1: LLM, Phase 2: shared) | Same |
| OutcomeMemory | Yes | Yes |
| ChaosGenerator | Yes | Yes |
| Three-Soul | Yes | Yes |
| Callbacks | Yes | Yes (+ downstream forwarding) |

#### Three-Soul Architecture (shared by both modes)

```
ActorSoul (人魂) → ReflectorSoul (天魂)
  直连 WorldState    三层审查
  地魂 tool calling 工具池（独立模块）
```

- **ActorSoul** (人魂/行动之魂): 直连 WorldState, outputs structured Intent with precise IDs + CognitiveChain, driven by CognitiveEngine (four-stage: Perception→Motivation→Planning→Decision)
- **地魂** (能力之魂): tool calling 工具池，行动落地层（独立模块 `soul/earth/`）
- **ReflectorSoul** (天魂/守护之魂): 三层审查 — Layer 1 (action_type validation) → Layer 2 (RuleEngine validation) → Layer 3 (LLM intent review). Rejection feedback is narrative-化, ActorSoul only sees natural language

#### Decision Context Pipeline

`lifecycle.rs` assembles complete decision context each tick:

1. Memory context (three-tier memory + survival warnings + sanity + deferred dialogue + dream)
2. Summary context (action history sliding window)
3. Outcome context (action result learning from OutcomeMemory)
4. Action context (descriptions + field schema from prompt cache)

This context is written to `DecisionContextSnapshot` and exposed via `/api/v1/context` enrichment for both modes.

#### Memory System (Three-Tier Architecture)

- **Working Memory**: Short-term context, recent events
- **Episodic Memory**: Event-based memories with timestamps (SQLite)
- **Semantic Memory**: Vector-based knowledge store using HNSW indexing (instant-distance)
- **Outcome Memory (Hermes)**: SQLite action result learning

#### Key Agent Modules

- `src/core/lifecycle.rs` - Main decision loop (orchestrator), context assembly, snapshot write
- `src/core/agent.rs` - Agent struct with all component references
- `src/core/builder.rs` - AgentBuilder (fluent API)
- `src/core/reflector_ext.rs` - ReflectorSoul three-layer validation + graded audit
- `src/core/social.rs` - Social event processing + LLM favorability evaluation
- `src/soul/actor/engine.rs` - CognitiveEngine (four-stage: Perception→Motivation→Planning→Decision)
- `src/soul/actor/chain.rs` - CognitiveChain (causal reasoning trace)
- `src/soul/actor/translation.rs` - Chinese LLM boundary translation (aliases → canonical)
- `src/soul/actor/chaos.rs` - Sanity chaos generator (low-sanity random behavior)
- `src/soul/actor/prompt_template.rs` - YAML-driven prompt template loader
- `src/soul/actor/prompt_cache.rs` - Prompt cache (persona + actions)
- `src/soul/actor/summary_window.rs` - Sliding context window for action history
- `src/soul/reflector/` - ReflectorSoul: three-layer validation (single entry point)
- `src/soul/earth/` - EarthSoul: tool calling 工具池，行动落地层
- `src/component/memory/` - Three-tier memory system with SQLite backends
- `src/component/memory/outcome.rs` - Outcome Memory (Hermes): action result learning
- `src/component/persona/` - Dynamic persona, trait evolution (lifespan is server-authoritative)
- `src/component/llm/` - LLM client abstraction (`DirectLlmClient` + `FallbackLlmClient` + `OpenClawBridge`)
- `src/component/social/` - RelationshipStore (SQLite, social graph)
- `src/component/immediate/` - ImmediateEventHandler (instant event processing)
- `src/infra/transport/` - WebSocket communication layer
- `src/infra/api/` - HTTP API server: handlers, context generation, services

### Protocol Layer

The `protocol` crate defines all shared types:
- `ServerMessage` - Server → Agents (registered, world_state, game_rules_update, agent_died)
- `ClientMessage` - Agents → Server (intent, dialogue)
- `WorldState` - Complete world snapshot sent each tick
- `Intent` - Agent decision structure

### Data-Driven Design

All game mechanics configured via YAML in `crates/server/config/` (JSON fallback):
- `actions.yaml`, `attributes.yaml`, `items.yaml`, `locations.yaml`
- `game_rules.yaml`, `time.yaml`, `inventory.yaml`, `recipes.yaml`
- `skills/` — AI Procedural Skills (SKILL.md behavioral instructions, see below)

**AI Procedural Skills**: Skills are SKILL.md files (YAML frontmatter + markdown body) that define behavioral instructions injected into the Agent's LLM context. Not RPG numerical attributes. Path: `config/skills/{category}/{skill_id}/SKILL.md`. Agent reads SKILL.md from `$CYBER_JIANGHU_CONFIG_DIR/skills/` at prompt-build time. Skill acquisition: Agent issues `practice` action → `StateChange::SkillLearned` → `SkillMutator` appends to `AgentState.skills` (JSONB `_skills` key).

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
8. **Write paths restricted**: Only use relative paths under `./` for all write operations, including tmp files (`./tmp`)
9. 本项目归属"Cyber-Jianghu-MMO-MAS"，不是"Cyber-Jianghu-MOO-MAS"

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
| World-building rules | `crates/server/config/world_building_rules.yaml` |
| Skill definitions | `crates/server/config/skills/{category}/{skill_id}/SKILL.md` |
| Prompt templates (agent) | `crates/server/config/prompt_templates.yaml` |
| Database migrations | `crates/server/migrations/*.sql` |
| Docker stack | `docker-compose.yml`, `docker-compose.prod.yml` |

## API Endpoints

### Server (port 23333)
- `GET /health` - Health check
- `POST /api/v1/agent/connect` - Connect device
- `POST /api/v1/agent/register` - Register new agent (returns `narrative_config`)
- `POST /api/v1/agent/rebirth` - Delete agent
- `POST /api/v1/agent/auto-rebirth` - Auto rebirth
- `GET /api/v1/agent/{id}/context` - Get agent context
- `POST /api/v1/agent/biography` - Receive biography from agent (body: `{agent_id, biography}`)
- `WS /ws?token={auth_token}` - WebSocket connection
- Admin API: `/api/admin/reload-config`, `/api/admin/login`, `/api/admin/logout`, `/api/admin/session`

### Agent HTTP API (port 23340-23999, auxiliary to WebSocket)

**Core** (WebSocket primary, HTTP auxiliary):
- `GET /api/v1/state` - Get current WorldState
- `GET /api/v1/context` - Get narrative context + DecisionContextSnapshot enrichment

**Character**:
- `GET /api/v1/character` - Get character info
- `GET /api/v1/character/soul-cycles` - Get soul cycle records (paginated)
- `GET /api/v1/character/dream/records` - Get dream records
- `POST /api/v1/character/dream` - Inject dream (consumed by lifecycle, peeked by context handler)
- `POST /api/v1/character/rebirth` - Rebirth character

**Attributes & Status**:
- `GET /api/v1/attributes` - Get attribute values
- `GET /api/v1/tick` - Get tick status
- `GET /api/v1/lifespan` - Get lifespan status
- `GET /api/v1/cognitive` - Get structured cognitive context

**Relationships & Memory**:
- `GET /api/v1/relationship/list` - Get all relationships
- `GET /api/v1/memory/recent` - Get recent memories
- `POST /api/v1/memory/search` - Search memories

**Characters (Multi-character, 设备与角色分离)**:
- `GET /api/v1/characters` - List all characters
- `POST /api/v1/characters/switch` - Switch current character
- `GET /api/v1/characters/{agent_id}` - Get character by ID

**Biography**:
- `GET /api/v1/character/biography` - Get cached biography (query: `agent_id`)
- `POST /api/v1/character/biography` - Generate biography from soul cycles + daily summaries (query: `agent_id`)

**Validation & Review**:
- `POST /api/v1/validate` - Validate intent
- `GET /api/v1/review/pending` - Get pending reviews
- `POST /api/v1/review/{intent_id}` - Submit review
- `GET /api/v1/review/{intent_id}/status` - Get review status

**Events & Config**:
- `GET /api/v1/events` - Death events SSE stream
- `GET/POST /api/v1/config/llm-disabled` - LLM disable toggle
- `GET/POST /api/v1/config/auto-rebirth` - Auto-rebirth toggle
- `POST /api/v1/config/reload` - Hot reload config
- `GET /api/v1/setup/status` - Get setup status
- `GET /api/v1/config/llm/providers` - Get LLM providers
- `GET /api/v1/config/llm/usage` - Get LLM token usage

### Admin Web Panel
- `GET /admin/` - Main dashboard
- `GET /admin/{*path}` - Admin panel routes (served from `crates/server/src/admin/`)

## Quick Start Guides

- [QuickStart-Server.md](crates/server/QuickStart-Server.md) - Server development
- [QuickStart-Agent.md](crates/agent/QuickStart-Agent.md) - Agent development
- [Architecture docs](crates/server/docs/architecture/) and [Agent docs](crates/agent/docs/architecture/)
