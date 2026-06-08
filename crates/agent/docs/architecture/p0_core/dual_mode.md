# 双栖运行模式 (Dual Mode)

虚境：江湖 Agent SDK 采用了一套极其优雅的**双栖运行架构**，能够在“完全自治”与“外部大脑”之间无缝切换。

相关代码：
- `crates/agent/src/runtime/claw/`
- `crates/agent/src/component/llm/direct_client.rs` (内置)
- `crates/agent/src/runtime/claw/bridge.rs` (外置)

## 核心设计理念：完全同构

无论使用哪种模式，Agent 的核心认知管线（三层记忆、人设演化、三魂循环、意图验证、即时事件处理）**完全一致**。唯一的区别在于底层的 `LlmClient` 接口由谁来实现。

| 模块 | Cognitive 模式 (内置) | Claw 模式 (外置) |
|---|---|---|
| **ActorSoul (认知引擎)** | `CognitiveEngine` | `CognitiveEngine` |
| **EarthSoul (地魂工具)** | `run_tool_loop` 共享执行 | `run_tool_loop` 共享执行 |
| **ReflectorSoul (天魂审查)**| 本地 RuleEngine + LLM Validator | 本地 RuleEngine + LLM Validator |
| **Memory System (三层记忆)**| 本地 `MemoryManager` 维护 | 本地 `MemoryManager` 维护 |
| **LLM 客户端** | `FallbackLlmClient` (直连 OpenAI 兼容 API) | `OpenClawBridge` (通过 WebSocket 转发给 OpenClaw) |

## 模式详解

### 1. Cognitive 模式 (默认)

**适用场景**：Agent 作为独立的进程运行，自主进行思考和决策。
**工作流**：
1. Agent 接收到 `WorldState`。
2. 组装上下文，调用内部的 `FallbackLlmClient`。
3. `FallbackLlmClient` 直接发起 HTTP 请求到配置的 LLM 服务商（如 Ollama, OpenAI）。
4. 获得返回结果后解析为 `Intent`，经过天魂审查后提交给服务端。

**高可用性**：内置了多级 Fallback 机制（403/429/Timeout 自动降级备用模型）。

### 2. Claw 模式

**适用场景**：将 Agent 作为“肉体”（执行器和上下文收集器），而将“灵魂”（决策权）托管给外部复杂的 Agent 调度系统（如 OpenClaw）。
**工作流**：
1. Agent 启动时指定 `--mode claw`。
2. Agent 实例化 `OpenClawBridge` 作为其 `LlmClient` 实现。
3. OpenClaw 通过 WebSocket 连接到 Agent 的 `23340` 端口。
4. 当 `CognitiveEngine` 组装好 Prompt 并发起 LLM 调用时，`OpenClawBridge` 将请求打包为 `LlmRequest` 消息，通过 WebSocket 发送给 OpenClaw。
5. OpenClaw 在外部完成复杂的推理（可能经过多个 Agent 的讨论、规划），然后将最终结果返回给 Agent。
6. Agent 解析结果，经过天魂审查后，提交给服务端。

## 热切换与 LlmClientContainer

SDK 支持在运行时动态切换 LLM 配置，这是通过 `LlmClientContainer` 实现的：

```rust
pub type LlmClientContainer = Arc<RwLock<Arc<dyn LlmClient>>>;
```

无论是 ActorSoul 还是 ReflectorSoul，都持有这个容器的引用。当用户在 Web Panel 中修改了 LLM 配置时，系统只需将容器内的指针替换为新的 `LlmClient` 实例，下一次 Tick 就会自动使用新的模型，无需重启 Agent。
