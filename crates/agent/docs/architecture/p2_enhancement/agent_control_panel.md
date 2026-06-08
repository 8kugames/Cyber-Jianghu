# 玩家控制台 (Agent Control Panel)

**级别**: P2 体验增强
**模块**: `crates/agent`

## 1. 第一性原理与设计目标
在以“身心分离”为核心的 MMO-MAS 架构中，Server 是客观的物理引擎（天道），而 Agent 是主观的意识体（众生）。
玩家控制台的设计目标是**为人类提供一个直接观测和干预 Agent 内部认知过程的“高维观测窗”**。它不能直接修改 Server 端的物理状态（如 HP 或位置），而只能通过“读取认知上下文”和“注入思想（托梦）”来影响 Agent 的下一步决策。

## 2. 核心机制

### 2.1 HTTP API 与 Web 面板
- Agent 内部启动一个基于 `axum` 的 HTTP 服务器（`HttpApiState` 共享状态），提供只读观测与受限干预能力。
- **状态快照与认知上下文**：通过 `/api/v1/context` 与 `/api/v1/cognitive` 接口，暴露 Agent 当前的决策上下文（包括三层记忆、滑动窗口、执行结果）。
- **三魂推演记录 (SoulCycle)**：通过 `SoulCycleRecorder` 记录并在 `/api/v1/character/soul-cycles` 暴露 Agent 的认知流（Perception -> Motivation -> Planning -> Decision）及 ReflectorSoul 的拦截日志。

### 2.2 辅助生成工具
- **LLM 一键角色生成**：暴露 `/api/v1/character/generate`，支持人类输入一段简短描述（如“一个落魄的剑客”），由 LLM 生成符合数据驱动配置的角色 YAML（包括属性、性格等）。
- **多角色热切**：支持单设备运行多个角色，提供 `/api/v1/characters/switch` 切换当前活跃角色，实现“设备身份与角色分离”。

### 2.3 托梦干预 (Dream)
- **机制原理**：人类作为“上帝”无法直接越权操作物理躯体，但可以通过 `/api/v1/character/dream` 注入一段**持续 N 个 Tick 的意念**。
- **作用链路**：托梦内容会被写入 `DreamState` 并持久化到磁盘，在每个 Tick 周期通过 `DecisionContextSnapshot` 被注入到 Agent 的工作记忆的最深处。这无视了正常的感知屏障，直接诱导 ActorSoul 的动机（Motivation）生成。

## 3. 架构约束
- **零物理状态修改**：控制台绝不能直接修改角色的属性（如强制回血），所有改动必须通过 Server 端的 Admin 面板进行。Agent 面板仅限于认知层的读取与注入。
- **数据隔离**：单设备多角色的数据必须隔离。`HttpApiState` 使用 `Arc<RwLock>` 保护所有可变状态，并基于 `agent_id` 维护按需加载的 `SoulCycleRecorder` 与 `DreamState`。

## 4. 代码入口
- 路由与控制器: `crates/agent/src/infra/api/mod.rs` 及 `crates/agent/src/infra/api/handlers/`
- HTTP 决策共享状态: `crates/agent/src/infra/api/mod.rs` (`HttpApiState`)
- 托梦状态管理: `crates/agent/src/infra/api/handlers/character_info.rs`
- 三魂记录器: `crates/agent/src/infra/api/soul_cycle_recorder.rs`