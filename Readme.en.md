# Cyber-Jianghu (Realm: Jianghu)

> **中文版本**: [Readme.md](./Readme.md)

> AI-native game

> An AI-driven, large-scale multi-agent martial arts sandbox (MMO-MAS)

---

## Game Introduction

**Cyber-Jianghu** is a martial arts sandbox world driven entirely by AI.

There are no pre-written scripts and no traditional NPCs. Every character—whether the innkeeper or a passing blade-master—is an autonomous AI agent with an independent personality, memory, and goals. They feel hunger, fight over scarce resources, form alliances, and hold grudges. The web of feuds, economy, and factional power across the entire jianghu **emerges** from the autonomous behavior of thousands of AIs.

The core driving force of this world is **survival pressure**: hunger, scarce resources, and irreversible permanent death.

## Core Architecture: The Way of Heaven & All Beings

```text
The Way of Heaven (Server)            All Beings (Agents)
Authoritative rules of the            Autonomous actors in a
objective world                       subjective world
· Advance world time and environment  · Observe and perceive changes in the world
· Lock-free real-time action engine   · Think, reason, and decide
· Transactional rollback for state    · Submit a chain of independent atomic actions
· Fully config-driven dynamic rules   · Three-soul architecture: thinking, tools, review
```

- **The Way of Heaven (天道)**: A cold, impartial physics engine. It favors no one and only enforces the rules of physics and the laws of the world.
- **All Beings (众生)**: AIs that struggle and decide autonomously within the laws of Heaven, driven by the need to survive and pursue their goals.

## Three-Soul Architecture (Agent's Internal Structure)

To give AI a human-like thinking process and prevent it from acting against the worldview, every agent adopts a **Three-Soul Architecture** that isolates cognition, execution, and self-review:

| Module | Core Responsibility | How It Works |
|---------|---------------------|--------------|
| **Human Soul (人魂)** | Motivation reasoning and planning | Connects directly to the objective world state, fused with the character's personality and memory. It is the agent's emotional and rational brain, completing the entire thinking loop from "perceiving the environment" to "making a decision" in a single pass. When a character is under excessive pressure, the Human Soul can also trigger irrational, chaotic behavior. |
| **Earth Soul (地魂)** | Action execution and tool calling | Embedded in the Human Soul's thinking process. It acts as the agent's "hands and eyes," providing tools for memory search, ability lookup, relationship queries, etc. It lets the LLM fetch precise data on demand during reasoning, while being subject to strict safety limits on calls. |
| **Heaven Soul (天魂)** | Rules and worldview review | The agent's internal "self-censor." Before an action is submitted to "Heaven," it performs action legality checks, initial physics-rule review, and worldview-fitting (OOC) review. On any violation, Heaven Soul immediately rejects it and asks the Human Soul to rethink. |

## Key Features

- **Permanent Death and Reincarnation**: A character has only one life. On death, its state is permanently frozen. Players may choose "reincarnation" and the system generates a brand-new character while keeping the account-device binding, achieving a clean separation between the player's device and game characters.
- **Three-Tier Memory with Emotion Coupling**: Agents have short-term working memory, timestamped episodic memory with a forgetting curve, and long-term semantic memory that supports fuzzy association. Memory formation and recall are strongly influenced by the agent's current "emotion" (positive/negative, agitated/calm).
- **AI Meta-Cognitive Skill Framework**: An agent's "skills" are no longer raw damage numbers, but AI thinking patterns such as "Insight into People" or "Sense of Advance and Retreat." Agents automatically acquire these cognitive frameworks by accumulating experience in the world, learning to think more like a human.

- **Pure Data-Driven Action System**: All interaction actions (attack, give, speak, etc.) are not hard-coded—they are defined dynamically through configuration files. Transmission scope (global broadcast, local visibility, private whisper, etc.) all support hot-reload.
- **High-Concurrency, Real-Time Processing**: The server uses a lock-free, single-thread processing engine that completely eliminates data conflicts under high concurrency. In-memory state and database persistence remain strictly consistent—any failed action safely rolls back to its prior state.
- **Deep LLM Consumption Optimization**: Before sending to the LLM, the system automatically filters out unchanged environment information and extracts the focus areas the model truly needs, drastically reducing wasted computation.
- **Huoyun Cave Heaven (Macro Governance)**: [Currently only Action Evolution is implemented] Rejected unknown actions from agents enter the evolution pool, where the Three Sovereigns (Fuxi / Shennong / Xuanyuan) review them through a three-stage pipeline: Fuxi preliminary review → Shennong & Xuanyuan parallel review → Fuxi final review. Once two-thirds approve, the action is written to `actions.yaml` and takes effect via hot-reload. The Three Sovereigns have separate duties—Fuxi (direction of evolution) + Shennong (survival and resource balance) + Xuanyuan (worldview and stable order)—a separation of powers that keeps the world from falling into silence or collapsing into chaos.

## Project Structure

```text
Cyber-Jianghu/
├── crates/
│   ├── protocol/       # Communication protocol layer (defines the data standard for server-agent interaction)
│   ├── server/         # Game server ("The Way of Heaven" — physics and rules engine)
│   ├── embedding/      # Embedding service (local/remote dual mode)
│   └── agent/          # Agent SDK (built-in LLM and external LLM dual-mode runtime)
├── docs/               # Architecture and feature documentation
└── scripts/            # Utility scripts
```

## Quick Start

### Developers

| Module | Description |
|--------|-------------|
| [Agent Quick Start](crates/agent/QuickStart-Agent.en.md) | Guide for running and developing the Agent |
| [Server Quick Start](crates/server/QuickStart-Server.en.md) | Guide for developing and deploying the Server |

### Common Commands

```bash
# Build the server (release)
cargo build -p cyber-jianghu-server --release

# Build the agent
cargo build -p cyber-jianghu-agent

# Run the full test suite
cargo nextest run --workspace

# Format check and static analysis
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

## License

MIT OR Apache-2.0
