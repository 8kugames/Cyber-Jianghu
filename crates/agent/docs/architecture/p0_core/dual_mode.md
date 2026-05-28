# 双栖运行模式

**级别**: P0 核心基石
**模块**: `crates/agent`

## 1. 设计目标
支持不同的部署拓扑，兼顾“独立智能体”（每个 Agent 自带大脑进程）与“中心化集群大模型管控”（所有 Agent 桥接到统一的调度中心）的需求。

## 2. 核心机制
### 2.1 Cognitive 模式 (独立模式)
- 调用内置的 LLM 客户端（如直连 Ollama、OpenAI API 或 VLLM）。
- Agent 作为一个独立的进程/线程运行，包含了从网络通信、上下文组装到大模型请求的所有全流程。
- 适合本地开发、单机小规模测试或分布式独立部署。

### 2.2 Claw 模式 (附庸模式)
- 通过 OpenClaw 协议，将 Agent 桥接到外部第三方大模型调度平台（中心大脑）。
- Agent 不再自己发起 LLM HTTP 请求，而是将组装好的 `DecisionContextSnapshot` 和 Prompt 发送给远端中心，等待远端返回 Intent。
- 适合大规模集群部署，统一由专业的推理加速集群（如 vLLM + K8s）集中处理所有智能体的思考，提高 GPU 批处理利用率。

## 3. 架构约束
- 两者必须共享完全一致的构建管线（`AgentBuilder`）、三魂架构和记忆系统。
- 除网络请求层的实现类（`DirectLlmClient` vs `OpenClawBridge`）不同外，绝不允许存在任何 Agent 本身游戏机制或认知能力上的差分。
- LLM 交互通过 `LlmClient` trait 的 `send_chat_exchange` 方法抽象，DirectLlmClient 用 HTTP，OpenClawBridge 用 WebSocket。地魂 `tool_loop.rs` 的共享循环逻辑通过此抽象实现模式无关。

## 4. 代码入口
- 模式切换入口: `crates/agent/src/bin/cyber-jianghu-agent.rs`
- Claw 桥接实现: `crates/agent/src/component/llm/openclaw_bridge.rs`
