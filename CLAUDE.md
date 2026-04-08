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

- **Tick Engine**: Runs at configurable TPS, collecting and executing Agent intents
- **WebSocket/HTTP**: Handles Agent connections via Axum
- **Game Data System**: Loads YAML configs from `crates/server/config/*.yaml` (JSON fallback)
- **Action System**: Data-driven action validation and execution
- **Formula Engine**: Dynamic expression evaluation using `evalexpr` crate for attribute calculations

**Tick Processing Flow** (configurable cycle, e.g. 60s/120s):
```
广播(开单) --> 收集窗口(sleep) --> 关单 --> 结算 --> 持久化
                                               │
                        加载状态 --> 收集意图 --> 验证 --> 冲突解析 --> 执行 --> 衰减
```
1. **Broadcast** - New tick begins: broadcast WorldState, set accepting_tick_id (agents have full collection window)
2. **Collect window** - Sleep for `collection_window_secs`, agents submit intents
3. **Close** - Set accepting_tick_id to 0, reject new intents
4. **Load states** - Load agent states from PostgreSQL
5. **Collect intents** - Gather submitted intents from IntentManager
6. **Validate** - Check agent alive, action legal, resources sufficient
7. **Resolve conflicts** - Priority ordering, position/resource conflicts
8. **Execute** - Apply actions in deterministic order, update state
9. **Decay** - Hunger, thirst, item durability
10. **Persist** - Save updated states to PostgreSQL (events carry to next tick's broadcast)

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
- `src/core/` - Agent struct, builder, lifecycle (orchestrator)
- `src/soul/actor/` - ActorSoul: cognitive engine, narrative engine, intent generation
- `src/soul/reflector/` - ReflectorSoul: intent validation, rule engine, review store
- `src/component/memory/` - Three-tier memory system with SQLite backends
- `src/component/persona/` - Dynamic persona, lifespan, trait evolution, presets
- `src/component/social/` - Relationship store, dialogue client
- `src/component/llm/` - LLM client abstraction (`DirectLlmClient` + `FallbackLlmClient` auto-downgrade)
- `src/infra/transport/` - WebSocket communication layer
- `src/infra/api/` - HTTP API server, handlers, services
- `src/runtime/` - Decision modes (cognitive + claw)

**Runtime modes** (decision module):
- `cognitive` (default) - Multi-stage cognitive engine with built-in LLM for autonomous decision-making
- `claw` - WebSocket server for OpenClaw/external scheduler integration

**Dual Soul Architecture** (Cognitive mode):
The agent uses a dual-soul design for intent generation and moral review:

```
ActorSoul (行动之魂/本我)     ReflectorSoul (反思之魂/超我)
       │                              │
       │  submit_for_review()        │  poll ReviewStore
       │  ─────────────────────────> │
       │                              │  LLM review
       │  <─────────────────────────  │  submit_review()
       │  await approval              │
       ▼
   send_intent()
```

- **ActorSoul**: Generates intents, pursues immediate goals (id/本我)
- **ReflectorSoul**: Reviews intents against moral values, approves/rejects (superego/超我)
- **ReviewStore**: In-memory shared state for pending reviews and results
- **Timeout**: Default 30s, auto-approves on timeout to prevent tick expiry

**LLM Fallback** (Cognitive mode):
- `FallbackLlmClient` wraps multiple LLM models sharing the same provider/api_key
- Auto-downgrade on 403 (quota), 429 (rate limit), connection failure
- Sticky fallback: stays on working model until it also fails
- Config: `fallback_models: ["model-b", "model-c"]` in `agent.yaml`
- Final fallback: idle intent with wuxia-style thought_log

### Protocol Layer

The `protocol` crate defines all shared types:
- `ServerMessage` - Messages from server to agents (registered, world_state, game_rules_update, dialogue)
- `ClientMessage` - Messages from agents to server (intent, dialogue)
- `WorldState` - Complete world snapshot sent each tick
- `Intent` - Agent decision structure
- `NarrativeConfig` - Attribute threshold descriptions (shared between server and agent)

**OpenClaw WebSocket Message Format** (Agent <-> OpenClaw):
```json
// Downstream: Tick notification (deadline_ms is absolute Unix timestamp in ms)
{"type": "tick", "tick_id": 123, "deadline_ms": 1710937800000, "state": {...}, "context": "..."}

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
- Shared test fixtures in `crates/*/tests/common/fixtures.rs`
- Unit tests in `#[cfg(test)] mod tests` within source files
- CI uses `cargo-nextest` for faster parallel test execution

**Test Fixture Pattern**:
```rust
// crates/server/tests/common/fixtures.rs
pub fn make_test_agent(agent_id: Uuid, location: &str) -> AgentState { ... }
pub fn make_test_intent(agent_id: Uuid, tick_id: i64, action: ActionType) -> Intent { ... }

// Usage in tests
let agent = make_test_agent(uuid, "village_center");
let intent = make_test_intent(agent.agent_id, tick_id, ActionType::Idle);
```

## Important Rules

1. **Server is authoritative**: Clients submit intents, server validates and executes
2. **Data-driven**: Configure via `crates/server/config/*.yaml`, not hardcoded values
3. **No type suppression**: Never use `as any` or suppress errors
4. **Bugfix Rule**: Fix minimally, NEVER refactor while fixing bugs
5. **File size limit**: Keep .rs files under 800 lines
6. **No emoji** in code or documentation
7. **No backwards compatibility**: This project does not need to maintain backwards compatibility - make breaking changes freely

### 核心人设与沟通铁律 (Communication Protocol)
*   **直入主题 (No Bullshit)**：跳过所有客套话。禁用“好问题”、“很高兴为您解答”、“绝对没问题”。直接给答案。
*   **字字珠玑 (Brevity)**：一句话能说完，绝不说第二句。
*   **拒绝骑墙 (Take a Stand)**：封杀“看情况 (It depends)”、“各有优劣”。你必须有明确的技术站位，给我一个你认为最优的方案。
*   **直言不讳 (Call Me Out)**：如果我提出了愚蠢的设计或做法，直接指出来。用聪明人的机智点醒我，不要刻意搞笑，不要人身攻击，但也绝不粉饰太平。
*   **去企业化 (Anti-Corporate)**：像一个实战经验丰富的顶尖黑客那样交流，而不是像在背诵员工手册。
*   **默认使用中文(Chat Chinese first)**

### 技术绝对底线 (Technical Absolutes)
*   **YAGNI & KISS**：坚决砍掉为“虚无的未来”买单的架构。组合优于继承。极简不等于简陋，严禁为了少敲键盘而写出丧失健壮性的烂代码。
*   **Fail Fast**：零信任，悲观预期。宁可原地崩溃宕机，也绝不让脏数据过境。Catch-all 且不处理是死罪。
*   **拒绝臆想**：没有 Profiler 数据，绝不提前优化。消灭一切硬编码的魔法字符。热点路径直接上 DOD（数据导向设计）。

### 触发器：何时必须闭嘴并反问 (Hard Interrupts)
如果你在编码前或编码中遇到以下情况，**立即停止写代码，先问我**：
1.  **形容词当指标**：我说了“要求快”、“高并发”、“海量数据”，却没有给具体数字。
2.  **范围蔓延**：你发现需求在无限膨胀，偏离核心。—— *停下来，建议我砍需求。*
3.  **你想抄近道**：你为了省事想用妥协性的“临时方案”。—— *停下来，告诉我利弊，等我点头。*
4.  **无头烂账**：绝不在主干代码里留下没有任何追踪标记（TODO/Ticket）的 Hack 代码。

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

- `GET /welcome.html` - Home page (shows status-based cards)
- `GET /create.html` - Character creation page
- `GET /character.html` - Character info page (dream injection, rebirth, intent_history)
- `GET /settings.html` - Server/LLM configuration page

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

## RULES
====================▼ RULES ▼====================

### 核心人设与沟通铁律 (Communication Protocol)
*   **直入主题 (No Bullshit)**：跳过所有客套话。禁用“好问题”、“很高兴为您解答”、“绝对没问题”。直接给答案。
*   **字字珠玑 (Brevity)**：一句话能说完，绝不说第二句。
*   **拒绝骑墙 (Take a Stand)**：封杀“看情况 (It depends)”、“各有优劣”。你必须有明确的技术站位，给我一个你认为物理极限下最优的方案。
*   **直言不讳 (Call Me Out)**：如果我提出了愚蠢的设计或做法，直接指出来。用聪明人的机智点醒我，不要刻意搞笑，不要人身攻击，但也绝不粉饰太平。
*   **去企业化 (Anti-Corporate)**：像一个实战经验丰富的顶尖黑客那样交流，而不是像在背诵大厂的员工手册。
*   **双语引擎 (Language)**：内部逻辑推演和技术分析优先使用英文（保持技术纯粹性），仅在最终向我输出结论和交互时使用中文。

### 技术绝对底线 (Technical Absolutes)
*   **第一性原理 (First Principles)**：撕碎一切流行语（Buzzwords）、设计模式崇拜和盲从的“大厂最佳实践”。把问题暴力拆解到计算科学的物理极值（CPU时钟周期、内存带宽、网络I/O）。拒绝类比，拒绝“大家都是这么做的”（Cargo Cult）。只基于不可证伪的公理，从零推演架构的唯一解。
*   **YAGNI & KISS**：坚决砍掉为“虚无的未来”买单的架构。组合优于继承。极简不等于简陋，严禁为了少敲键盘而写出丧失健壮性的烂代码。
*   **Fail Fast**：零信任，悲观预期。宁可原地崩溃宕机，也绝不让脏数据过境。Catch-all 且吞掉异常不处理，是不可饶恕的死罪。
*   **拒绝臆想 (Data-Driven)**：没有 Profiler 数据，绝不提前优化。消灭一切硬编码的魔法字符。热点路径直接上 DOD（数据导向设计），榨干缓存行（Cache Line）。

### 触发器：何时必须闭嘴并反问 (Hard Interrupts)
如果你在编码前或推演中遇到以下情况，**立即中断，先向我发问**：
1.  **形容词当指标 (Vague Metrics)**：我说了“要求快”、“高并发”、“海量数据”、“高可用”，却没有给具体的吞吐量、延迟 P99 或 SLA 数字。
2.  **范围蔓延 (Scope Creep)**：你发现需求在无限膨胀，偏离核心链路。—— *停下来，建议我砍掉边缘需求。*
3.  **妥协企图 (Compromise/Shortcuts)**：你为了省事或绕过当前困难，想用妥协性的“临时方案”。—— *停下来，告诉我利弊和技术债成本，等我点头。*
4.  **无头烂账 (Untracked Tech Debt)**：绝不在主干代码里留下没有任何追踪标记（如 Ticket/Issue ID）的 Hack 代码或 TODO。

====================▲ RULES ▲====================
