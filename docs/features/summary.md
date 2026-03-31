# Cyber-Jianghu 已实现功能摘要 (Feature Summary)

 本文档为开发者提供当前赛博江湖架构中**已实际实现并可用**的核心业务功能摘要。所有模块遵循数据驱动设计，通过 Tick 引擎和 Agent SDK 交互。

 ## 一、 服务端 (天道引擎)

 ### 1. 核心运行机制
 - [x] **Tick 驱动**: 可配置 TPS 循环（可配置），执行意图收集、衰减结算（饥饿/口渴/环境伤害）、动作执行、状态持久化与广播。
 - [x] **tick_id 秒级时间戳**: `tick_id` 改为 Unix 秒级时间戳，支持 `real_seconds_per_tick` 动态调整游戏时间流速。
 - [x] **公式引擎**: `evalexpr`（战斗伤害等运行时计算）+ 自研 AST 引擎（派生属性动态计算），均支持 YAML 编写表达式。
 - [x] **数据驱动配置**: 所有核心数据（属性、物品、地图、动作、叙事配置等）通过 YAML 加载，支持热更新 (`POST /api/admin/reload-config`)。

### 2. 实体与状态管理
- [x] **Agent 生命周期**: 注册降生、属性初始化、存活状态维护、寿命衰减与死亡判定。
- [x] **持久化**: PostgreSQL 存储 Agent 基础数据、实时状态、场景掉落物。
- [x] **状态广播**: 基于 WebSocket 的 `WorldState` 广播，维护 `agent_id -> device_id` 反向映射确保路由正确。

### 3. 动作与交互系统（数据驱动）

动作系统完全由 `crates/server/config/actions.yaml` 定义，无需修改代码即可新增或修改动作。

**Tick 结算流水线**（8 阶段）:
```
意图收集 --> 验证 --> 冲突解析 --> 执行 --> 状态变更 --> 衰减 --> 广播 --> 持久化
```

- **意图收集**: 仅接受当前 tick_id 的意图，过期拒绝
- **验证**: `IntentValidator` 检查动作合法性、属性充足性、目标状态
- **冲突解析**: `IntentResolver` 处理位置冲突和资源竞争
- **执行**: `ActionExecutor` 根据 `actions.yaml` 中的定义执行动作
- **状态变更**: `AttributeMutator`/`InventoryMutator`/`LocationMutator` 等 `StateMutator` 应用变更
- **衰减**: 饥饿、口渴、物品耐久等被动损耗
- **广播**: `WorldState` 推送所有 Agent
- **持久化**: 写入 PostgreSQL

**动作定义结构**（`actions.yaml`）:
```yaml
attack:
  description: "攻击目标，造成伤害"
  damage_formula: "10 + strength * 0.5 + weapon_bonus * weapon_multiplier"
  validation:
    requires_target: true
    requires_target_alive: true
    required_fields: [target_agent_id]
  requirements:
    - attribute: stamina
      min: 5
      cost: 5
```

`ActionType` 为字符串包装（任何字符串均有效），扩展动作只需编辑 YAML。

**已实现动作**: `idle`, `move`, `attack`, `gather`, `speak`, `whisper`, `steal`, `give`, `discard`, `use_item`, `equip`, `unequip`, `dialogue_request`, `dialogue_accept`, `dialogue_reject`, `dialogue_end`

**物品与背包**: 地上物品拾取、背包容量校验、物品消耗（武器/消耗品/任务道具）

**对话系统**: 完整的对话生命周期管理（请求/接受/拒绝/内容传输/结束），服务端作为中间人路由与验证。

## 二、 Agent SDK (众生躯壳)

支持内置 LLM 自主决策（Cognitive 模式）或外部调度（Claw 模式）。

### 1. 运行模式
 - [x] **Cognitive 模式**（默认）: 内置多阶段认知引擎，Agent 完全自主。
 - [x] **Claw 模式**: WebSocket + HTTP API 供 OpenClaw 接入，内置认知能力作为 API。
 - [x] **默认 LLM**: 默认改为 openclaw，支持 ollama 自定义端口配置。
 - [x] **网络容错**: WebSocket 自动断线重连、指数退避、注册流自动恢复。
 - [x] **WebSocket 心跳**: 内置 Ping/Pong 消息机制，保持连接活跃。
 - [x] **LLM 开关闸**: 紧急停止 token 消耗的控制机制，Web 面板可操作。

### 2. AI 与认知核心

**三层记忆系统**:
- [x] **工作记忆**: FIFO 短期上下文队列（最大条目数可配置）
- [x] **情景记忆**: SQLite 持久化，带时间戳的事件流
- [-] **语义记忆**:
  - [x] FTS 全文搜索（返回完整记忆条目）
  - [x] HNSW 向量索引基础设施（Embedder + `instant-distance`）
  - [ ] `add()` 为 no-op（语义记忆由 episodic 后端通过向量生成写入，非 direct add）
  - [ ] `ensure_embeddings_for_priority()` 未实现
  - [ ] `ensure_embedding(memory_id)` 未实现

**四阶段认知流水线**（Cognitive 模式内置运行，Claw 模式通过 WebSocket Tick 消息下发）:
 - **Perception**: 数值状态 → 叙事化自然语言
 - **Motivation**: 基于人设推断内在驱动力
 - **Planning**: 制定行动计划与可用动作
 - **Decision**: 引导最终行动决策
 - [x] **合并优化**: Perception + Motivation + Planning 合并为一次 LLM 调用，减少 token 消耗
 - [x] **persona 缓存**: 认知引擎缓存人设，减少重复计算
 - [x] **deadline 感知**: 认知引擎感知 tick 截止时间，避免过期被拒

**叙事引擎**: 将生硬数值（`health: 30%`）转化为自然语言（"身负重伤、头晕目眩"），方便 LLM 理解。

 **动态人设**: `Persona` 根据外界反馈（被攻击/被治愈）动态偏移；支持好感度/信任度关系图谱。

 **双 Soul 架构**:
 - ActorSoul (行动之魂/本我): 生成意图，执行行动
 - ReflectorSoul (反思之魂/超我): 审查意图，道德判断（默认启用）
 - `ReviewStore` 共享内存用于进程内审查通信

### 3. 意图控制（双层架构）

**第一层: 规则/LLM 验证**（接入 intent 提交链路）:
 - [x] 规则引擎验证器 (`RuleEngine`)，HTTP API `POST /api/v1/validate`
 - [x] LLM 验证器 (`IntentValidator`)，10 秒超时降级策略，驳回后返回 `ServerError{ValidationFailed}`
 - [x] Cognitive 路径: 决策 → 验证 → 驳回 → `think_with_feedback(feedback)` 重试（验证器与认知引擎共用 `llm_arc`）
 - [x] **ActorSoul + ReflectorSoul LLM 独立配置**: 新增 `llm_reflector` 字段
 - [x] **LLM 配置热重载**: 文件监听自动热重载 + API Key 验证 + zeroize 内存安全

**第二层: 超我审查**（ActorSoul + ReflectorSoul，进程内双 Soul 架构）:
 - [x] `ActorSoul.submit_for_review()` 在 `lifecycle.rs:296-300` 被调用，intent 经审查后再发送
 - [x] `ReflectorSoul` 后台任务每 5 秒轮询 `ReviewStore`，超时 30 秒自动通过
 - [x] **ActorSoul + ReflectorSoul LLM 独立配置**: `llm_reflector` 字段支持独立配置审查 LLM
 - [x] 远程 Observer 模式已移除（HTTP 轮询 + 协议层 `ReviewRequest`/`ReviewResult` 均已删除）
 - [x] 审查系统 API 仅供监控工具使用: `GET /api/v1/review/pending`、`POST /api/v1/review/{intent_id}`、`GET /api/v1/review/{intent_id}/status`

## 三、 通信协议 (Protocol)

- [x] 统一 Rust 数据结构，Serde JSON 序列化/反序列化
- [x] 明确区分 `ServerMessage`（下行）和 `ClientMessage`（上行），涵盖世界状态、动作意图、热更通知

## 四、 OpenClaw 集成

 - [x] npm 包 `@8kugames/cyber-jianghu-openclaw` 已独立发布
 - [x] 提供 `jianghu_act` 动作执行工具、注册 Hook、内存插件
 - [x] Claw 模式 WebSocket Tick 消息携带四阶段认知上下文，引导外部 AI 结构化推理
 - [x] WebSocket 心跳机制（Ping/Pong）保持连接活跃
 - [x] LLMRequest 消息字段: `llm_request`（注意：旧版本为 `l_l_m_request`）
 - [x] 详见 [8kugames/Cyber-Jianghu-Openclaw](https://github.com/8kugames/Cyber-Jianghu-Openclaw)

---

## 五、 设备与角色系统（Phase 3）

### 1. 设备管理
- [x] `/api/v1/agent/connect` 端点获取设备身份凭证
- [x] `devices` 表持久化（`auth_token` + `last_seen`）
- [x] WebSocket 双重验证: `device_id` + `auth_token`

### 2. 角色管理
- [x] `/api/v1/agent/register` 创角，支持一设备多角色
- [x] `/api/v1/agent/rebirth` 归隐机制：死亡角色标记 `retired`，保留历史数据

### 3. Web 管理面板
 - [x] `GET /admin/` → Admin Dashboard 入口（agent 列表、统计数据）
 - [x] `GET /` → Web 面板入口（角色创建/信息/管理导航）
 - [x] `GET /character.html` → 角色属性、背包、经历
 - [x] `GET /manage.html` → 梦境注入与转生

## 六、 生产部署（Phase 4）

- [x] 容器启动自动执行数据库迁移（`/app/migrations/*.sql`）
- [x] `ADMIN_READ_TOKEN`/`ADMIN_WRITE_TOKEN` 空值自动生成
- [x] `scripts/version-bump.sh` 自动检测变更并升级版本号
- [x] PostgreSQL 就绪等待机制

---

## 七、 待实现功能 (TODO)

### 服务端

- [ ] **物品自然损坏**: `crates/server/src/tick/decay.rs:219-227`
  - 基础设施已就绪（`AgentItem.durability`、`agent_inventory.durability` 列、`ItemDefinition.max_durability/decay_rate` 类型定义）
  - 需要实现：衰减逻辑（查询背包 → 扣减耐久 → 移除物品 → 发送通知）+ `items.yaml` 配置衰减率

### Agent SDK

- [ ] **语义记忆向量生成**: `crates/agent/src/component/memory/backends/semantic/backend.rs`
  - 基础设施已就绪（HNSW 向量索引 + FTS fallback + LocalEmbedder）
  - 需要实现：`SemanticMemoryBackend::add()` 空操作 → 改为真正写入向量存储
  - 需要实现：`ensure_embeddings_for_priority()` stub → 实现优先级记忆的向量生成
  - 需要实现：`ensure_embedding(memory_id)` stub → 实现单个记忆的向量生成

- [ ] **记忆归档与强度更新**: `crates/agent/src/component/memory/backends/episodic.rs:166-178`
  - 已实现：Ebbinghaus 遗忘曲线计算、重要性评分器
  - 需要实现：`archive_memories()` stub → 改为调用 `ArchiveMemoryBackend::archive()` 真正移动到归档表
  - 需要实现：`strengthen_memory()` stub → 需要 `MemoryStore` schema 支持 `strength`/`access_count`/`last_accessed_at` 列
