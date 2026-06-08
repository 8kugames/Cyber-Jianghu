# 模型网关与调度 (Model Gateway)

在多 Agent 模拟环境中，API 限流 (429)、服务商宕机或余额耗尽 (403) 是极其常见的异常。为此，Agent SDK 的底层实现了一个统一的模型网关调度器。

相关代码路径：`crates/agent/src/component/llm/client.rs` 与 `direct_client.rs`。

## 统一抽象 `LlmClient`

系统所有需要用到大模型的地方（CognitiveEngine 人魂决策、ReflectorSoul 天魂审查、动态角色生成等），都统一依赖 `LlmClient` trait。

```rust
#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn complete(&self, prompt: &str) -> Result<String>;
    // ... 结构化与流式方法
}
```

## `FallbackLlmClient` (容灾降级包装器)

我们在配置中可以指定多个模型（包含不同 provider）：
```yaml
llm:
  models:
    - provider: sensenova
      model: SenseChat-5
    - provider: ollama
      model: qwen2.5:14b
```

SDK 启动时会将它们组装为一个 `FallbackLlmClient`。它的调度策略如下：
1. **优先使用当前活跃模型** (Sticky Session)
2. **发生错误时拦截并分类** (`classify_llm_error`):
   - `GiveUp`: 如 Prompt 过长，直接抛出。
   - `Retry`: 网络波动，原地重试。
   - `Fallback`: 当前模型 404 / 403 (额度耗尽)，自动切换到下一个可用模型。
   - `FallbackAndDisable`: 当前模型触发 429 限流或服务宕机，触发**熔断**并切换到下一个。

## `SharedBreaker` 熔断器 (Circuit Breaker)

为了防止多个 Agent 雪崩式地重试已经被限流的服务商，系统实现了一个全局共享的熔断器 `SharedBreaker`。

当某个 `provider/model` 被判定为 `FallbackAndDisable` 时，`SharedBreaker` 会记录其时间戳，在接下来的 **3600 秒 (1 小时)** 内，任何对该模型的调用（无论来自地魂 tool loop 还是主干）都会直接在本地被拦截并返回 "熔断冷却中"，从而平滑地将流量切换到本地兜底模型（如 Ollama），1小时后自动尝试恢复。
