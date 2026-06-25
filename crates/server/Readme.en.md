# Cyber-Jianghu - Server (The Way of Heaven)

> **中文版本**: [Readme.md](./Readme.md)

The **Server** is the underlying "physics engine" (i.e. the Way of Heaven) of the entire multi-agent online simulation world. It does not interfere with individual choices—it only objectively maintains the world state, advances the passage of time, processes agent action requests, and periodically broadcasts environmental changes to the world.

## Core Design Principles

| Principle | Description |
|-----------|-------------|
| **Absolute Server Authority** | The server is the single source of truth. Agents can only submit "action intents"—whether the action ultimately succeeds is decided mercilessly by the server according to physical laws and game rules. |
| **Real-Time, Conflict-Free Processing** | All concurrent agent requests enter a single, one-way real-time processing channel and are executed in sequence. This design completely eliminates concurrent data conflicts and achieves efficient, safe state transitions. |
| **Safe In-Memory and Persistence Mechanism** | The in-memory world state is always the latest read source, but any state change must be safely persisted to the database before it is mirrored into memory. This design effectively prevents "ghost states" when the system crashes. |
| **Pure Data-Driven** | Core game mechanics (martial arts skills, item attributes, crafting recipes, world laws) are never hard-coded in the source—they are fully driven by external configuration files and support dynamic hot-reload at runtime. |

## Quick Start

- See [Server Quick Start Guide](QuickStart-Server.en.md)

## Architecture Documentation

For more details, see the `docs/architecture/` directory.

### Core Systems

| Document | Description |
|----------|-------------|
| [tick_scheduler.md](docs/architecture/p0_core/tick_scheduler.md) | World-time advancement engine and the natural physiological decay mechanism for characters |
| [realtime_pipeline.md](docs/architecture/p0_core/realtime_pipeline.md) | Real-time action processing channel (lock-free concurrency engine) |
| [state_processor.md](docs/architecture/p0_core/state_processor.md) | State processor (guaranteeing strict consistency between state updates and database storage) |
| [action_system.md](docs/architecture/p0_core/action_system.md) | Action validation and execution system |
| [high_performance_state.md](docs/architecture/p0_core/high_performance_state.md) | High-performance in-memory state cache design |

### Major Features

| Document | Description |
|----------|-------------|
| [connection_session.md](docs/architecture/p1_major/connection_session.md) | Network connection session, request rate limiting, and player-device binding |
| [game_data_driven.md](docs/architecture/p1_major/game_data_driven.md) | Game configuration-driven system |
| [procedural_skills.md](docs/architecture/p1_major/procedural_skills.md) | AI procedural skills and the experience-driven auto-acquisition system |

### Experience-Enhancement Features

| Document | Description |
|----------|-------------|
| [chronicle.md](docs/architecture/p2_enhancement/chronicle.md) | Auto-recorded and generated world group chronicles and historical annals |
| [http_api_admin.md](docs/architecture/p2_enhancement/http_api_admin.md) | Visual admin dashboard API and dynamic config hot-reload mechanism |

## License

MIT OR Apache-2.0
