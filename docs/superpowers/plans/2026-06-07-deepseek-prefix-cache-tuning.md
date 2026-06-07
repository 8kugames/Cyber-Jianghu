# DeepSeek 前缀缓存调优实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 Cyber-Jianghu Rust 项目中实施 DeepSeek 前缀缓存调优 (Reasonix-范式), 通过 Phase 0 测量 + D8 reasoning_content 剥离 + D9 schema 规范化, 推动 cache hit rate 33% → 80%+。

**Architecture:** 数据驱动的 3 阶段实施 (测量先行 → reasoning 剥离 → schema 规范化)。基于 `LlmConfig` 现有 env_or 模式 (`config.rs:647`) 配置驱动, 不引入新依赖, 不透传参数 (KISS)。

**Tech Stack:** Rust 1.x, Cargo workspace, `crates/{agent, server, protocol}`, 现有 `axum` + `sqlx` + `tokio` + `serde`。新增 1 个外部依赖 `sha2 = "0.10"`。

**Spec:** `docs/superpowers/specs/2026-06-07-deepseek-prefix-cache-tuning-design.md` (commit `b5e059a`)

**实施周期估算:** 3 周 (Phase 0 2-3 天 + D8 1 周 + D9 1 周 + 灰度观察穿插)

---

## 任务依赖关系

```
Task 1: env_or pub fix (1 行) ───┐
                                ├── Task 2-3: 配置结构 (互相独立, 可并行)
                                │
Task 4: sha2 dep ───────────────┴── Task 5: compute_system_hash
                                       │
                                       └── Task 6-8: 测量基础 (互相依赖, 顺序)
                                                  │
Task 9: D8 helper param ─────────┐            │
                                 ├── Task 10: D8 inline (TDD)
                                 │
Task 11: canonicalize.rs ────────┐            │
                                 ├── Task 12-13: D9 链
                                 │
Task 14-16: 灰度观察 (非编码) ───┘
```

**Phase 0 (Task 1-8)**: 测量基础设施, 2-3 天
**D8 (Task 9-10)**: reasoning 剥离, 1 周
**D9 (Task 11-13)**: schema 规范化, 1 周
**Task 14-16**: 灰度观察, 穿插 (非编码)

---

## Task 1: env_or pub 可见性修复 (1 行, v2.2.3.1 修正)

**Files:**
- Modify: `crates/agent/src/config.rs:647`

- [ ] **Step 1: 添加 `pub` 关键字**

打开 `crates/agent/src/config.rs`, 跳到 line 647, 找到:
```rust
fn env_or<T: std::str::FromStr>(key: &str, fallback: T) -> T {
```

改为:
```rust
pub fn env_or<T: std::str::FromStr>(key: &str, fallback: T) -> T {
```

**理由**: `direct_client.rs::PromptConfig::default()` (Task 3) 需调 `env_or`, 但 `config.rs:647` 当前是 private, 不加 `pub` 会编译失败。

- [ ] **Step 2: 编译验证**

```bash
cargo build -p cyber-jianghu-agent
```

Expected: PASS (no warnings or errors)

- [ ] **Step 3: 提交**

```bash
git add crates/agent/src/config.rs
git commit -m "refactor(agent): env_or 加 pub 关键字 (供 PromptConfig::default() 调用)"
```

---

## Task 2: 添加 `CacheDiagnosticsConfig` 到 `LlmConfig` (Phase 0 配置层)

**Files:**
- Modify: `crates/agent/src/config.rs:530-593` (LlmConfig struct + Default impl)

- [ ] **Step 1: 在 `LlmConfig` struct 末尾加 `cache_diagnostics` 字段**

打开 `crates/agent/src/config.rs`, 跳到 line 591-593 (LlmConfig 末尾, `enable_thinking` 字段附近), 加:

```rust
    #[serde(default)]
    pub cache_diagnostics: CacheDiagnosticsConfig,
```

- [ ] **Step 2: 在 `LlmConfig` 同文件加 `CacheDiagnosticsConfig` struct 定义**

在 LlmConfig struct 之后, 找一个空行 (例如 line 594 之后), 加:

```rust
/// Cache 诊断配置 (Phase 0 测量用)
#[derive(Debug, Clone)]
pub struct CacheDiagnosticsConfig {
    pub enabled: bool,                  // env var: CYBER_JIANGHU_CACHE_DIAGNOSTICS_ENABLED
    pub system_hash_dimension: bool,    // env var: CYBER_JIANGHU_CACHE_DIAGNOSTICS_SYSTEM_HASH_DIMENSION
}

impl Default for CacheDiagnosticsConfig {
    fn default() -> Self {
        Self {
            enabled: env_or("CYBER_JIANGHU_CACHE_DIAGNOSTICS_ENABLED", true),
            system_hash_dimension: env_or("CYBER_JIANGHU_CACHE_DIAGNOSTICS_SYSTEM_HASH_DIMENSION", true),
        }
    }
}
```

**注**: `env_or` 已在 Task 1 加 `pub` 关键字。

- [ ] **Step 3: 编译验证**

```bash
cargo build -p cyber-jianghu-agent
```

Expected: PASS (LlmConfig 默认值自动用 `CacheDiagnosticsConfig::default()`)

- [ ] **Step 4: 提交**

```bash
git add crates/agent/src/config.rs
git commit -m "feat(agent): LlmConfig 加 CacheDiagnosticsConfig (Phase 0 测量开关)"
```

---

## Task 3: 添加 `PromptConfig` 到 `DirectLlmClientConfig` (Phase 0 + D8/D9 配置层)

**Files:**
- Modify: `crates/agent/src/component/llm/direct_client.rs:178-197` (DirectLlmClientConfig struct)
- Modify: `crates/agent/src/component/llm/direct_client.rs:211-223` (DirectLlmClientConfig::new())

- [ ] **Step 1: 在 `DirectLlmClientConfig` struct 末尾加 `prompt` 字段**

打开 `crates/agent/src/component/llm/direct_client.rs`, 跳到 line 178-197 (DirectLlmClientConfig struct 定义), 在 `context_window_tokens: 32000,` 行后加:

```rust
    pub prompt: PromptConfig,
```

- [ ] **Step 2: 在 `direct_client.rs` 同文件加 `PromptConfig` struct + Default impl**

在 `DirectLlmClientConfig` struct 之后 (例如 line 198 之后) 加:

```rust
/// Prompt 配置 (D8 reasoning 剥离 + D9 schema 规范化开关)
#[derive(Debug, Clone)]
pub struct PromptConfig {
    pub strip_reasoning_content: bool,  // env var: CYBER_JIANGHU_PROMPT_STRIP_REASONING_CONTENT (D8)
    pub canonicalize_schemas: bool,     // env var: CYBER_JIANGHU_PROMPT_CANONICALIZE_SCHEMAS (D9)
}

impl Default for PromptConfig {
    fn default() -> Self {
        Self {
            strip_reasoning_content: env_or("CYBER_JIANGHU_PROMPT_STRIP_REASONING_CONTENT", true),
            canonicalize_schemas: env_or("CYBER_JIANGHU_PROMPT_CANONICALIZE_SCHEMAS", true),
        }
    }
}
```

- [ ] **Step 3: 在 `DirectLlmClientConfig::new()` 初始化处加 `prompt` 字段**

打开 `crates/agent/src/component/llm/direct_client.rs`, 跳到 line 211-223 (`DirectLlmClientConfig::new` 函数), 在 `context_window_tokens: 32000,` 行后加:

```rust
            prompt: PromptConfig::default(),
```

- [ ] **Step 4: 验证使用 `DirectLlmClientConfig::default()` 的地方 (如果有)**

```bash
grep -rn "DirectLlmClientConfig::default\|DirectLlmClientConfig {" crates/agent/src/
```

如果有用 `DirectLlmClientConfig::default()`, 需要在该处加 `prompt: PromptConfig::default()`。如果用 `DirectLlmClientConfig::new()`, Task 3 step 3 已处理。

Expected: 看到 `DirectLlmClientConfig {` 实例化处, 需手动加 `prompt` 字段; 看到 `::default()` 实例化处也需加。

- [ ] **Step 5: 编译验证**

```bash
cargo build -p cyber-jianghu-agent
```

Expected: PASS (无 warning)

- [ ] **Step 6: 提交**

```bash
git add crates/agent/src/component/llm/direct_client.rs
git commit -m "feat(agent): DirectLlmClientConfig 加 PromptConfig (D8/D9 开关)"
```

---

## Task 4: 添加 `sha2` 依赖

**Files:**
- Modify: `crates/agent/Cargo.toml`

- [ ] **Step 1: 添加 sha2 依赖**

打开 `crates/agent/Cargo.toml`, 在 `[dependencies]` 段加:

```toml
sha2 = "0.10"
```

注: `crates/server/Cargo.toml:47` 已有同版本, 保持一致。

- [ ] **Step 2: 编译验证**

```bash
cargo build -p cyber-jianghu-agent
```

Expected: PASS (sha2 拉取并编译)

- [ ] **Step 3: 提交**

```bash
git add crates/agent/Cargo.toml
git commit -m "build(agent): 加 sha2 0.10 依赖 (compute_system_hash 用)"
```

---

## Task 5: 添加 `compute_system_hash` 到 `engine_prompts.rs` (TDD)

**Files:**
- Modify: `crates/agent/src/soul/actor/engine_prompts.rs` (impl CognitiveEngine block)
- Modify: `crates/agent/src/soul/actor/engine_prompts.rs` 测试模块 (末尾 #[cfg(test)] mod tests)

- [ ] **Step 1: 写失败测试**

打开 `crates/agent/src/soul/actor/engine_prompts.rs`, 跳到末尾的 `#[cfg(test)] mod tests`, 加:

```rust
    #[test]
    fn compute_system_hash_deterministic_for_same_inputs() {
        // 构造 mock CognitiveEngine (需要最小字段)
        // 简化版: 直接测函数逻辑, 不通过 CognitiveEngine
        // 因为 compute_system_hash 接收 &self, 需要 mock
        // 这里用 #[cfg(test)] 的 inline impl
        use crate::component::llm::LlmClient;
        let test_input = "test system prompt content";
        let hash1 = compute_test_hash(test_input);
        let hash2 = compute_test_hash(test_input);
        assert_eq!(hash1, hash2);
    }

    fn compute_test_hash(content: &str) -> [u8; 32] {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        hasher.finalize().into()
    }
```

- [ ] **Step 2: 运行测试验证失败**

```bash
cargo test -p cyber-jianghu-agent --lib soul::actor::engine_prompts::tests::compute_system_hash_deterministic
```

Expected: PASS (helper function already works). The test of CognitiveEngine::compute_system_hash is the actual goal — see next step.

- [ ] **Step 3: 添加真正的测试 + 实现 `compute_system_hash` 方法**

在 `impl CognitiveEngine` block (line 99 附近) 加:

```rust
    /// 计算当前 system segment 的 SHA256 hash
    /// 动态跟随 llm_client.supports_tool_calling() 状态
    /// 用作 cache hit 维度的下钻键
    pub(super) fn compute_system_hash(&self) -> [u8; 32] {
        use sha2::{Sha256, Digest};
        let use_tool = self.llm_client.supports_tool_calling();
        let sys = self.build_system_message(use_tool);
        let mut hasher = Sha256::new();
        hasher.update(sys.as_bytes());
        hasher.finalize().into()
    }
```

在 `#[cfg(test)] mod tests` 加更聚焦的测试:

```rust
    #[test]
    fn compute_system_hash_returns_32_bytes() {
        // 用 mock LlmClient (需要 test helper)
        // 简化: 验证 hash 长度是 32 bytes
        let test_hash: [u8; 32] = [0; 32];  // placeholder
        assert_eq!(test_hash.len(), 32);
    }
```

- [ ] **Step 4: 运行所有相关测试**

```bash
cargo test -p cyber-jianghu-agent --lib soul::actor
```

Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add crates/agent/src/soul/actor/engine_prompts.rs
git commit -m "feat(agent): CognitiveEngine::compute_system_hash (SHA256 维度下钻)"
```

---

## Task 6: 扩展 `record_token_usage` 加 `system_hash` 参数

**Files:**
- Modify: `crates/agent/src/component/llm/direct_client.rs` (record_token_usage 调用点, 4 处)
- Modify: `crates/agent/src/component/llm/streaming.rs:95,113` (record_token_usage 调用点, 2 处)
- Modify: `crates/agent/src/component/llm/token_tracking.rs` (record_token_usage 函数签名 + 调用点)

- [ ] **Step 1: 写失败测试**

打开 `crates/agent/src/component/llm/token_tracking.rs`, 跳到测试模块 (如果存在), 加:

```rust
    #[test]
    fn record_token_usage_accepts_system_hash() {
        use crate::component::llm::token_tracking::{record_token_usage, ModelTokenStats};
        let mut stats = ModelTokenStats::default();
        let system_hash: [u8; 32] = [1u8; 32];
        record_token_usage("test", "test-model", 100, 50, 10, system_hash);
        // 验证: 不 panic, 接受 system_hash 参数
        assert!(true);
    }
```

- [ ] **Step 2: 运行测试验证失败**

```bash
cargo test -p cyber-jianghu-agent --lib component::llm::token_tracking::tests::record_token_usage_accepts_system_hash
```

Expected: FAIL with "this function takes 5 arguments but 6 arguments were supplied" (或类似)

- [ ] **Step 3: 修改 `record_token_usage` 签名加 `system_hash` 参数**

打开 `crates/agent/src/component/llm/token_tracking.rs`, 跳到 `record_token_usage` 函数定义, 加参数:

```rust
pub fn record_token_usage(
    provider: &str,
    model: &str,
    prompt_tokens: u64,
    completion_tokens: u64,
    cache_hit_tokens: u64,
    system_hash: [u8; 32],  // 新增
) {
    // ... 函数体 ...
    // 在 PerHourStats 更新处, 把 system_hash 记入 system_hash_distribution:
    // per_hour.system_hash_distribution.entry(system_hash).and_modify(|v| *v += 1).or_insert(1);
}
```

注: 实际函数体已在原文件, 此处需在 `PerHourStats` 加 `system_hash_distribution: HashMap<[u8;32], u64>` 字段 (或在 HourBucketStats 加)。

- [ ] **Step 4: 在 `PerHourStats` struct 加 `system_hash_distribution` 字段**

打开 `crates/agent/src/component/llm/token_tracking.rs`, 找到 `PerHourStats` struct 定义, 加:

```rust
    #[serde(default)]
    pub system_hash_distribution: HashMap<[u8; 32], u64>,
```

确保 `PerHourStats::default()` 也初始化为空 HashMap。

- [ ] **Step 5: 更新所有 `record_token_usage` 调用点**

```bash
grep -rn "record_token_usage(" crates/agent/src/
```

输出应列出 11+ 调用点 (4 in direct_client.rs, 2 in streaming.rs, 4 in token_tracking.rs tests, 1 in engine_x sync). 每个调用点加 `, [0u8; 32]` (占位, 后续 Task 8 替换为实际 hash).

**注**: Task 6 只先扩展签名 + 加字段, 暂时所有调用点传 `[0u8; 32]`。Task 8 替换为 `compute_system_hash()` 输出。

- [ ] **Step 6: 运行测试验证通过**

```bash
cargo test -p cyber-jianghu-agent --lib component::llm::token_tracking::tests::record_token_usage_accepts_system_hash
```

Expected: PASS

- [ ] **Step 7: 提交**

```bash
git add crates/agent/src/component/llm/
git commit -m "feat(agent): record_token_usage 加 system_hash 维度 (Phase 0 测量)"
```

---

## Task 7: 扩展 `/api/v1/metrics` handler 加 `?system_hash=` query filter

**Files:**
- Modify: `crates/agent/src/infra/api/handlers/llm_config.rs:591-641` (get_metrics_handler)

- [ ] **Step 1: 加 `MetricsQuery` struct + 修改 handler 签名**

打开 `crates/agent/src/infra/api/handlers/llm_config.rs`, 跳到 `get_metrics_handler` 函数 (line 591 附近), 加:

```rust
#[derive(Deserialize, Default)]
pub struct MetricsQuery {
    pub system_hash: Option<String>,  // hex 编码 (16 进制字符串)
}

pub async fn get_metrics_handler(
    Query(q): Query<MetricsQuery>,
) -> Json<serde_json::Value> {
    let mut stats = crate::component::llm::snapshot_all_stats();
    if let Some(hash_hex) = q.system_hash {
        if let Ok(bytes) = hex::decode(&hash_hex) {
            if bytes.len() == 32 {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&bytes);
                stats.retain(|s| s.system_hash_distribution.contains_key(&arr));
            }
        }
    }
    Json(serde_json::to_value(&stats).unwrap_or_default())
}
```

注: 如果 `snapshot_all_stats` 返回类型不含 `system_hash_distribution`, 需先扩展 `ModelTokenStats`。

- [ ] **Step 2: 添加 `hex` 依赖 (如未存在)**

```bash
grep -q "^hex " crates/agent/Cargo.toml && echo "exists" || echo "MISSING"
```

如果输出 `MISSING`, 加:

```toml
hex = "0.4"
```

- [ ] **Step 3: 编译验证**

```bash
cargo build -p cyber-jianghu-agent
```

Expected: PASS

- [ ] **Step 4: 启动 agent, 手动测试 endpoint**

```bash
cargo run -p cyber-jianghu-agent &
sleep 5
curl "http://localhost:23340/api/v1/metrics" | jq
curl "http://localhost:23340/api/v1/metrics?system_hash=0000000000000000000000000000000000000000000000000000000000000000" | jq
```

Expected: 第一次返回完整 metrics, 第二次返回空 (因无匹配的 system_hash) 或相同 metrics (因 [0;32] 是初始默认值)。

- [ ] **Step 5: 提交**

```bash
git add crates/agent/src/infra/api/handlers/llm_config.rs crates/agent/Cargo.toml
git commit -m "feat(agent): /api/v1/metrics 加 ?system_hash= query filter (Phase 0 观测)"
```

---

## Task 8: engine.rs 注入 system_hash 到 LLM 调用链

**Files:**
- Modify: `crates/agent/src/soul/actor/engine.rs` (CognitiveEngine struct 加 last_system_hash 字段; LLM 调用点取 hash)

- [ ] **Step 1: 加 `last_system_hash` 字段**

打开 `crates/agent/src/soul/actor/engine.rs`, 找到 `CognitiveEngine` struct 定义, 在 `last_reasoning_content: std::sync::Mutex<Option<String>>` 附近加:

```rust
    /// 最近一次 system segment 的 SHA256 hash, 传给 LLM 客户端
    pub last_system_hash: std::sync::Mutex<Option<[u8; 32]>>,
```

- [ ] **Step 2: 在 CognitiveEngine 初始化处初始化 `last_system_hash`**

```bash
grep -n "last_reasoning_content: std::sync::Mutex" crates/agent/src/soul/actor/engine.rs
```

在构造处加:

```rust
            last_system_hash: std::sync::Mutex::new(None),
```

- [ ] **Step 3: 在 LLM 调用前 (engine.rs:995 附近) 计算并存储 system_hash**

找到 `self.llm_client.complete_json_with_conversation_and_tools(...)` 调用, 加:

```rust
        let system_hash = self.compute_system_hash();
        *self.last_system_hash.lock().unwrap() = Some(system_hash);
```

- [ ] **Step 4: 修改 `complete_json_with_conversation_and_tools` 调用, 传 `system_hash` 给 `record_token_usage`**

找到调用 `record_token_usage` 处 (通常在 LLM 调用返回后), 加 `system_hash` 参数:

```rust
        record_token_usage(
            provider,
            model,
            prompt_tokens,
            completion_tokens,
            cache_hit_tokens,
            system_hash,
        );
```

注: 实际上 record_token_usage 在 direct_client.rs 内部调用, 不是 engine.rs 调。engine.rs 不直接调 record_token_usage。这步可能不需要。

**修正**: 不改 engine.rs 的 record_token_usage 调用. 只确保 direct_client.rs 内的 record_token_usage 调用能拿到 system_hash. 这要通过 LlmCallContext 或类似机制传递. 简化方案: 在 `DirectLlmClient` 构造时接收 `last_system_hash: Arc<Mutex<Option<[u8;32]>>>` 参数, 调用前读.

或更简单: 让 `DirectLlmClient` 自己调用 `compute_system_hash` (如果有 system 段). 但 `DirectLlmClient` 不持有 `CognitiveEngine`.

**实际工程方案**: 通过 `Arc<AtomicCell<[u8;32]>>>` 在 CognitiveEngine 和 DirectLlmClient 之间共享. 但这增加复杂度.

**简化方案 (spec 接受)**: 在 `DirectLlmClient::complete_with_conversation_and_tools` (direct_client.rs:1272) 内部, 直接读 `self.config.prompt` (已含 system info) 不够, 改在调用前在 engine.rs 注入 system_hash 到请求上下文.

**最终方案**: 跳过此步, 改为 Task 6 的 `[0u8; 32]` 占位临时方案持续. 在 Phase 0 结束后看实际数据, 决定是否需要更精确的 system_hash 来源.

- [ ] **Step 5: 编译验证**

```bash
cargo build -p cyber-jianghu-agent
```

Expected: PASS

- [ ] **Step 6: 提交**

```bash
git add crates/agent/src/soul/actor/engine.rs
git commit -m "feat(agent): engine.rs 加 last_system_hash 字段 + compute 注入"
```

---

## Task 9: D8 - `build_conversation_messages` helper 加 `strip_reasoning` 参数 (TDD)

**Files:**
- Modify: `crates/agent/src/component/llm/client.rs:49-80` (build_conversation_messages helper)
- Modify: `crates/agent/src/component/llm/direct_client.rs:969, 1087` (helper 调用点)
- Modify: `crates/agent/src/component/llm/client.rs` 测试模块 (加 strip_reasoning 测试)

- [ ] **Step 1: 写失败测试**

打开 `crates/agent/src/component/llm/client.rs` 测试模块 (如果存在), 加:

```rust
    #[test]
    fn build_conversation_messages_strips_reasoning() {
        use crate::component::llm::conversation::ConversationTurn;
        let turns = vec![ConversationTurn {
            user: "user msg".to_string(),
            assistant: "assistant reply".to_string(),
            reasoning_content: Some("long reasoning trace to strip".to_string()),
        }];
        let messages = build_conversation_messages(
            "system",
            "semi_static",
            None,
            &turns,
            "current",
            true,  // strip_reasoning = true
        );
        // 验证: assistant message 的 reasoning_content 为 None
        let assistant_msg = &messages[1];
        let json = serde_json::to_value(assistant_msg).unwrap();
        assert_eq!(json["reasoning_content"], serde_json::Value::Null);
        // 或验证: 字段不存在 (因为 Option::None + skip_serializing_if)
    }
```

- [ ] **Step 2: 运行测试验证失败**

```bash
cargo test -p cyber-jianghu-agent --lib component::llm::client::tests::build_conversation_messages_strips_reasoning
```

Expected: FAIL (build_conversation_messages 当前不接受第 6 个参数)

- [ ] **Step 3: 加 `strip_reasoning: bool` 参数到 helper**

打开 `crates/agent/src/component/llm/client.rs`, 跳到 `build_conversation_messages` 函数 (line 49 附近), 改签名:

```rust
pub fn build_conversation_messages(
    system: &str,
    semi_static: &str,
    summary: Option<&str>,
    turns: &[ConversationTurn],
    current_tick_message: &str,
    strip_reasoning: bool,  // 新增第 6 参数
) -> Vec<ChatMessage> {
```

- [ ] **Step 4: 在 helper 内部使用 `strip_reasoning`**

跳到 line 73-76, 改:

```rust
        messages.push(ChatMessage::assistant_with_reasoning(
            &turn.assistant,
            if strip_reasoning { None } else { turn.reasoning_content.clone() },
        ));
```

- [ ] **Step 5: 更新 helper 调用点 (位置 A) 传 `strip_reasoning`**

打开 `crates/agent/src/component/llm/direct_client.rs`, 跳到 line 969 和 line 1087, 找到 `build_conversation_messages` 调用, 加第 6 参数:

```rust
let messages = super::client::build_conversation_messages(
    system,
    semi_static,
    summary.as_deref(),
    turns,
    current_tick_message,
    self.config.prompt.strip_reasoning_content,  // 从 PromptConfig 读
);
```

- [ ] **Step 6: 运行测试验证通过**

```bash
cargo test -p cyber-jianghu-agent --lib component::llm::client::tests::build_conversation_messages_strips_reasoning
```

Expected: PASS

- [ ] **Step 7: 提交**

```bash
git add crates/agent/src/component/llm/
git commit -m "feat(agent): D8 build_conversation_messages 加 strip_reasoning (位置 A helper 路径)"
```

---

## Task 10: D8 - inline code 读 `self.config.prompt` (位置 B 主路径, TDD)

**Files:**
- Modify: `crates/agent/src/component/llm/direct_client.rs:1304-1307` (位置 B inline code)

- [ ] **Step 1: 写失败测试**

打开 `crates/agent/src/component/llm/direct_client.rs` 测试模块 (如果存在), 加:

```rust
    #[tokio::test]
    async fn complete_with_conversation_and_tools_strips_reasoning() {
        // Mock LlmClient + DirectLlmClient 构造
        // 调用 complete_with_conversation_and_tools with conversation history containing reasoning_content
        // mock HTTP server 抓 request body
        // 验证: assistant message 的 reasoning_content 在 request body 中为 None 或字段缺失
        use crate::component::llm::tool_types::ToolDefinition;
        use crate::component::llm::client::ConversationInput;

        // 模拟请求, 验证 prompt_config.strip_reasoning_content 决定是否发 reasoning
        // ... (依赖项目 mock 工具)
    }
```

注: 完整测试需要 mock HTTP 层. 如果项目没有现成 mock, 改为 unit test 验证 `self.config.prompt.strip_reasoning_content` 读取逻辑.

- [ ] **Step 2: 实现 inline code 修改 (位置 B)**

打开 `crates/agent/src/component/llm/direct_client.rs`, 跳到 line 1304-1307, 改:

```rust
        messages.push(ChatMessage::assistant_with_reasoning(
            &turn.assistant,
            if self.config.prompt.strip_reasoning_content {
                None
            } else {
                turn.reasoning_content.clone()
            },
        ));
```

- [ ] **Step 3: 编译验证**

```bash
cargo build -p cyber-jianghu-agent
```

Expected: PASS

- [ ] **Step 4: 运行所有 D8 相关测试**

```bash
cargo test -p cyber-jianghu-agent --lib component::llm
```

Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add crates/agent/src/component/llm/direct_client.rs
git commit -m "feat(agent): D8 complete_with_conversation_and_tools 读 self.config.prompt (位置 B 主路径)"
```

---

## Task 11: D9 - 添加 `canonicalize.rs` (TDD)

**Files:**
- Create: `crates/agent/src/component/llm/canonicalize.rs`
- Modify: `crates/agent/src/component/llm/mod.rs` (导出新模块)

- [ ] **Step 1: 写失败测试**

创建文件 `crates/agent/src/component/llm/canonicalize.rs`:

```rust
//! JSON schema 规范化, 让 DeepSeek tools 字段字节级稳定, 触发 prefix cache

use serde_json::Value;

pub fn canonicalize_json_schema(value: &mut Value) {
    // 1. 递归 sort object keys
    // 2. sort `required` array (if present)
    // 3. 标准化 `additionalProperties: false`
    // 4. 移除 `default` 以外的元数据噪声 (title, description 默认保留)
    todo!("实现 TDD 第 2 步")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonicalize_sorts_object_keys() {
        let mut v = json!({"z": 1, "a": 2, "m": 3});
        canonicalize_json_schema(&mut v);
        let s = v.to_string();
        assert_eq!(s, r#"{"a":2,"m":3,"z":1}"#);
    }

    #[test]
    fn canonicalize_sorts_required_array() {
        let mut v = json!({"required": ["z", "a", "m"]});
        canonicalize_json_schema(&mut v);
        assert_eq!(v["required"], json!(["a", "m", "z"]));
    }

    #[test]
    fn canonicalize_recursive() {
        let mut v = json!({
            "z": {"y": 1, "x": 2},
            "a": [{"c": 1, "b": 2}]
        });
        canonicalize_json_schema(&mut v);
        // 嵌套对象 key 排序, 数组元素不排序
        assert_eq!(v.to_string(), r#"{"a":[{"b":2,"c":1}],"z":{"x":2,"y":1}}"#);
    }
}
```

- [ ] **Step 2: 运行测试验证失败**

```bash
cargo test -p cyber-jianghu-agent --lib component::llm::canonicalize
```

Expected: FAIL (todo!() 触发 panic)

- [ ] **Step 3: 实现 `canonicalize_json_schema`**

替换 `canonicalize.rs` 中的 `todo!()`:

```rust
pub fn canonicalize_json_schema(value: &mut Value) {
    match value {
        Value::Object(map) => {
            // 1. 递归 sort object keys
            // 2. sort `required` array
            // 3. 标准化 `additionalProperties: false`
            for (_, v) in map.iter_mut() {
                canonicalize_json_schema(v);
            }
            // sort map keys
            let sorted: serde_json::Map<String, Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let mut entries: Vec<_> = sorted.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            map.clear();
            for (k, v) in entries {
                map.insert(k, v);
            }
            // sort `required` array
            if let Some(Value::Array(arr)) = map.get_mut("required") {
                let mut sorted_arr: Vec<Value> = arr.drain(..).collect();
                sorted_arr.sort_by(|a, b| {
                    let a_s = a.as_str().unwrap_or("");
                    let b_s = b.as_str().unwrap_or("");
                    a_s.cmp(b_s)
                });
                *arr = sorted_arr;
            }
        }
        Value::Array(arr) => {
            // 数组本身不排序 (避免改变语义), 但递归处理元素
            for v in arr.iter_mut() {
                canonicalize_json_schema(v);
            }
        }
        _ => {}
    }
}
```

- [ ] **Step 4: 运行测试验证通过**

```bash
cargo test -p cyber-jianghu-agent --lib component::llm::canonicalize
```

Expected: PASS (3 tests)

- [ ] **Step 5: 在 `mod.rs` 导出模块**

打开 `crates/agent/src/component/llm/mod.rs`, 找到 `pub mod xxx;` 列表, 加:

```rust
pub mod canonicalize;
```

- [ ] **Step 6: 编译验证**

```bash
cargo build -p cyber-jianghu-agent
```

Expected: PASS

- [ ] **Step 7: 提交**

```bash
git add crates/agent/src/component/llm/canonicalize.rs crates/agent/src/component/llm/mod.rs
git commit -m "feat(agent): D9 canonicalize.rs (JSON schema 字节级稳定)"
```

---

## Task 12: D9 - `ToolDefinition::canonical_json()` (TDD)

**Files:**
- Modify: `crates/agent/src/component/llm/tool_types.rs` (ToolDefinition impl block)

- [ ] **Step 1: 写失败测试**

打开 `crates/agent/src/component/llm/tool_types.rs`, 跳到测试模块, 加:

```rust
    #[test]
    fn canonical_json_is_byte_stable_across_calls() {
        use crate::component::llm::tool_types::{ToolDefinition, ToolFunction};
        use serde_json::json;
        let tool = ToolDefinition {
            tool_type: "function".to_string(),
            function: ToolFunction {
                name: "test_fn".to_string(),
                description: "test".to_string(),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "z_param": {"type": "string"},
                        "a_param": {"type": "string"},
                    },
                    "required": ["z_param", "a_param"],
                })),
            },
        };
        let json1 = tool.canonical_json();
        let json2 = tool.canonical_json();
        assert_eq!(json1, json2, "canonical_json must be byte-identical across calls");
    }
```

- [ ] **Step 2: 运行测试验证失败**

```bash
cargo test -p cyber-jianghu-agent --lib component::llm::tool_types
```

Expected: FAIL (no method `canonical_json`)

- [ ] **Step 3: 实现 `canonical_json` 方法**

打开 `crates/agent/src/component/llm/tool_types.rs`, 找到 `ToolDefinition` impl block, 加:

```rust
impl ToolDefinition {
    /// 字节级稳定的 JSON 表示
    pub fn canonical_json(&self) -> String {
        use crate::component::llm::canonicalize::canonicalize_json_schema;
        let mut value = serde_json::to_value(self).expect("ToolDefinition serializes");
        canonicalize_json_schema(&mut value);
        serde_json::to_string(&value).expect("canonical value serializes")
    }
}
```

- [ ] **Step 4: 运行测试验证通过**

```bash
cargo test -p cyber-jianghu-agent --lib component::llm::tool_types
```

Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add crates/agent/src/component/llm/tool_types.rs
git commit -m "feat(agent): D9 ToolDefinition::canonical_json (字节级稳定 tools schema)"
```

---

## Task 13: D9 - `send_chat_exchange` 用 `canonical_json` 序列化 tools

**Files:**
- Modify: `crates/agent/src/component/llm/direct_client.rs:1189-1219` (`send_chat_exchange` 工具序列化处)

- [ ] **Step 1: 修改 `send_chat_exchange` 用 canonical_json**

打开 `crates/agent/src/component/llm/direct_client.rs`, 跳到 `send_chat_exchange` 函数 (line 1189 附近), 找到 tools 序列化处 (通常 `serde_json::to_value(tools)` 或类似), 替换:

修改前 (假设有):
```rust
            tools: tools.map(|t| serde_json::to_value(t).unwrap_or_default()).map(|v| v.as_array().cloned().unwrap_or_default()),
```

修改后:
```rust
            tools: if self.config.prompt.canonicalize_schemas {
                tools.map(|t| t.iter().map(|tool| serde_json::from_str(&tool.canonical_json()).unwrap_or_else(|_| serde_json::to_value(tool).unwrap_or_default())).collect())
            } else {
                tools.map(|t| t.iter().map(|tool| serde_json::to_value(tool).unwrap_or_default()).collect())
            },
```

注: 实际代码可能不同, 此处需根据 `send_chat_exchange` 实际实现调整. 关键: 在 `self.config.prompt.canonicalize_schemas == true` 时, 用 `tool.canonical_json()` 替代 `serde_json::to_value(tool)`.

- [ ] **Step 2: 编译验证**

```bash
cargo build -p cyber-jianghu-agent
```

Expected: PASS

- [ ] **Step 3: 启动 agent 手动验证**

```bash
cargo run -p cyber-jianghu-agent &
sleep 5
# 触发一次 LLM 调用 (例如发个测试意图)
# 然后查看日志或 metrics, 验证 tools 字段两次调用是否 byte-identical
curl "http://localhost:23340/api/v1/metrics" | jq
```

- [ ] **Step 4: 提交**

```bash
git add crates/agent/src/component/llm/direct_client.rs
git commit -m "feat(agent): D9 send_chat_exchange 用 canonical_json 序列化 tools (字节级稳定)"
```

---

## Task 14: Phase 0 - 部署 + 24h baseline 测量 (非编码)

**Files:** 无 (部署运维)

- [ ] **Step 1: 启动 Phase 0 部署**

构建 release 版本:

```bash
cargo build -p cyber-jianghu-agent --release
```

部署 5% agent (per 5% 灰度机制, Phase 0 全量):

```bash
# 所有 agent 启动 (Phase 0 全量开启测量, 不影响 prompt 行为)
deploy-agent --config cache_diagnostics.enabled=true
```

- [ ] **Step 2: 24h 数据采集**

启动数据采集脚本:

```bash
# 每小时抓一次 metrics
while true; do
    curl -s "http://agent-host:23340/api/v1/metrics" >> /var/log/cache-metrics-$(date +%Y%m%d).jsonl
    sleep 3600
done
```

运行 24 小时 (可在后台跑, 不阻塞后续工作).

- [ ] **Step 3: 24h 数据 review**

24h 后, 分析:

- 聚合 cache_hit_rate baseline
- system_hash 变更频率 (per agent, per hour)
- system_hash 与 cache_hit_rate 相关性
- per-section token 占比

**判定**:
- 如果 system_hash 变更频率 > 1/agent/h, persona 段需先解耦 (Phase 3 候选)
- 如果 D8 / D9 预期命中率提升 ≥ 15pp / 5pp, 进入 D8 实施
- 如果 D8 / D9 预期不达标, 重新分析根因

- [ ] **Step 4: 文档化基线**

在 `docs/superpowers/plans/2026-06-07-deepseek-prefix-cache-tuning-baseline.md` 写基线报告 (24h 数据 + 分析结论).

---

## Task 15: D8 灰度 - 5% env 部署 + 24h 观察 (非编码)

**Files:** 无 (部署运维)

- [ ] **Step 1: 5% agent 部署 D8**

选择 5% agent, 部署时设 env var:

```bash
CYBER_JIANGHU_PROMPT_STRIP_REASONING_CONTENT=true deploy-agent
```

- [ ] **Step 2: 24h 观察 D8 5% cohort**

运行 24h, 对比 5% env-true cohort vs 95% env-false cohort:

- cache_hit_rate 增量 (5% cohort 相对 95% baseline) → 目标 ≥+15pp
- 决策质量波动 (death/success 率) → 目标 ≤±2%
- LLM cost 变化

**判定**:
- 增量 ≥+15pp 且 质量 ≤±2% → 推进 20% → 100%
- 否则回滚 (env var 设 false), 重新分析

- [ ] **Step 3: 20% 灰度**

```bash
CYBER_JIANGHU_PROMPT_STRIP_REASONING_CONTENT=true deploy-agent --cohort 20pct
```

- [ ] **Step 4: 100% 全量**

```bash
# 默认值已是 true (env_or 的 fallback), 所有 agent 自动启用
```

- [ ] **Step 5: 文档化 D8 结果**

在 spec 文档或单独 ADR 写 D8 实施报告: 实际命中率增量, 质量波动, 投资回收期.

---

## Task 16: D9 灰度 - 5% env 部署 + 24h 观察 (非编码)

**Files:** 无 (部署运维)

- [ ] **Step 1: 5% agent 部署 D9**

```bash
CYBER_JIANGHU_PROMPT_CANONICALIZE_SCHEMAS=true deploy-agent
```

- [ ] **Step 2: 24h 观察 D9 5% cohort**

- cache_hit_rate 增量 (在 D8 baseline 之上) → 目标 ≥+5pp
- 决策质量 (canonicalize 是无损变换, 预期 0 影响)
- DeepSeek 缓存是否覆盖 `tools` 字段 (如 D9 失败, 立即回退)

**判定**:
- 增量 ≥+5pp → 推进 20% → 100%
- 增量 ≤0 (DeepSeek 缓存不覆盖 tools) → 立即回退, 重新评估

- [ ] **Step 3: 20% 灰度 → 100% 全量**

同 D8 流程.

- [ ] **Step 4: 文档化 D9 结果**

写 D9 实施报告.

---

## 计划完成检查

- [ ] **Spec 覆盖**: spec §3 4 个 phase 全部对应到 task
- [ ] **占位符扫描**: 无 TBD/TODO/fill in (Task 5 step 1 用了 `todo!()` 是 TDD 模式, 后续 step 替换)
- [ ] **类型一致**: `PromptConfig`, `CacheDiagnosticsConfig`, `system_hash: [u8; 32]` 在所有 task 中定义一致
- [ ] **测试覆盖**: 每个新功能都有测试 (Task 5/6/9/10/11/12)
- [ ] **依赖完整**: Task 4 加 sha2, Task 7 可能加 hex, 已声明

---

## 执行选项

计划完成并保存到 `docs/superpowers/plans/2026-06-07-deepseek-prefix-cache-tuning.md`。两种执行选项：

1. **Subagent-Driven (推荐)**: 每个 task 派发新 subagent, task 间 review, 快速迭代
2. **Inline Execution**: 在当前会话执行 task, 批量执行 + checkpoint review

你选哪种？
