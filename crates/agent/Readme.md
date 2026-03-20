# Cyber-Jianghu Agent SDK

Agent SDK 是连接赛博江湖服务端的桥梁。它为开发者提供了与游戏世界交互的基础设施，并且内置了记忆、认知、对话等高级 AI 模块，方便快速构建拥有独立思考能力的“赛博侠客”。

## 架构概览

```
crates/agent/src/
├── core/           # 核心控制与生命周期
│   ├── agent.rs    # Agent 主体结构
│   ├── builder.rs  # Agent 构建器
│   ├── lifecycle.rs # 核心运行循环
│   └── cognitive/  # 认知流管道编排
├── transport/      # 网络通信层
│   └── websocket.rs # WebSocket 长连接客户端实现
├── runtime/        # 运行与决策模式
│   ├── decision/   # 决策接口定义
│   │   ├── http/   # HTTP 模式 API (供 OpenClaw 或外部 LLM 调用)
│   │   └── cognitive.rs # 内置 Cognitive 决策模式
│   └── notify/     # 异步通知系统 (如 OpenClaw 回调)
├── ai/             # 智能增强模块子系统
│   ├── llm/        # 统一的大模型客户端抽象 (直连/OpenClaw)
│   ├── cognitive/  # 认知转换与叙事化生成
│   ├── memory/     # 记忆系统实现
│   │   ├── backends/ # 记忆存储后端 (工作、情景、语义、归档)
│   │   ├── embedder.rs # 向量化支持
│   │   └── registry.rs # 记忆检索器
│   ├── persona/    # 动态人设引擎 (性格演变)
│   ├── validator/  # LLM 意图拦截与验证
│   ├── dialogue/   # 智能对话处理客户端
│   ├── relationship/ # 社交与人际关系管理
│   └── lifespan/   # 寿命推演计算
├── config.rs       # Agent 侧配置
├── models.rs       # 内部数据模型
└── bin/            # CLI 启动入口点
```

## 运行模式与接口说明 (For Developers)

Agent 支持多种运行模式，以适应不同的集成场景。

### 1. Claw 模式 (推荐 OpenClaw / 外部应用集成)
通过启动一个混合服务（WebSocket + HTTP API），将底层的 WebSocket 协议转换为易于调用的 REST API。
- **启动**: `cyber-jianghu-agent run --mode claw --port 23340`
- **主要接口**:
  - WebSocket `/ws`: 实时决策推送
  - `GET /api/v1/state`: 获取最新收到的 `WorldState` 快照。
  - `GET /api/v1/context`: 提取并生成适合放入 LLM Prompt 的 Markdown 叙事上下文。
  - `POST /api/v1/intent`: 接收外部 LLM 生成的动作，并转发至服务端。
  - `POST /api/v1/validate`: 提前验证将要执行的动作是否合法（体力、位置校验）。
  - `POST /api/v1/memory`: 写入记忆。
  - `POST /api/v1/memory/search`: 基于向量或 FTS 搜索语义记忆。

### 2. Cognitive 模式 (内置完整智能流)
Agent 将接管全部决策，内置完整的认知流水线。
- **核心流程 (`core/cognitive/pipeline.rs`)**:
  1. **Perception (感知)**: 解析 `WorldState` 和个人状态，翻译为自然语言（如“你正处于客栈，体力充沛”）。
  2. **Memory Retrieval (记忆检索)**: 检索相关的情景与语义记忆。
  3. **Motivation (动机)**: 结合 `Persona` 与当前状态，推断短期和长期动机。
  4. **Decision (决策)**: 组装 Prompt，调用内置 LLM 客户端生成 `Intent`。
  5. **Validation (验证)**: 检查 LLM 幻觉或非法动作，必要时重试。
  6. **Execution (执行)**: 将合法的意图投递给 `transport` 层。

### 3. 其他辅助模式
- **Simple**: 使用硬编码规则（如“饿了就吃，困了就睡”）快速测试物理系统。
- **Idle**: 保持连接并响应心跳，但不做任何动作，常用于占位。

## 核心智能模块

- **Memory System**:
  - 分层设计：`Working` (短期缓存), `Episodic` (时间序列事件), `Semantic` (向量化知识), `Archive` (长期归档)。
  - 本地化：默认采用 SQLite + 本地 Embedder，保护隐私与控制成本。
- **Persona System**:
  - 根据世界中发生的事件动态演进性格特征（如多次被攻击后变得“多疑”）。
- **Relationship System**:
  - 维护对其他 Agent 的好感度、信任度与交往记录，驱动对话与交互态度。

## 扩展与二次开发
1. **自定义记忆后端**: 可以在 `ai/memory/backends/` 下实现新的 Trait 来对接向量数据库（如 Milvus / Qdrant）。
2. **新增决策模式**: 在 `runtime/decision/` 下实现新的决策逻辑并注册到 `builder.rs`。
3. **调整认知 Prompt**: 在 `ai/prompts.rs` 中修改内置的 Prompt 模板结构，优化大模型的表现。
