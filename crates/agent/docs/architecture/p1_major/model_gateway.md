# 模型网关与调度

**级别**: P1 重要特性
**模块**: `crates/agent`

## 1. 设计目标
提供统一的 LLM 客户端池，确保在各类不可靠的商业或开源 LLM API 上实现高可用性、容错和成本监控。

## 2. 核心机制
### 2.1 主备切换 (Fallback Client)
- 当主模型（如调用国外的 OpenAI）发生超时、触发并发限流 (Rate Limit) 或 5xx 错误时，网关会自动无缝切换到备用模型（如本地部署的 Ollama 或国内大模型）。
- 切换对上层的认知引擎完全透明，保障游戏的流畅运行。

### 2.2 自动重试机制
- 内置指数退避（Exponential Backoff）重试策略，处理网络瞬断和偶发的幻觉输出。

### 2.3 Token 消耗监控
- 拦截并解析 API 返回的 `usage` 字段。
- 记录和统计每次决策的算力消耗（Prompt Tokens, Completion Tokens），输出到监控日志中，为后续的成本优化提供依据。

## 3. 架构约束
- 网关层必须实现标准的 `LlmClient` Trait。
- 不能无限重试，通常限制在 2-3 次，超出则返回预设的默认“发呆”或“休息”动作。

## 4. 代码入口
- 接口定义: `crates/agent/src/component/llm/client.rs`
- 备用网关实现: `crates/agent/src/component/llm/fallback.rs`
