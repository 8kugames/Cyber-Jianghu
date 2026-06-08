# 命令行工具 (CLI)

**级别**: P2 体验增强
**模块**: `crates/agent`

## 1. 第一性原理与设计目标
Agent 需要作为独立的进程在各种环境中运行（开发、测试、生产）。CLI 必须提供标准化的入口，通过简单的参数支持不同的运行模式（Cognitive 与 Claw），并能优雅地处理网络端口分配、配置加载和设备注册。

## 2. 核心机制

### 2.1 运行模式切换 (Runtime Mode)
CLI 是 Agent 架构“双模式统一”的直接体现：
- `run --mode cognitive`：内置 LLM 客户端，自闭环进行思维推演。
- `run --mode claw`：桥接模式，不直接调用 LLM，而是通过 WebSocket 将 Prompt 和意图转发给外部的 OpenClaw 集群（`OpenClawBridge`）。
- **统一抽象**：两者的唯一差异在于 `create_llm_client` 返回的是 `FallbackLlmClient` 还是 `OpenClawBridge`，其余组件（记忆、关系、三魂引擎）完全一致。

### 2.2 自动端口探测与 HTTP API 挂载
- 允许通过 `--port 0` 参数让操作系统自动选择空闲端口（优先尝试 `23340`，如果占用则在 `23340~23999` 范围内随机探测）。
- 这解决了在单机大规模部署成百上千个 Agent 时遇到的端口冲突问题。
- 启动后自动挂载 HTTP API 和前端静态面板。

### 2.3 设备与角色解耦 (Device-Character Separation)
- **设备注册**：首次运行会调用 `/device/register` 自动向 Server 注册设备（分配 `device_id` 与 `auth_token`）并持久化到本地。
- **配置加载与热重载**：提供 `config` 命令更新本地环境配置，并支持运行时通过 API 触发配置热重载。
- 提供 `create-character` 子命令，允许绕过 Web 页面直接在终端创建角色并上报给 Server。

## 3. 架构约束
- **Fail Fast**：CLI 在启动时必须执行严格的配置校验（如 `earth_soul.validate()`），如果关键配置（如设备 YAML 状态与 Server 失去同步）不一致，必须立即退出并要求用户明确处理，禁止在运行时隐式 Panic。
- **无锁阻塞**：CLI 作为 Agent 进程的入口，在初始化期间可安全使用 `tokio::task::block_in_place` 或 `block_on` 处理配置拉取，但进入主循环后必须全异步。

## 4. 代码入口
- CLI 核心定义与启动: `crates/agent/src/bin/cyber-jianghu-agent.rs`
- 运行时模式配置: `crates/agent/src/config.rs`
- OpenClaw 桥接入口: `crates/agent/src/runtime/claw/`