# AGENTS.md

This file provides guidance to AI coding agents (Claude Code, Zed, Codex, etc.) when working with code in this repository.

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
- **Dedicated-Model Training Data Pipeline**: Structured collection of Agent↔LLM interaction traces into a survival-reward ledger + SFT export pipeline, for fine-tuning a world-specialized model. Reward anchors survival causality only (天道无为); subjective cognition (reputation/relationship/mood) never enters reward

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

# Check / install agent self-update from GitHub Release (CLI never restarts itself)
cyber-jianghu-agent update --check-only
cyber-jianghu-agent update

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
# The Server runs migrations on every startup via run_migrations() (replicates
# the entrypoint logic), so the `postgres` service no longer needs a mounted
# migration volume in production. Files are applied in filename order.
# Key tables: agents, agent_states, experiences, action_evolution_proposals,
#   action_evolution_proposal_groups, soul_review_votes, resource_nodes (gatherable stock)
# Key migrations:
#   022_agent_relationships.sql     - relationship graph table (agent_relationships)
#   023_chronicle_period_unique.sql - chronicle period uniqueness constraint
#   026_resource_nodes.sql          - resource node stock table (gatherable depletion model)
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
- `src/training_export/` - SFT training-data export pipeline (config loader + scheduler + runner + sft_transform + checkpoint; reward ledger lives in `src/reward/`)

**Migration Runner**: Server runs migrations on every startup via `run_migrations()` (replicates the Docker entrypoint logic), so the deploy does not depend on a mounted migration volume. Migration files in `crates/server/migrations/*.sql` are applied in filename order.

**Dashboard Read Auth**: Dashboard READ endpoints use `require_client_read_token`, which accepts either `CLIENT_READ_TOKEN` (game-client read-only token) or the admin `ADMIN_READ_TOKEN`. When `CLIENT_READ_TOKEN` is unset, callers may use `ADMIN_READ_TOKEN` (fallback). This separates the game-client read credential from admin access.

**Relationship Sync (Strategy B)**: Agents report a full relationship snapshot at game-day end via `ClientMessage::RelationshipSnapshot`. The server keeps only the **latest** snapshot per holder: each write is a transactional DELETE-all-for-source + INSERT, keyed by the directed pair `(source_agent_id, target_agent_id)` (migration `022`). `game_day` is accepted and logged but NOT persisted — there is no per-day history. Re-sending the same snapshot is idempotent; an older snapshot arriving after a newer one rolls the holder's graph back (no ordering guard; agents report sequentially over a single connection, so this is not expected in practice). Relationship perception is strictly directional: A→B and B→A are independent rows — A's view of B never overwrites B's view of A.

### Agent Architecture

The agent crate implements a **unified Agent SDK** with cognitive engine, memory, persona, and two runtime modes. Both modes share identical initialization and core architecture.

#### Three-Soul Architecture

```
ActorSoul (人魂) → ReflectorSoul (天魂)
  直连 WorldState    四层审查
  内嵌 EarthSoul tool calling（LLM 推理中按需调用）
```

- **ActorSoul** (人魂/行动之魂): 直连 WorldState, outputs structured Intent with CognitiveChain, driven by CognitiveEngine (four-stage: Perception→Motivation→Planning→Decision)
- **EarthSoul** (地魂/能力之魂): tool calling 工具池，嵌入 ActorSoul 的 LLM 推理循环中。LLM 按需调用工具（`query_world`, `search_memory`, `skill_view`, `list_skills` 等）
- **ReflectorSoul** (天魂/守护之魂): 四层审查 — Layer 0 (硬性目标校验：物品持有/可见、人物在场，拦截 LLM 臆造目标，见 `reflector/hard_logic.rs`) → Layer 1 (action_type) → Layer 2 (RuleEngine) → Layer 3 (LLM OOC review).

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

| Element             | Convention             | Example                          |
| ------------------- | ---------------------- | -------------------------------- |
| Structs/Enums       | `PascalCase`           | `TickScheduler`, `ActionType`    |
| Functions/Variables | `snake_case`           | `execute_tick()`, `agent_states` |
| Constants           | `SCREAMING_SNAKE_CASE` | `MAX_RETRY_ATTEMPTS`             |
| Type aliases        | `PascalCase`           | `GameRulesCallback`              |

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

### SQL / sqlx Conventions

本节防的是一族"编译/单测不可见、只在真实 PostgreSQL 执行才暴露"的缺陷（曾三次线上事故：validator SUM 解码、telemetry EXTRACT 解码、game_rules_config 幻表）：

- **聚合返回类型按 PostgreSQL 类型提升规则解码**：`COUNT(*)`/`SUM(int4)` 返回 BIGINT，只能按 `i64` 解码；`SUM(int8)`/`AVG`/`EXTRACT(EPOCH)`/`PERCENTILE_CONT` 返回 numeric，进 Rust 前必须 `::float8`（f64）或 BigDecimal，否则 sqlx 运行期报 mismatched types。
- **禁止在 sqlx 解码点吞错**：`.ok()`/`unwrap_or(0)`/`map_err(|_| ...)` 会把解码失败伪装成 None/0；映射自定义错误前必须 log 底层 `sqlx::Error`。
- **运行时配置以内存 game_data registry 为准**：数据库没有 `game_rules_config` 之类的配置表，SQL 不得引用；表/列名以 `crates/server/migrations/` 为唯一事实（如 `server_deployment.deployed_at`）。
- **新增/修改聚合或含计算列的 SQL，必须跑活库守卫测试并补对应执行路径**：见 `crates/server/tests/sqlx_live_schema_guard_test.rs` 文件头的运行命令（需 `DATABASE_URL`）。空表路径测不出解码缺陷，守卫测试必须写入夹具数据。
- **静态 SQL 一律用 `sqlx::query!`/`query_as!`/`query_scalar!` 宏**（编译期对真实 schema 校验表/列/参数/返回类型，幻表幻列与解码错配在编译期归零）。可空性不被推断时用 `AS "name!"` 强制非空标注（语义上确非空才允许）。动态拼接 SQL（format! 构建）无法用宏，必须走活库守卫测试。**修改任何宏查询后必须重跑** `cargo sqlx prepare --workspace`（需 `DATABASE_URL` 指向迁移完毕的库，见守卫测试文件头）并提交 `.sqlx/` 缓存，否则 CI（`SQLX_OFFLINE=true`）会因缓存缺失而失败。

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

| Crate                      | Purpose                                   |
| -------------------------- | ----------------------------------------- |
| `axum`                     | Web framework with WebSocket support      |
| `sqlx`                     | PostgreSQL async driver                   |
| `tokio`                    | Async runtime                             |
| `evalexpr`                 | Formula/expression evaluation             |
| `tokio-tungstenite`        | WebSocket client (agent)                  |
| `rusqlite`                 | Local SQLite storage (agent memory)       |
| `instant-distance`         | HNSW vector index (agent semantic memory) |
| `candle-core/transformers` | Local BERT inference (embedding crate)    |

## Key Configuration Files

| Purpose                  | Path                                                                             |
| ------------------------ | -------------------------------------------------------------------------------- |
| Environment variables    | `.env`                                                                           |
| Server configuration     | `crates/server/config/*.yaml`                                                    |
| World-building rules     | `crates/server/config/world_building_rules.yaml`                                 |
| Skill definitions        | `crates/server/config/skills/{category}/{skill_id}/SKILL.md`                     |
| Prompt templates (agent) | `crates/server/config/prompt_templates.yaml` (含 `rule_sections` 按需检索配置)   |
| Souls governance config  | `crates/server/config/souls.yaml` (Soul 审议规则、投票阈值、主题路由)            |
| Action evolution config  | `crates/server/config/action_evolution.yaml` (动作演化策略、能力清单)            |
| Training export config   | `crates/server/config/training_export.yaml` (SFT 导出：调度/分桶/容量，env 覆盖) |
| Reward config            | `crates/server/config/reward.yaml` (生存 reward ：分量/周期，fail-fast 强制配置) |
| Database migrations      | `crates/server/migrations/*.sql`                                                 |
| Docker stack             | `docker-compose.yml`, `docker-compose.prod.yml`                                  |

### Config Hot-Reload Coverage (升级 runbook 必读)

配置变更的生效路径分两档，静默替换文件对第二档**不生效**：

- **自动热重载（scheduler mtime 监视，下一 tick 生效并广播 ConfigUpdate）**：仅 `actions.yaml`、`skills/`、`narrative_config.yaml`、`game_rules.yaml`、`world_building_rules.yaml`、`prompt_templates.yaml`（见 `tick/scheduler.rs` watch 清单）。
- **需手动全量重载**：其余全部配置——含 `attributes.yaml`、`emotion.yaml`、`items.yaml`、`locations.yaml`、`initial_inventory.yaml`、`time.yaml`、`recipes.yaml` 等。生效方式：`POST /api/admin/reload-config`（write token；全量重载 + 原子换缓存 + 刷新存量 Agent 的 StatusComponent 元数据，保留当前属性值只更新 decay/max 等元信息）或重启 server。

升级部署时替换了第二档配置文件的，必须在 runbook 中包含 reload-config 或重启步骤，否则旧值静默继续运行（无错误无告警）。

### Environment Variables (auth)

| Variable                    | Required | Purpose                                                                                                                                                                                                                                  |
| --------------------------- | -------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ADMIN_READ_TOKEN`          | optional | Dashboard / read API token. If unset, Server generates a random one at startup and logs it.                                                                                                                                              |
| `ADMIN_WRITE_TOKEN`         | optional | Dashboard write API token. If unset, Server generates a random one at startup and logs it.                                                                                                                                               |
| `CLIENT_READ_TOKEN`         | optional | Read-only auth token for game clients. If unset, dashboard READ endpoints fall back to `ADMIN_READ_TOKEN`. When set, dashboard READ endpoints (`require_client_read_token`) accept **either** `CLIENT_READ_TOKEN` or `ADMIN_READ_TOKEN`. |
| `CYBER_JIANGHU_AGENT_TOKEN` | optional | Agent-side static HTTP API token (union with device `auth_token`). When set, external clients can authenticate against the agent HTTP API before device registration.                                                                    |

## Protocol Types

The `crates/protocol` crate also exports `PROTOCOL_VERSION`: an **independent semver** string (currently `3.2.0`) decoupled from the crate version, describing the Server/Agent/Client wire contract. Bump rules: incompatible wire change -> major; additive optional field/endpoint -> minor; no-contract-impact fix -> patch. Contract JSON Schema fragments for external consumers live in `docs/contracts/` (`world_state` / `intent` / `version` / `state_stream`).

The `crates/protocol` crate defines the wire types shared between Server, Agent, and Dashboard/Client. Key enums and contract structs:

### Content & Risk Enums

- `OocRisk` (`low` / `medium` / `high`) — out-of-character risk classification emitted during ReflectorSoul review / dialogue validation.

### Config Enums (data-driven config model, `crates/server/config/*.yaml`)

- `ConfigType` (`skills` / `actions` / `game_rules` / `world_building_rules` / `prompt_templates` / `persona_event_rules` / `narrative_config`) — discriminator for `GET/PUT /api/config/{filename}` payloads.
- `EffectType` (`attribute_change` / `attribute_max_change` / `add_item`) — action effect kinds.
- `RequirementType` (`attribute` / `item`) — gating requirements for actions/skills.
- `Operation` (`add` / `set` / `multiply`) — arithmetic operation applied to an attribute in `EffectType`.
- `ValidationType` (`not_empty` / `min_value` / `max_value` / `min_length` / `max_length` / `item_exists`) — declarative validators used by `actions.yaml`.

### Relationship Protocol Contract (Strategy B, full snapshot per game day)

- `RelationshipMemory` — relationship memory record with `i64` millisecond timestamps (epoch ms), agent IDs, valence/affinity score, and free-form metadata.
- `RelationshipKeyEvent` — discrete event that shifted a relationship (fight, trade, gift, dialogue, etc.); `i64` ms timestamps.
- `ClientMessage::RelationshipSnapshot` — variant carrying a full relationship snapshot from Agent → Server at game-day end. The server keeps only the latest snapshot per holder (last-writer-wins on the `source_agent_id` scope); `game_day` is accepted for logging only. Idempotent for same-day resend; no cross-day ordering guard. Edges are strictly directional: A→B and B→A are independent rows.

Timestamps in the relationship protocol are `i64` milliseconds (Unix epoch), not `f64` seconds.

## API Endpoints

### Server (port 23333)

**Agent Lifecycle**:

- `GET /api/v1/version` - Protocol handshake (public, no auth; returns `server_version` + `protocol_version`)
- `POST /api/v1/device/verify` - Strict device verification (returns 404 if unknown; agent must re-register)
- `POST /api/v1/device/register` - Explicit device registration (server generates device_id, returns 201 Created)
- `POST /api/v1/agent/register` - Register new agent (returns `narrative_config`)
- `POST /api/v1/agent/retire` - Retire active character (mark as retired)
- `POST /api/v1/agent/by-device` - Look up agent(s) by device_id (device→character mapping)
- `POST /api/v1/agent/auto-rebirth` - Auto rebirth (INSERT new agent, old agent dead→retired)
- `GET /api/v1/agent/{id}/context` - Get agent context
- `POST /api/v1/agent/biography` - Receive biography from agent (body: `{agent_id, biography}`)
- `GET /api/v1/agent/{id}/biography` - Get agent biography from server DB (fallback read for agent)
- `POST /api/v1/agent/grant-items` - Admin inventory injection (requires write_token)
- `POST /api/v1/validate-action` - Validate action parameters
- `POST /api/v1/action-evolution/propose` - Submit action evolution proposal

**Training Data Export** (mixed auth: `write_token` for trigger/delete, `client_read_token` for reads; scheduler runs in background, endpoints are manual-trigger/inspect):

- `POST /api/v1/training/export` - Trigger an SFT export run (write_token)
- `GET /api/v1/training/exports` - List export runs (client read token)
- `GET /api/v1/training/exports/{run_id}` - Get a run's metadata (client read token)
- `DELETE /api/v1/training/exports/{run_id}` - Delete a run + artifacts (write_token)
- `GET /api/v1/training/exports/{run_id}/download` - Download exported SFT data (client read token)
- `GET /api/v1/training/checkpoint` - Get scheduler checkpoint / progress (client read token)

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
- `GET /api/dashboard/reward/trends` - Survival-reward trends (daily/period aggregates)
- `GET /api/dashboard/reward/lifetime/{id}` - Agent lifetime reward summary
- `GET /api/dashboard/chronicles/pending` - Pending generation tasks
- `GET /api/dashboard/actions-map` - Actions mapping
- `GET /api/dashboard/items` - List items
- `GET /api/dashboard/status-configs` - Status configurations
- `GET /api/dashboard/display-map` - Action type display name mapping
- `GET /api/dashboard/layer-display` - Tianhun layer display name mapping (data-driven)

**Dashboard Read (Client Token)** — these endpoints use `require_client_read_token` (accepts `CLIENT_READ_TOKEN` or admin read token), designed for read-only game client access:

- `GET /api/dashboard/agent-relationships` - Global relationship graph (all agents)
- `GET /api/dashboard/agent-relationships/{agent_id}` - Single agent's relationships
- `GET /api/dashboard/world-snapshot` - Unified world snapshot (read-only transaction isolation; aggregates world state in a single consistent read)
- `GET /api/dashboard/locations` - Location/map graph structure
- `GET /api/dashboard/dialogues` - Aggregated dialogue view (supports `?limit`, `?tick_from`)
- `GET /api/dashboard/deaths` - Death timeline (supports `?limit`, `?tick_from`)
- `GET /api/config/{filename}` - Get config file content (frontend boot fetches locations/attributes YAML)
- `GET /api/dashboard/config/llm` - Get LLM config
- `GET /api/dashboard/config/llm/enabled` - LLM enabled flag
- `GET /api/dashboard/agent/{id}/roles` - Get agent roles

**Dashboard (Write Token)**:

- `POST /api/dashboard/agents/cleanup` - Cleanup offline agents
- `POST /api/dashboard/chronicles/generate` - Generate chronicle
- `PUT /api/dashboard/agent/{id}/vendor-refill` - Set vendor refill rules
- `PUT /api/config/{filename}` - Update config file content
- `POST /api/dashboard/config/llm` - Save LLM config
- `POST /api/dashboard/config/llm/enabled` - Set LLM enabled flag
- `POST /api/dashboard/agent/{id}/roles` - Assign role

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
- `POST /api/v1/admin/reload-character` - Reload character.yaml into the running agent (hot persona refresh)
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
- `GET /api/v1/memory/recent` - Get recent memories (supports `?since=<RFC3339>` incremental mode: returns bare array of `{tick_id, event_type, payload}` for offline frame-fill, bounded by MAX_PAGE_SIZE=100)
- `GET /api/v1/memory/daily-summaries` - Get daily summaries
- `POST /api/v1/memory/search` - Search memories (semantic)
- `POST /api/v1/memory` - Store memory

**Characters (Multi-character, 设备与角色分离)**:

- `GET /api/v1/characters` - List all characters
- `POST /api/v1/characters/switch` - Switch current character
- `GET /api/v1/characters/{agent_id}` - Get character by ID
- `POST /api/v1/characters/{agent_id}/rebirth` - Rebirth by id (409 if not the active character; delegates to `/api/v1/character/rebirth`)
- `POST /api/v1/characters/{agent_id}/inject-dream` - Dream injection by id (409 if not the active character; delegates to `/api/v1/character/dream`)
- `GET/POST /api/v1/characters/{agent_id}/biography` - Get/generate biography by id (delegates to `/api/v1/character/biography?agent_id=`)

**Validation & Review**:

- `POST /api/v1/validate` - Validate intent

**Self-Update (GitHub Release)**:

- `GET /api/v1/update/status` - Update status view (current version/digest, latest release, last check)
- `POST /api/v1/update/check` - Check latest GitHub release now
- `POST /api/v1/update/apply` - Download + install latest release and restart (refused inside containers and for cargo `target/` builds)

**Events & Config**:

- `GET /api/v1/events` - Death events SSE stream
- `GET /api/v1/version` - Protocol handshake (public, no auth; `protocol_version` is an independent semver `PROTOCOL_VERSION`, `server_version` proxied live from server, null when unreachable)
- `GET /api/v1/state/stream` - WorldState + IntentSnapshot composite SSE stream (桌面窗口消费; view-shape payload per `docs/contracts/state_stream.schema.json`; accepts `?token=` for EventSource)

**Agent HTTP Auth**: all agent HTTP endpoints (except public paths `/`, `/api/v1`, `/api/v1/health`, `/api/v1/version`, `/api/v1/setup`, static assets) require `Authorization: Bearer <token>`. Accepted tokens are the UNION of env `CYBER_JIANGHU_AGENT_TOKEN` (static token, enables auth before device registration for external clients) and the device `auth_token` from server registration (used by the local panel). SSE endpoints (`/api/v1/events`, `/api/v1/state/stream`) additionally accept `?token=<token>`. No token configured at all -> 503 (fail-closed).

**Agent Self-Update**: agent can self-update from GitHub Releases (`update` section in `agent.yaml`: `enabled`/`auto_apply`/`check_interval_secs`/`repo`, defaults `true`/`true`/`21600`/`8kugames/Cyber-Jianghu`). Update decision is digest identity (current exe sha256 vs latest release platform asset `digest`) because release tags carry the SERVER version while agent crate versions are independent. Download is sha256-verified (missing digest => refuse to install). Background loop auto-applies and restarts (unix execve / Windows `*.exe.old` + spawn). Auto-apply is skipped inside containers (`/.dockerenv` / `/.containerenv` — update the image instead) and for cargo builds (exe under `target/`). Env `CYBER_JIANGHU_SELF_UPDATE=0` hard-disables all update network activity. The CLI `update` subcommand installs but never restarts itself.

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
