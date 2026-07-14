# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**Cyber-Jianghu (虚境：江湖)** is an AI-driven MMO-MAS (Massive Multiplayer Online Multi-Agent Simulation) martial arts sandbox. Every character is an autonomous AI agent with independent personality, memory, and goals. No scripts, no NPCs — only harsh physics and survival pressure. Characters hunger, fight, form alliances, and hold grudges — all emergent from thousands of AI agents.

### Core Philosophy: Body-Mind Separation (身心分离)

- **Server ("天道" / Physics Engine)**: Objective world state, authoritative game logic, data-driven rules via YAML hot-reload
- **Agent ("众生" / Consciousness)**: Subjective AI decision-making with unified cognitive architecture — only LLM location differs (Cognitive built-in vs Claw external via OpenClaw)
- **"天道无为，万物自化"**: The server provides objective physics; agents create emergent behavior through autonomous decisions

### Key Features

- **Three-Soul Architecture**: ActorSoul (action, with embedded EarthSoul tool calling) → ReflectorSoul (guardian/validation). EarthSoul is not a separate pipeline step — it runs inside ActorSoul's LLM inference loop
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
# Key tables: agents, agent_states, experiences, action_evolution_proposals,
#   action_evolution_proposal_groups, soul_review_votes
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
├── embedding/       # Embedding service (local BERT inference + HTTP API, bge-small-zh-v1.5)
├ server/          # Game server ("天道" - physics engine)
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
- **Action System**: Data-driven action validation and execution (`actions.yaml` defines transmission, display_name, validators, highlights)
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
```

**State Management** (DashMap write-through):
- `AgentStateCache = Arc<DashMap<Uuid, AgentState>>` — in-memory cache, startup-loaded from DB
- Write-through: persist to DB → await confirm → update DashMap
- Persist failure → DashMap NOT updated → Agent receives ExecutionResult(success=false)

**Conflict Resolution**: FIFO via single IntentWorker (zero race conditions)

**Atomic Intent Queue**:
- Single tick can submit multiple independent ATOMIC Intents (`subsequent_intents`), executed in order
- Failed Intent triggers rollback ONLY for itself and aborts the rest of the queue (previously successful intents in the queue are kept)

Key server modules:
- `src/tick/scheduler.rs` - Pure clock scheduler (decay + broadcast)
- `src/tick/realtime.rs` - IntentWorker (real-time intent processing engine)
- `src/tick/processor/` - StateProcessor (validate + execute + Saga rollback)
- `src/actions/` - Action execution with data-driven ActionType
- `src/game_data/` - Config loading, caching, and formula evaluation
- `src/governance/` - Soul 审议引擎 (SoulReviewEngine): 投票式提案审核, ProposalStore, TopicClassifier
- `src/websocket/` - WebSocket connection management
- `src/handlers/` - HTTP API endpoints (dashboard SPA via `/admin/*`)
- `src/state.rs` - Shared AppState, AgentStateCache
- `src/chronicle/` - Chronicle generation (群像传记): auto-generates every 7 game days

### Agent Architecture

The agent crate implements a **unified Agent SDK** with cognitive engine, memory, persona, and two runtime modes. Both modes share identical initialization and core architecture.

#### Three-Soul Architecture

```
ActorSoul (人魂) → ReflectorSoul (天魂)
  直连 WorldState    三层审查
  内嵌 EarthSoul tool calling（LLM 推理中按需调用）
```

- **ActorSoul** (人魂/行动之魂): 直连 WorldState, outputs structured Intent with CognitiveChain, driven by CognitiveEngine (four-stage: Perception→Motivation→Planning→Decision)
- **EarthSoul** (地魂/能力之魂): tool calling 工具池，嵌入 ActorSoul 的 LLM 推理循环中。LLM 按需调用工具（`query_world`, `search_memory`, `skill_view`, `list_skills` 等）
- **ReflectorSoul** (天魂/守护之魂): 三层审查 — Layer 1 (action_type) → Layer 2 (RuleEngine) → Layer 3 (LLM OOC review).

#### Memory System (Three-Tier Architecture)

- **Working Memory**: Short-term context, recent events
- **Episodic Memory**: Event-based memories with timestamps (SQLite)
- **Semantic Memory**: Vector-based knowledge store using HNSW indexing (bge-small-zh-v1.5)
- **Outcome Memory**: Action result learning
- **CoreAffect**: Emotion-memory linkage driven by Barrett's theory (valence×arousal)

#### Token Optimization & Performance
- **AttentionController & DeltaEngine**: Lean prompts via WorldStateStore diffing and two-stage focus summarization
- **DeepSeek Cache Tuning**: system_hash metric tracking, reasoning stripping (D8, 默认关闭，env var `CYBER_JIANGHU_PROMPT_STRIP_REASONING_CONTENT` 启用), and schema canonicalization (D9)

### Data-Driven Design

All game mechanics configured via YAML in `crates/server/config/`:
- `actions.yaml`, `attributes.yaml`, `items.yaml`, `locations.yaml`
- `game_rules.yaml`, `time.yaml`, `emotion.yaml`, `narrative_config.yaml`
- `skills/` — AI Procedural Skills (SKILL.md)

**AI Procedural Skills**: 5 meta-cognitive behavioral frameworks. Acquired via experience thresholds automatically, pushed via WebSocket `ConfigUpdate`, cached locally.

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
| `candle-core/transformers` | Local BERT inference (embedding crate) |

## Key Configuration Files

| Purpose | Path |
|---------|------|
| Environment variables | `.env` |
| Server configuration | `crates/server/config/*.yaml` |
| World-building rules | `crates/server/config/world_building_rules.yaml` |
| Skill definitions | `crates/server/config/skills/{category}/{skill_id}/SKILL.md` |
| Prompt templates (agent) | `crates/server/config/prompt_templates.yaml` (含 `rule_sections` 按需检索配置) |
| Souls governance config | `crates/server/config/souls.yaml` (Soul 审议规则、投票阈值、主题路由) |
| Action evolution config | `crates/server/config/action_evolution.yaml` (动作演化策略、能力清单) |
| Database migrations | `crates/server/migrations/*.sql` |
| Docker stack | `docker-compose.yml`, `docker-compose.prod.yml` |

## API Endpoints

### Server (port 23333)

**Agent Lifecycle**:
- `POST /api/v1/device/verify` - Strict device verification (returns 404 if unknown; agent must re-register)
- `POST /api/v1/device/register` - Explicit device registration (server generates device_id, returns 201 Created)
- `POST /api/v1/agent/register` - Register new agent (returns `narrative_config`)
- `POST /api/v1/agent/retire` - Retire active character (mark as retired)
- `POST /api/v1/agent/auto-rebirth` - Auto rebirth (INSERT new agent, old agent dead→retired)
- `GET /api/v1/agent/{id}/context` - Get agent context
- `POST /api/v1/agent/biography` - Receive biography from agent (body: `{agent_id, biography}`)
- `GET /api/v1/agent/{id}/biography` - Get agent biography from server DB (fallback read for agent)
- `POST /api/v1/agent/grant-items` - Admin inventory injection (requires write_token)
- `POST /api/v1/validate-action` - Validate action parameters
- `POST /api/v1/action-evolution/propose` - Submit action evolution proposal

**WebSocket**:
- `WS /ws?token={auth_token}` - WebSocket connection

**Dashboard (Read Token)**:
- `GET /api/dashboard/agents` - List all agents
- `GET /api/dashboard/agents/offline` - Offline agents
- `GET /api/dashboard/agents/dead` - Dead agents
- `GET /api/dashboard/agent/{id}` - Agent details
- `GET /api/dashboard/agent/{id}/experiences` - Agent experiences
- `GET /api/dashboard/agent/{id}/vendor-refill` - Vendor refill rules
- `GET /api/dashboard/agent-daily-summaries` - All daily summaries
- `GET /api/dashboard/agent-daily-summaries/{agent_id}` - Agent daily summaries
- `GET /api/dashboard/stats` - Dashboard statistics
- `GET /api/dashboard/experiences` - Experience stream
- `GET /api/dashboard/chronicles` - List chronicles
- `GET /api/dashboard/chronicles/{id}` - Get chronicle
- `GET /api/dashboard/chronicles/llm-stats` - LLM token stats
- `GET /api/dashboard/chronicles/pending` - Pending generation tasks
- `GET /api/dashboard/actions-map` - Actions mapping
- `GET /api/dashboard/items` - List items
- `GET /api/dashboard/status-configs` - Status configurations
- `GET /api/dashboard/display-map` - Action type display name mapping
- `GET /api/dashboard/layer-display` - Tianhun layer display name mapping (data-driven)

**Dashboard (Write Token)**:
- `POST /api/dashboard/agents/cleanup` - Cleanup offline agents
- `POST /api/dashboard/chronicles/generate` - Generate chronicle
- `PUT /api/dashboard/agent/{id}/vendor-refill` - Set vendor refill rules
- `DELETE /api/dashboard/agent/{id}/vendor-refill/{item_id}` - Delete vendor refill rule
- `GET/PUT /api/config/{filename}` - Get/update config file content
- `POST /api/config/llm` - Save LLM config
- `GET/POST /api/config/llm/enabled` - LLM enabled flag

**Admin Auth**:
- `POST /api/admin/login` - Admin login
- `POST /api/admin/logout` - Admin logout
- `GET /api/admin/session` - Check admin session
- `POST /api/admin/reload-config` - Reload game config
- `GET /health` - Health check

### Embedding Service (port 23350, Docker standalone)

- `GET /api/health` - Health check (model loaded status)
- `POST /api/embed` - Single text embedding (`{"text": "..."}` -> `{"embedding": [...], "dimension": 512}`)
- `POST /api/embed-batch` - Batch text embedding (`{"texts": [...]}` -> `{"embeddings": [[...], ...]}`)

Agent embedder provider selection (via `CYBER_JIANGHU_EMBEDDER_REMOTE_URL` env var):
- Set → Remote mode (HTTP to embedding service, fast fail on connection error)
- Unset → Local mode (in-process candle-transformers)
- Both fail → Unavailable (FTS5 fallback)

### Agent HTTP API (port 23340-23999, auxiliary to WebSocket)

**Core** (WebSocket primary, HTTP auxiliary):
- `GET /api/v1/state` - Get current WorldState
- `GET /api/v1/context` - Get narrative context + DecisionContextSnapshot enrichment

**Character**:
- `GET /api/v1/character` - Get character info
- `POST /api/v1/character/generate` - LLM one-click character generation
- `POST /api/v1/character/register` - Register new character (forwards to Server)
- `POST /api/v1/character/rebirth` - Rebirth character
- `GET /api/v1/character/soul-cycles` - Get soul cycle records (paginated)
- `GET /api/v1/character/dream/records` - Get dream records
- `GET/POST /api/v1/character/dream` - Dream injection (sustained n-turn thought injection)

**Biography**:
- `GET /api/v1/character/biography` - Get cached biography (query: `agent_id`)
- `POST /api/v1/character/biography` - Generate biography from soul cycles + daily summaries (query: `agent_id`)

**Attributes & Status**:
- `GET /api/v1/attributes` - Get attribute values
- `GET /api/v1/attribute-meta` - Attribute categories
- `GET /api/v1/tick` - Get tick status
- `GET /api/v1/lifespan` - Get lifespan status
- `GET /api/v1/cognitive` - Get structured cognitive context

**Relationships & Memory**:
- `GET /api/v1/relationship/list` - Get all relationships
- `GET /api/v1/memory/recent` - Get recent memories
- `GET /api/v1/memory/daily-summaries` - Get daily summaries
- `POST /api/v1/memory/search` - Search memories (semantic)
- `POST /api/v1/memory` - Store memory

**Characters (Multi-character, 设备与角色分离)**:
- `GET /api/v1/characters` - List all characters
- `POST /api/v1/characters/switch` - Switch current character
- `GET /api/v1/characters/{agent_id}` - Get character by ID

**Validation & Review**:
- `POST /api/v1/validate` - Validate intent

**Events & Config**:
- `GET /api/v1/events` - Death events SSE stream
- `GET /api/v1/state/stream` - WorldState + IntentSnapshot composite SSE stream (桌面窗口消费)
- `GET/POST /api/v1/config/llm-disabled` - LLM disable toggle
- `GET/POST /api/v1/config/auto-rebirth` - Auto-rebirth toggle
- `GET/POST /api/v1/config/llm` - Get/update LLM config
- `GET /api/v1/config/llm/providers` - Get LLM providers
- `GET /api/v1/config/llm/usage` - Get LLM token usage
- `POST /api/v1/config/reload` - Hot reload config
- `POST /api/v1/config/server` - Set server address
- `GET /api/v1/setup/status` - Get setup status
- `GET /api/v1/actions` - Get action type mapping
- `GET /api/v1/metrics` - LLM performance metrics (支持 `?system_hash=<hex64>` 按 system_hash 维度过滤, Phase 0 测量用)

### Admin Web Panel
- `GET /admin/` - Main dashboard
- `GET /admin/{*path}` - Admin panel routes (served from `crates/server/static/admin/`)

## Quick Start Guides

- [QuickStart-Server.md](crates/server/QuickStart-Server.md) - Server development
- [QuickStart-Agent.md](crates/agent/QuickStart-Agent.md) - Agent development
- [Architecture docs](crates/server/docs/architecture/) and [Agent docs](crates/agent/docs/architecture/)
