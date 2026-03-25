# Cyber-Jianghu 已实现功能摘要 (Feature Summary)

本文档旨在为开发者提供当前赛博江湖 (Cyber-Jianghu) 架构中**已实际实现并可用**的核心业务功能摘要。所有的模块都遵循数据驱动设计，并通过 Tick 引擎和 Agent SDK 进行交互。

## 一、 服务端 (天道引擎) 功能

服务端充当整个游戏世界的"物理引擎"与规则仲裁者。

### 1. 核心运行机制
- [x] **Tick 驱动系统**: 实现了基于固定频率（可配置 TPS）的时间流转循环，处理意图收集、自然衰减（饥饿、口渴、环境伤害）、动作结算、状态持久化与状态广播。
- [x] **动态公式引擎**: 实现了基于 AST 的自研公式求值引擎 (`formula_engine`)，允许在配置文件中直接编写数学表达式并绑定 Agent 属性进行动态计算。
- [x] **数据驱动配置**: 所有核心数据（属性定义、物品、地图位置、动作定义、叙事配置等）均通过 YAML 文件在启动时加载，支持热更新。

### 2. 实体与状态管理
- [x] **Agent 生命周期**: 支持新 Agent 降生（注册）、属性初始化、存活状态维护以及寿命自然衰减与死亡判定。
- [x] **持久化系统**: 集成 PostgreSQL 数据库，实现 Agent 基础数据、实时状态（Health/Energy等）、以及场景掉落物的持久化读写。
- [x] **状态广播**: 实现了基于 WebSocket 的高性能 `WorldState` 广播机制，将局部或全局世界快照实时同步给各 Agent 客户端。
- [x] **agent_id → device_id 反向映射**: 维护角色到设备的映射关系，确保广播消息能正确路由到 WebSocket 连接。

### 3. 动作与交互系统
- [x] **基础行为**: 实现了 `Idle` (空闲)、`Move` (基于地理图节点的移动校验)。
- [x] **战斗系统**: 实现了基础的 `Attack` (攻击) 动作，包含伤害计算、扣血逻辑及死亡事件生成。
- [x] **物品与背包**:
  - 支持地上的物品拾取 (`Gather`)。
  - 背包容量校验、物品消耗。
  - 支持各类物品定义（武器、消耗品、任务道具）。
- [x] **对话系统**: 实现了完整的对话生命机会管理（请求、接受、拒绝、内容传输、结束），并在服务端作为中间人进行消息路由与验证。

## 二、 客户端 SDK (Agent) 功能

Agent SDK 是接入世界的"躯壳"，外部 LLM（如 openclaw）作为"大脑"。

### 1. 接入与运行模式
- [x] **Claw 模式**: Agent 默认模式，为 OpenClaw 和其他外部 LLM 编排框架提供 WebSocket + HTTP API 接口。内置叙事引擎、记忆系统、意图验证等认知能力作为 API 供外部调用。
- [x] **网络通信容错**: 实现了 WebSocket 自动断线重连、指数退避策略以及注册流的自动恢复。
- [x] **强制 WebSocket Intent 提交**: HTTP `/api/v1/intent` 已禁用，必须通过 WebSocket 提交意图以确保 Tick 同步。

### 2. AI 与认知核心 (内置模块)
- [x] **多级记忆系统**:
  - [x] **工作记忆**: 短期上下文维持 (FIFO 队列，支持限制最大条目数)。
  - [x] **情景记忆**: 结合时间的事件流水账 (SQLite 持久化)。
  - [x] **语义记忆**: 实现了本地向量存储（Embedder）和全文检索 (FTS) 回退机制，用于存取世界观知识和过往经验。
- [x] **认知感知流水线**:
  - [x] **叙事翻译**: 将生硬的数值（如 `health: 30%`）转化为自然语言描述（如"你感到头晕目眩，身负重伤"），方便 LLM 理解。
  - [x] **动机推演**: 基于当前状态和性格，自动推断出下一步应当采取的短期动机。
- [x] **四阶段认知上下文**: WebSocket Tick 消息内置结构化认知引导
  - [x] **Perception (感知)**: 理解当前世界状态、自身状态、环境观察
  - [x] **Motivation (动机)**: 基于人设生成内在驱动力
  - [x] **Planning (规划)**: 制定行动计划和可用动作
  - [x] **Decision (决策)**: 引导最终决策的思考提示
- [x] **动态人设与社交**:
  - [x] 角色性格 (`Persona`) 能够根据外界反馈（被攻击、被治愈）进行动态偏移。
  - [x] 支持建立与其他 Agent 的好感度/信任度关系图谱，并支持查询与修改。
- [x] **意图验证器** (HTTP API 可用，WebSocket 链路已自动集成):
  - [x] 基于规则引擎和 LLM 二次校验的拦截器已实现 (`ai/validator/`)
  - [x] HTTP API `POST /api/v1/validate` 可供 OpenClaw 主动调用
  - [x] WebSocket intent 提交链路自动调用验证器（10秒超时降级策略）
  - [x] 验证失败返回 `ServerError{ValidationFailed}` 允许在剩余 tick 时间内重试
  - [ ] Observer Agent 审查系统未接入 intent 提交链路

- [x] **Observer Agent 审查系统** (API 已实现，独立于 intent 链路):
  - [x] `GET /api/v1/review/pending` - Observer Agent 轮询待审查意图
  - [x] `POST /api/v1/review/{intent_id}` - 提交审查结果 (批准/拒绝)
  - [x] `GET /api/v1/review/{intent_id}/status` - 查询审查状态
  - [x] 超时自动通过机制已实现 (`ReviewStore::process_timeouts`)
  - [ ] Player Agent intent 提交未经过审查流程（需 OpenClaw 自行编排）

## 三、 通信协议 (Protocol)

- [x] 实现了统一的 Rust 数据结构，并通过 Serde 提供全套的 JSON 序列化/反序列化。
- [x] **消息边界**: 明确区分 `ServerMessage`（服务端下发）和 `ClientMessage`（客户端上报），涵盖世界状态、动作意图、热更通知等。

## 四、 扩展支持 (OpenClaw 集成)
- [x] OpenClaw 集成已独立发布为 npm 包 `@8kugames/cyber-jianghu-openclaw`。
- [x] 实现了 `jianghu_act` 动作执行工具、注册 Hook 以及内存插件，支持外部 AI Agent 零代码接入赛博江湖。
- [x] WebSocket Tick 消息内置四阶段认知上下文，引导 OpenClaw 进行结构化推理。
- [x] 详见 [8kugames/Cyber-Jianghu-Openclaw](https://github.com/8kugames/Cyber-Jianghu-Openclaw)。

---

## 五、 设备与角色系统 (Phase 3 重构)

实现了设备身份（Device）与角色身份（Agent）的完全分离，支持归隐转生机制。

### 1. 设备管理
- [x] **设备注册**: 新增 `/api/v1/agent/connect` 端点，用于设备首次连接时获取身份凭证。
- [x] **设备持久化**: 设备信息存储在 `devices` 表中，包含 `auth_token` 和 `last_seen`。
- [x] **设备认证**: WebSocket 连接现在基于 `device_id` 进行验证，而非角色 Token。

### 2. 角色管理
- [x] **角色注册**: 新增 `/api/v1/agent/register` 端点，设备可为自身创建多个角色。
- [x] **归隐机制**: 角色死亡后可通过 `/api/v1/agent/rebirth` 标记为 `retired` (归隐)，保留历史数据供查看，然后创建新角色。
- [x] **角色绑定**: 角色通过 `device_id` 关联到设备，支持一个设备管理多个角色。

### 3. Web 管理面板
- [x] **角色创建页面**: `GET /` 提供可视化角色创建界面。
- [x] **角色信息页面**: `GET /character.html` 展示角色属性、背包、经历等信息。
- [x] **管理页面**: `GET /manage.html` 支持梦境注入和转生操作。

## 六、 生产部署 (Phase 4)

### 1. Docker 部署优化
- [x] **自动迁移**: 容器启动时自动执行数据库迁移文件 (`/app/migrations/*.sql`)。
- [x] **Token 自动生成**: 当 `ADMIN_READ_TOKEN` 或 `ADMIN_WRITE_TOKEN` 为空时自动生成随机 Token。
- [x] **版本管理脚本**: 新增 `scripts/version-bump.sh` 自动检测变更并升级版本号。

### 2. 配置优化
- [x] **空值过滤**: Token 环境变量增加空字符串过滤，避免空值被误认为有效配置。
- [x] **数据库等待**: 容器启动时等待 PostgreSQL 就绪后再执行迁移。
