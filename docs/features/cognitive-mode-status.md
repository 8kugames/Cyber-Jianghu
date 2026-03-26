# Cyber-Jianghu Agent 实现状态报告

**日期**: 2026-03-26
**分支**: dev
**状态**: 双 Soul 架构实现完成

---

## 一、核心架构

### 设计原则

**COI (Composition Over Inheritance)**：HTTP API 是独立可组合组件，Cognitive 和 Claw 模式的区别仅在于 LLM（大脑）在内还是外。

**ActorSoul + ReflectorSoul 架构**：单进程双 Soul 设计，通过共享内存（ReviewStore）进行通信。

```
┌─────────────────────────────────────────────────────────────┐
│                    Cyber-Jianghu Agent                       │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│   shared ReviewStore ◄─────────────────────────────────────┐│
│         │                                                    ││
│         │ (内存共享)                                          ││
│         ▼                                                    ││
│   ┌─────────────────────────────────────────────────────┐   ││
│   │                Agent (主体)                             │   ││
│   │                                                      │   ││
│   │  ActorSoul (行动之魂) │ ReflectorSoul (反思之魂)    │   ││
│   │  ───────────────────┼───────────────────────────    │   ││
│   │  1. decide() ──► Intent │                          │   ││
│   │  2. submit_for_review() │  poll ReviewStore         │   ││
│   │                      │◄──── LLM 审查 ────────┐    │   ││
│   │  3. await approval ◄───┘                          │   ││
│   │  4. send_intent() ──► Server                      │   ││
│   └─────────────────────────────────────────────────────┘   ││
│                                                              │
│   ┌─────────────────────────────────────────────────────┐   │
│   │              HTTP API Server (辅助功能)               │   │
│   │                                                       │   │
│   │   GET /api/v1/state      - 状态查询                   │   │
│   │   GET /api/v1/context   - 上下文                      │   │
│   │   POST /api/v1/character/dream - 托梦注入             │   │
│   │   GET/POST /api/v1/review/* - 审查系统               │   │
│   │                                                       │   │
│   │   ⚠️ /api/v1/intent 已禁用（强制 WebSocket）         │   │
│   └─────────────────────────────────────────────────────┘   │
│                                                              │
│   ┌─────────────────────────────────────────────────────┐   │
│   │              WebSocket (主通道)                      │   │
│   │                                                       │   │
│   │   ◄── WorldState (tick 同步)                        │   │
│   │   ──► Intent (决策提交)                             │   │
│   └─────────────────────────────────────────────────────┘   │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

### 运行时模式对比

| 特性 | Cognitive 模式 | Claw 模式 |
|------|---------------|-----------|
| LLM 位置 | **内置** (Agent 内部) | **外置** (OpenClaw) |
| HTTP API | ✅ 完整支持 | ✅ 完整支持 |
| WebSocket | ✅ Agent ↔ Server | ✅ Agent ↔ Server |
| 托梦注入 | ✅ | ✅ |
| ReflectorSoul 审查 | ✅ **默认启用** | ✅ **默认启用** |
| 适用场景 | 独立运行、低延迟 | 复杂推理、外部大脑 |

### Soul 术语说明

| 概念 | 代码命名 | 含义 |
|------|----------|------|
| ActorSoul | 行动之魂/本我 | 生成意图，执行行动 |
| ReflectorSoul | 反思之魂/超我 | 审查意图，道德判断 |

**架构简化**：远程 Observer 模式已移除，仅保留进程内双 Soul 架构（通过 ReviewStore 共享内存）。

---

## 二、已实现功能

### 2.1 Cognitive 模式 (内置 LLM)

| Commit | 描述 |
|--------|------|
| `eaef0b3` | feat(agent): integrate Cognitive mode as default runtime |
| `2e3fa6f` | fix(agent): address code review findings |
| `d93b988` | docs: update CLAUDE.md for Cognitive mode as default |
| `d53e46c` | feat(agent): improve Cognitive mode for self-sufficient operation |
| `d70cbbb` | feat(agent): add Observer mode for intent review |
| `b94132b` | feat(agent): add --with-observer for dual agent orchestration |
| `126fe6a` | refactor(agent): HTTP API now available in Cognitive mode |
| **最新** | **feat: ActorSoul + ReflectorSoul 双 Soul 架构** |

### 2.2 核心组件

#### ClawDecisionState (`runtime/claw/decision.rs`)
- 持有 LLM Client + ContextBuilder
- `with_system_prompt()` 组合 persona + 决策规则
- `claw_decision()` 执行 LLM 调用 → JSON 解析 → Intent

#### ContextBuilder (`runtime/claw/context.rs`)
- WorldState → LLM Prompt 格式化
- 参考 ZeroClaw 的 History Management 设计
- 防止 context overflow

#### TurnCycle (`runtime/claw/turn_cycle.rs`)
- 参考 ZeroClaw 的 `run_tool_call_loop()` 设计
- 单次 LLM 决策循环

#### HistoryManager (`runtime/claw/history.rs`)
- 参照 ZeroClaw 的 History Management 设计
- Auto-compaction (LLM 摘要)
- FIFO eviction + 严格消息限制

### 2.3 HTTP API 端点

```
# 基础
GET  /api/v1               - API 发现
GET  /api/v1/health        - 健康检查
GET  /api/v1/state         - WorldState
GET  /api/v1/context       - Markdown 上下文
GET  /api/v1/attributes    - 属性数值

# 角色管理
POST /api/v1/character/register - 创建角色
GET  /api/v1/character         - 角色信息
GET  /api/v1/character/experiences - 经历日志
POST /api/v1/character/dream     - 托梦注入
POST /api/v1/character/rebirth   - 转生

# 关系与记忆
GET  /api/v1/relationship/list  - 关系列表
POST /api/v1/relationship       - 更新关系
GET  /api/v1/memory/recent      - 近期记忆
POST /api/v1/memory/search      - 记忆搜索
POST /api/v1/memory             - 存储记忆

# 审查系统
GET  /api/v1/review/pending         - 待审查列表
POST /api/v1/review/{intent_id}      - 提交审查
GET  /api/v1/review/{intent_id}/status - 审查状态

# 验证
POST /api/v1/validate - Intent 验证

# ⚠️ 已禁用
POST /api/v1/intent   - Intent 提交（强制走 WebSocket）
```

### 2.4 Web 面板

```
static/panel/
├── index.html      # 角色创建页
├── character.html   # 角色信息页
├── manage.html     # 管理页（托梦、转生）
├── script.js
└── style.css
```

---

## 三、验证状态

| 验证项 | 状态 | 说明 |
|--------|------|------|
| Build (`cargo build -p cyber-jianghu-agent`) | ✅ 通过 | |
| Tests (261) | ✅ 全部通过 | |
| Clippy on binary | ✅ 0 新问题 | 14 个预先存在于其他文件 |
| 架构复核 | ✅ CO 原则正确实现 | HTTP API 独立组件 |

---

## 四、设计决策

### 4.1 Web 控制语义通过托梦实现

**禁止**: HTTP Intent 提交（破坏 tick 同步）
**允许**: 托梦注入（语义影响 LLM 决策）

```
Web 托梦 → DreamState → consume_dream() → 注入 LLM Context → LLM 自主决策
```

### 4.2 ZeroClaw 参考

显式 Attribution:
- `runtime/claw/history.rs:11-12` — 参照 ZeroClaw 的 History Management 设计
- `runtime/claw/turn_cycle.rs:3` — 参考 ZeroClaw 的 run_tool_call_loop() 设计

### 4.3 Provider 支持

已实现的 Provider 类型 (`ai/llm/direct_client.rs`):
- `OpenClaw` — 读取 `~/.openclaw/openclaw.json` 配置
- `OpenAICompatible` — OpenAI 兼容 API（需手动指定 base_url 和 model）
- `Ollama` — 本地模型 (默认 `http://localhost:11434/v1`)

通过 `OpenAICompatible` 可接入 Claude、GLM 等兼容 OpenAI API 格式的服务。

---

## 五、待办事项

### 5.1 可选优化（非阻塞）

| 任务 | 优先级 | 说明 |
|------|--------|------|
| zeroclaw 架构深入研究 | 低 | 已完成初步探索，发现 zeroclaw 过于通用，不适合直接复用 |
| Agent 间 Channel 通信 | 低 | HTTP API 已实现基本通信 |

### 5.2 已废弃

| 任务 | 状态 | 说明 |
|------|------|------|
| OpenClaw 集成 (Cyber-Jianghu-Openclaw) | 维护中 | 作为独立 npm 包 `@8kugames/cyber-jianghu-openclaw` |
| zeroclaw 自建代理 | 放弃 | Claw 模式已满足需求 |

---

## 六、文件清单

### 核心文件

```
crates/agent/src/
├── bin/cyber-jianghu-agent.rs     # 主入口
│   ├── run_agent()               # Cognitive 模式入口
│   ├── start_http_api_server()   # HTTP API 启动
│   └── run_observer_mode()        # Observer 模式
│
├── runtime/claw/
│   ├── mod.rs                    # 模块定义
│   ├── decision.rs               # ClawDecisionState + claw_decision()
│   ├── context.rs                # ContextBuilder
│   ├── turn_cycle.rs             # TurnCycle
│   └── history.rs                # HistoryManager
│
├── runtime/decision/http/
│   ├── mod.rs                    # HTTP 决策 + API 状态
│   ├── handlers.rs               # HTTP 处理器
│   ├── cognitive_context.rs      # 认知上下文
│   └── review.rs                 # 审查系统
│
└── static/panel/                 # Web 面板
    ├── index.html
    ├── character.html
    └── manage.html
```

### 关键类型

```rust
// DecisionCallback
Arc<dyn Fn(&WorldState) -> Pin<Box<dyn Future<Output = Intent>>>>

// ClawDecisionState
pub struct ClawDecisionState {
    pub llm: Arc<dyn LlmClient>,
    pub context_builder: ContextBuilder,
    pub system_prompt: String,
}

// HttpApiState
pub struct HttpApiState {
    pub current_state: Arc<RwLock<Option<WorldState>>>,
    pub intent_tx: mpsc::Sender<Intent>,
    pub memory_manager: Option<Arc<Mutex<MemoryManager>>>,
    pub review_store: Option<Arc<ReviewStore>>,
    pub dream_store: Option<Arc<RwLock<DreamState>>>,
    // ...
}
```

---

## 七、使用方式

```bash
# Cognitive 模式（默认，ReflectorSoul 已内置启用）
cyber-jianghu-agent run --mode cognitive --character-name 张三

# Cognitive 模式（简写）
cyber-jianghu-agent run

# Claw 模式（ReflectorSoul 同样内置）
cyber-jianghu-agent run --mode claw

# Web 面板
# http://127.0.0.1:{port}/index.html
```

---

## 八、结论

**核心实现已完成**：
1. **双 Soul 架构**：ActorSoul + ReflectorSoul 默认启用，通过 ReviewStore 共享内存通信
2. **Cognitive 模式**：内置 LLM 决策，ReflectorSoul 自动审查
3. **Claw 模式**：外置 LLM（OpenClaw），ReflectorSoul 同样可用
4. **架构简化**：移除了远程 Observer 模式，单一进程双 Soul 设计更清晰
5. **HTTP API**：保留 `/api/v1/review/*` 端点供外部工具查询
