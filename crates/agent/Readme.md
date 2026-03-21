# Cyber-Jianghu Agent SDK

Agent SDK 是连接赛博江湖服务端的桥梁。它为开发者提供了与游戏世界交互的基础设施，并且内置了记忆、认知、对话等高级 AI 模块，方便快速构建拥有独立思考能力的"赛博侠客"。

## 架构概览

```
crates/agent/src/
├── transport/              # 纯 I/O 通信层
│   └── websocket.rs        # WebSocket 客户端（无业务逻辑）
├── core/                   # Agent 核心控制
│   ├── agent.rs            # Agent 主体结构与生命周期
│   ├── builder.rs          # Agent 构建器（ fluent API）
│   └── lifecycle.rs        # 连接/重连/无限重试逻辑
├── runtime/                # 运行时决策模式
│   └── decision/
│       ├── http/           # HTTP API 模式（供 OpenClaw 调用）
│       ├── ws/             # WebSocket 模式（实时推送）
│       └── cognitive/      # 内置认知决策模式
├── ai/                     # AI 增强模块
│   ├── llm/                # LLM 客户端（直连 / OpenClaw）
│   ├── cognitive/          # 叙事引擎（数值→自然语言）
│   ├── memory/             # 三层记忆系统
│   ├── persona/            # 动态人设（性格演变）
│   ├── validator/          # 意图验证器
│   ├── dialogue/           # 对话客户端
│   ├── relationship/       # 人际关系管理
│   └── lifespan/           # 寿命计算
├── config.rs               # 配置管理
├── models.rs               # 数据模型（re-export from protocol）
└── bin/                    # CLI 入口
    └── cyber-jianghu-agent.rs
```

## 快速开始

### 安装

```bash
# 从源码构建
cargo install --path crates/agent

# 或直接构建
cargo build -p cyber-jianghu-agent --release
```

### 基本使用

```bash
# Claw 模式（推荐 OpenClaw 集成）
cyber-jianghu-agent run --mode claw --port 23340

# Cognitive 模式（内置 AI 决策）
cyber-jianghu-agent run --mode cognitive

# 查看当前配置
cyber-jianghu-agent show

# 设置服务器地址
cyber-jianghu-agent config --ws-url ws://your-server:23333/ws

# 重置身份（清除 device_id）
cyber-jianghu-agent reset
```

## 运行模式

### Claw 模式（推荐）

**适用场景**: OpenClaw 集成、外部 LLM 调用、自定义决策逻辑

启动混合服务（HTTP API + WebSocket），提供 RESTful 接口供外部调用。

```bash
cyber-jianghu-agent run --mode claw --port 23340
```

**服务组件**:
- **HTTP API**: `http://localhost:23340/api/v1/*`
- **WebSocket**: `ws://localhost:23340/ws`（实时 tick 推送）
- **Web 面板**: `http://localhost:23340/`

#### HTTP API 端点

| 端点 | 方法 | 说明 |
|------|------|------|
| `/api/v1` | GET | API 发现（返回所有端点及示例） |
| `/api/v1/health` | GET | 健康检查 |
| `/api/v1/state` | GET | 获取当前 WorldState |
| `/api/v1/context` | GET | 获取叙事上下文（Markdown，推荐 LLM 使用） |
| `/api/v1/attributes` | GET | 梦境一瞥：获取属性值（禁止存储！） |
| `/api/v1/tick` | GET | 获取当前 tick 状态（用于轮询） |
| `/api/v1/intent` | POST | 提交决策意图 |
| `/api/v1/validate` | POST | 预验证动作合法性 |

**角色管理**:
| 端点 | 方法 | 说明 |
|------|------|------|
| `/api/v1/character` | GET | 获取角色信息 |
| `/api/v1/character/experiences` | GET | 获取经历日志（分页） |
| `/api/v1/character/dream` | GET/POST | 梦境注入（每游戏日 1 次） |
| `/api/v1/character/rebirth` | POST | 转世重生（删除角色） |
| `/api/v1/character/register` | POST | 创建角色 |

**记忆系统**:
| 端点 | 方法 | 说明 |
|------|------|------|
| `/api/v1/memory/recent` | GET | 获取最近记忆 |
| `/api/v1/memory/search` | POST | 搜索记忆 |
| `/api/v1/memory` | POST | 存储记忆 |

**关系系统**:
| 端点 | 方法 | 说明 |
|------|------|------|
| `/api/v1/relationship/list` | GET | 获取所有关系 |
| `/api/v1/relationship/{id}` | GET | 获取特定关系 |
| `/api/v1/relationship` | POST | 更新关系 |

**寿命系统**:
| 端点 | 方法 | 说明 |
|------|------|------|
| `/api/v1/lifespan` | GET | 获取寿命状态 |

**配置管理**:
| 端点 | 方法 | 说明 |
|------|------|------|
| `/api/v1/config` | GET | 获取当前配置 |
| `/api/v1/config/reload` | POST | 热重载配置 |
| `/api/v1/config/server` | POST | 设置服务器 URL（触发重连） |

**审核系统**（Player/Observer 模式）:
| 端点 | 方法 | 说明 |
|------|------|------|
| `/api/v1/review/pending` | GET | 获取待审核意图 |
| `/api/v1/review/{id}` | POST | 提交审核结果 |
| `/api/v1/review/{id}/status` | GET | 获取审核状态 |

#### WebSocket 协议

**下行（Agent → 外部）**:
```json
{"type": "tick", "tick_id": 105, "deadline_ms": 1710937800000, "state": {...}, "context": "..."}
{"type": "tick_closed", "tick_id": 105, "reason": "timeout", "next_tick_in_ms": 15000}
```

**上行（外部 → Agent）**:
```json
{"type": "intent", "tick_id": 105, "action_type": "move", "action_data": {...}, "thought_log": "..."}
```

### WebSocket 模式

**适用场景**: 实时交互、低延迟决策、流式处理

启动 WebSocket 服务器，实时推送 tick 并接收意图。

```bash
cyber-jianghu-agent run --mode ws --port 23341
```

**特性**:
- 实时 tick 广播
- 自动意图过期处理（过期 tick_id 静默丢弃）
- Ping/Pong 心跳
- 优雅断开

### Cognitive 模式

**适用场景**: 独立运行、无外部依赖、完整 AI 体验

Agent 内置完整认知流水线，自主决策。

**认知流程**:
1. **Perception（感知）**: 解析 WorldState，生成自然语言描述
2. **Memory Retrieval（记忆检索）**: 检索相关情景与语义记忆
3. **Motivation（动机）**: 结合 Persona 推断短期/长期动机
4. **Planning（规划）**: 制定行动计划
5. **Decision（决策）**: 组装 Prompt，调用 LLM 生成 Intent
6. **Validation（验证）**: 检查幻觉/非法动作，必要时重试

## 连接机制

### 初始连接

**无限重试**: 首次连接失败后无限重试，5 秒间隔，日志采样输出（前 5 次，之后 9、16、25...）

### 断线重连

**指数退避**: 1s → 2s → 4s → ... → max(tick_duration/2)

成功后重置退避计数器。

### 热重载

通过 API 或 CLI 动态切换服务器地址，无需重启：

```bash
# CLI
cyber-jianghu-agent config --ws-url ws://new-server:23333/ws

# API
curl -X POST http://localhost:23340/api/v1/config/server \
  -H "Content-Type: application/json" \
  -d '{"ws_url": "ws://new-server:23333/ws", "http_url": "http://new-server:23333"}'
```

## AI 模块

### 记忆系统

三层架构 + 可选语义层：

```
┌─────────────────────────────────────────────┐
│              Working Memory                  │
│   最近 N 条事件（默认 20），内存环形缓冲      │
└─────────────────────────────────────────────┘
                      ↓ importance >= threshold
┌─────────────────────────────────────────────┐
│              Episodic Memory                 │
│   重要事件，SQLite 持久化，艾宾浩斯遗忘       │
└─────────────────────────────────────────────┘
                      ↓ Ebbinghaus forgetting
┌─────────────────────────────────────────────┐
│              Archive Memory                  │
│   被遗忘的记忆，可通过努力召回                │
└─────────────────────────────────────────────┘
```

**艾宾浩斯遗忘**: 每 84 tick 执行一次，公式 `R(t) = e^(-t/S)`

### 动态人设

基于大五人格 + 自定义特征：

```rust
pub enum TraitType {
    Openness,        // 开放性
    Conscientiousness, // 尽责性
    Extraversion,    // 外向性
    Agreeableness,   // 宜人性
    Neuroticism,     // 神经质
    // 自定义特征
}
```

**特征演变**: 事件触发特征变化，随时间衰减回归基准

### 意图验证器

规则引擎验证 + LLM 辅助判断：

```rust
pub enum ValidationResult {
    Approved { reason: String, narrative: String },
    Rejected { reason: String, rejection_type: RejectionType },
}
```

**验证循环**: 最多 5 次重试，连续 3 次拒绝 → 强制 idle

### 人际关系

```rust
pub struct RelationshipMemory {
    target_id: Uuid,
    familiarity: f32,    // 0-1 熟悉度
    affection: f32,      // -1 to 1 好感度
    trust: f32,          // 0-1 信任度
    key_events: Vec<KeyEvent>,
    narrative: Option<String>, // AI 生成描述
}
```

**叙事更新**: 防抖设计，LLM 生成描述，SQLite 缓存

### 寿命计算

```rust
pub enum AgingStage {
    Youth,   // 16-25
    Prime,   // 26-45
    Middle,  // 46-65
    Elderly, // 66+
}
```

每个 tick 根据年龄减少属性，死亡风险随年龄增加。

## Web 面板

Claw 模式自带 Web 管理界面：

| 页面 | 路径 | 功能 |
|------|------|------|
| 角色创建 | `/` 或 `/index.html` | 创建新角色 |
| 角色信息 | `/character.html` | 查看属性、背包、经历 |
| 管理面板 | `/manage.html` | 梦境注入、转世重生 |

## 配置

配置文件位于 `~/.config/cyber-jianghu/agent.yaml`：

```yaml
identity:
  device_id: "uuid"
  auth_token: "token"

server:
  ws_url: "ws://localhost:23333/ws"
  http_url: "http://localhost:23333"

runtime:
  mode: "claw"
  port: 23340

memory:
  enabled: true
  working_capacity: 20
  episodic_threshold: 0.5

role: "player"  # player | observer
```

## Docker 部署

```bash
# 构建镜像
docker build -t cyber-jianghu-agent -f crates/agent/Dockerfile .

# 运行容器
docker run -d \
  -p 23340:23340 \
  -e SERVER_WS_URL=ws://host.docker.internal:23333/ws \
  -e SERVER_HTTP_URL=http://host.docker.internal:23333 \
  cyber-jianghu-agent
```

**环境变量**:
| 变量 | 说明 |
|------|------|
| `SERVER_WS_URL` | 游戏服务器 WebSocket 地址 |
| `SERVER_HTTP_URL` | 游戏服务器 HTTP 地址 |
| `AGENT_MODE` | 运行模式（claw/cognitive） |
| `AGENT_PORT` | HTTP API 端口 |

## 扩展开发

### 自定义记忆后端

在 `ai/memory/backends/` 实现 `MemoryBackend` trait：

```rust
#[async_trait]
pub trait MemoryBackend: Send + Sync {
    async fn store(&self, memory: &MemoryEntry) -> Result<()>;
    async fn retrieve(&self, query: &MemoryQuery) -> Result<Vec<MemoryEntry>>;
    async fn forget(&self, id: &Uuid) -> Result<()>;
}
```

### 新增决策模式

在 `runtime/decision/` 实现决策逻辑：

```rust
pub trait DecisionCallback: Send + Sync {
    async fn decide(&self, state: &WorldState) -> Result<Intent>;
}
```

### 自定义验证器

实现 `Validator` trait：

```rust
#[async_trait]
pub trait Validator: Send + Sync {
    async fn validate(&self, request: ValidationRequest) -> Result<ValidationResult>;
    async fn update_rules(&self, rules: WorldBuildingRules);
}
```

## 与 OpenClaw 集成

详见 [Cyber-Jianghu-Openclaw](https://github.com/8kugames/Cyber-Jianghu-Openclaw)

**核心流程**:
1. Agent 启动 Claw 模式，监听 HTTP API
2. OpenClaw 通过 `GET /api/v1/context` 获取叙事上下文
3. OpenClaw 决策后通过 `POST /api/v1/intent` 提交意图
4. Agent 转发意图到游戏服务器

## 许可证

AGPL-3.0 License
