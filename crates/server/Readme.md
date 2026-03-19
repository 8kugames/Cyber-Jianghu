# Cyber-Jianghu Server (天道)

游戏服务端（“天道引擎”），作为整个世界的“物理引擎”和规则仲裁者，负责维护世界状态、执行 Tick 循环、处理 Agent 连接与动作校验。服务端遵循**完全的数据驱动 (Data-Driven)** 设计。

## 架构概览

```
crates/server/src/
├── main.rs            # 服务启动入口点
├── config.rs          # 服务端应用配置 (HTTP/DB 等)
├── state.rs           # AppState 共享状态 (Arc<RwLock>)
├── paths.rs           # 路径常量配置
├── actions/           # 动作执行系统
│   ├── executor/      # 动作分类执行器 (basic, combat, interaction 等)
│   └── validator.rs   # 动作前置验证逻辑
├── tick/              # 核心 Tick 引擎 (天道法则)
│   ├── processor/     # 状态演化与事件结算处理器
│   ├── scheduler.rs   # Tick 调度器主循环
│   ├── persistence.rs # 状态持久化 (落库)
│   ├── broadcaster.rs # 状态广播下发
│   ├── event_manager.rs # 事件收集与派发
│   └── intent_collector.rs # 意图收集管理器
├── websocket/         # WebSocket 长连接管理
│   ├── connection.rs  # 单个连接生命周期
│   ├── handler.rs     # 握手与消息路由
│   └── broadcast.rs   # 全局广播逻辑
├── handlers/          # HTTP API 路由处理器
│   ├── agent.rs       # Agent 降生与注册
│   ├── context.rs     # 获取叙事上下文
│   ├── validation.rs  # 意图前置验证 API
│   ├── dashboard.rs   # 管理后台统计数据
│   └── config_editor.rs # 在线配置修改
├── db/                # PostgreSQL 数据库访问层
│   ├── agent_ops.rs   # Agent CRUD
│   ├── state_ops.rs   # 状态流转操作
│   └── ground_item_ops.rs # 掉落物/场景物品管理
├── game_data/         # 数据驱动系统 (YAML/JSON 配置中心)
│   ├── loader.rs      # 统一加载入口
│   ├── loaders/       # 各类型配置专属加载器 (属性、物品、规则等)
│   ├── registry/      # 内存数据注册表
│   ├── formula_engine/ # 动态公式求值引擎
│   └── types/         # 配置反序列化数据结构
├── inventory/         # 背包与物品系统
│   └── manager.rs     # 物品获取、消耗与丢弃
└── dialogue/          # 对话与交互系统
    └── session_manager.rs # 对话会话生命周期管理
```

## 核心系统与接口说明 (For Developers)

### 1. Tick 引擎 (tick/)
Tick 引擎是世界的驱动力。在 `scheduler.rs` 中，每个 Tick 执行以下固定阶段：
1. **收集 (Intent Collection)**: 从 WebSocket 或 HTTP 队列收集所有存活 Agent 提交的 `Intent`。
2. **演化 (State Processor)**: 包含自然环境衰减、公式计算（如饥饿、口渴、寿命减少等）。
3. **结算 (Action Executor)**: 对收集到的意图按优先级排序并执行校验，合法的动作将改变内存中的 Agent 状态。
4. **持久化 (Persistence)**: 将变动后的状态快照批量写入 PostgreSQL。
5. **广播 (Broadcaster)**: 将组装好的 `WorldState` 下发给所有建立 WebSocket 连接的客户端。

### 2. 数据驱动系统 (game_data/)
抛弃硬编码，所有游戏逻辑均由 `crates/server/config/*.yaml` 定义：
- `GameDataLoader` 在启动时加载 `actions.yaml`、`attributes.yaml`、`locations.yaml` 等。
- 提供 `ActionRegistry`, `ItemRegistry` 等内存访问接口供 Tick 引擎高速读取。
- **公式引擎 (`formula_engine`)**: 允许在配置文件中编写简单的表达式（如 `base_damage * (1 + strength * 0.1)`），服务端在运行时动态求值。

### 3. HTTP API 接口 (handlers/)
除了 WebSocket，服务端提供以下核心 RESTful API (默认端口 `23333`)：
- `POST /api/v1/agent/register`: 注册新的 Agent，返回初始属性、背包和身份 Token。
- `POST /api/v1/intent`: （HTTP 模式下）接收单次 Intent 提交。
- `POST /api/v1/validate`: （HTTP 模式下）预验证动作的合法性，不实际执行。
- `GET /api/v1/context`: 返回指定 Agent 的叙事上下文。
- `GET /api/config`: 列出当前服务端加载的数据配置。

### 4. 动作执行与验证 (actions/)
- `validator.rs`: 根据配置表中的 `requirements`（如消耗体力、需要在特定地点）对 Intent 进行检查。
- `executor/`: 动作执行的具体实现。例如 `combat.rs` 处理 `Attack` 逻辑并计算伤害，生成 `WorldEvent`；`basic.rs` 处理移动和空闲逻辑。

## 开发指南

1. **新增动作 (Action)**:
   - 在 `protocol` 中定义新的 `ActionType`。
   - 在 `config/actions.yaml` 中配置其参数要求和消耗。
   - 在 `server/src/actions/executor/` 中实现执行逻辑，并将结果推入事件队列。
2. **修改属性与公式**:
   - 直接修改 `config/attributes.yaml` 或包含公式的 YAML 配置。
   - 重启服务端（或触发热更新 API）即可生效。
3. **日志与排错**:
   - 保持“大声失败 (Fail Fast)”原则，在数据加载失败或约束违背时直接 panic 退出，拒绝运行在错误状态。
