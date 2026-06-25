# Cyber-Jianghu - Communication Protocol Layer

> **中文版本**: [Readme.md](./Readme.md)

This is the game's core communication protocol library. It defines all shared data structures, message exchange formats, and error types between the server (The Way of Heaven) and agents (All Beings).

The protocol layer adopts a stateless, purely data-driven design philosophy—drawing a clean, unified boundary between the game physics engine and the AI agents.

## Usage

Add to a Rust project:

```toml
[dependencies]
cyber-jianghu-protocol = { path = "crates/protocol" }
# If database support is needed
cyber-jianghu-protocol = { path = "crates/protocol", features = ["sqlx-support"] }
```

## Architecture Documentation

For more details, see the `docs/architecture/` directory.

### Core Systems

| Document | Description |
|----------|-------------|
| [action_type.md](docs/architecture/p0_core/action_type.md) | Fully data-driven action type system |
| [game_error.md](docs/architecture/p0_core/game_error.md) | Unified and standardized error code system |
| [websocket_pipeline.md](docs/architecture/p0_core/websocket_pipeline.md) | Real-time full-duplex communication pipeline design |

### Major Features

| Document | Description |
|----------|-------------|
| [attribute_component.md](docs/architecture/p1_major/attribute_component.md) | Modular character attribute component |
| [dialogue_session.md](docs/architecture/p1_major/dialogue_session.md) | Dialogue session management between agents |
| [hierarchical_map.md](docs/architecture/p1_major/hierarchical_map.md) | Hierarchical world map location system |
| [soul_cycle_report.md](docs/architecture/p1_major/soul_cycle_report.md) | Three-soul cognitive cycle report and tracking |
| [subsequent_intents.md](docs/architecture/p1_major/subsequent_intents.md) | Continuous atomic action intent queue |

### Experience Enhancements

| Document | Description |
|----------|-------------|
| [graded_llm_validation.md](docs/architecture/p2_enhancement/graded_llm_validation.md) | Tiered LLM behavior compliance validation |
| [immediate_event.md](docs/architecture/p2_enhancement/immediate_event.md) | Immediate broadcast mechanism for emergent events |
| [nl_state_mapping.md](docs/architecture/p2_enhancement/nl_state_mapping.md) | Automatic mapping from machine state to natural language |
| [numeric_leak_guard.md](docs/architecture/p2_enhancement/numeric_leak_guard.md) | Guard against the LLM leaking underlying numeric values |
| [world_building.md](docs/architecture/p2_enhancement/world_building.md) | Boundary constraints for game worldview and era setting |

## License

MIT OR Apache-2.0
