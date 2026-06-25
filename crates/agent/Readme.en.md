# Cyber-Jianghu - Agent SDK (All Beings)

> **中文版本**: [Readme.md](./Readme.md)

The **Agent SDK** is the bridge that connects to "The Way of Heaven" server, providing developers with the infrastructure to interact with the game world. It includes advanced AI modules for memory, cognition, dialogue, and emotion, and is built on a strictly first-principles architecture to ensure AI behavior is both logical and deeply human.

## Core Architectural Principles

### Flexible Modular Composition

Agent capabilities are no longer a rigid inheritance hierarchy—they are composed on demand like building blocks. By combining different capability modules (memory system, LLM client, immediate-event processing engine, world-state cache, etc.), you can flexibly customize an agent's behavioral foundation.

### Three-Soul Architecture (The Agent's Mental Core)

The agent's thinking and decision-making process is divided into three collaborating "soul" modules:

- **Human Soul — Action Soul (人魂)**: The agent's main brain. It directly perceives the objective world state, without going through any complex intermediate translation, and the LLM completes the entire reasoning pass in one breath: "perceive environment → generate motivation → make a plan → decide." It then outputs a concrete action intent.
- **Earth Soul — Capability Soul (地魂)**: A "toolbox" embedded in the Human Soul's thinking. It provides the LLM with tools for memory lookup, skill browsing, relationship queries, etc., letting the AI pull precise objective data on demand during thinking and ensuring decisions are well-grounded.
- **Heaven Soul — Guardian Soul (天魂)**: The agent's internal gatekeeper and compliance officer. Before an action is truly submitted to the server, Heaven Soul performs a strict three-layer check: whether the action is legal, whether physical rules allow it, and whether the behavior severely breaks the character's persona (out-of-character prevention). On any violation, Heaven Soul immediately intercepts and rejects it in natural language, asking the Human Soul to rethink.

### Unified Dual-Mode Runtime

To fit different runtime environments, the Agent SDK supports two execution modes:

- **Built-in Mode (default)**: The agent ships with its own LLM client; all thinking, decision-making, and memory retrieval are completed in a local closed loop.
- **External Mode**: The agent acts as a "shell." Concrete thinking and decision-making are delegated to an external scheduling node (such as OpenClaw).
> **Key Consistency**: In either mode, the agent's underlying cognitive engine, memory system, chaos generator, and other core modules are identical—the only difference is who handles communication with the LLM.

## Core Module Overview

- **Memory and Emotion System**: Includes short-term working memory, episodic memory with a forgetting curve, long-term semantic memory that supports fuzzy association, and experience memory that learns from action outcomes. The formation and retrieval of these memories are modulated by the agent's current core affect (positive/negative, aroused/calm).
- **Dynamic Persona**: A character's personality is not fixed—it evolves dynamically with experienced events and interaction feedback.
- **Immediate-Event Handling**: Specifically handles unexpected events that fall outside normal time-flow (e.g. someone suddenly talking to you), ensuring the agent can respond to external stimuli in time.
- **Anti-Divergence and Optimization**: Through strict context filtering and delta-based comparison, only the most critical environment changes are passed to the LLM, dramatically saving compute.

## API and Communication Design

**All core action interactions between the agent and the server are forced through a real-time WebSocket long-connection.**
HTTP endpoints are only auxiliary—used for dashboard display, state queries, and admin management—and never participate in submitting actions that change the game world.

## Quick Entry Points

- **[Agent Quick Start Guide](QuickStart-Agent.en.md)**
- **[Core Architecture Documentation](docs/architecture/p0_core/)**
- **[Advanced Features Documentation](docs/architecture/p1_major/)**
