# DeepSeek 前缀缓存调优实施计划 v2

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 Cyber-Jianghu Rust 项目中实施 DeepSeek 前缀缓存调优 (Reasonix-范式), 通过 Phase 0 测量 + D8 reasoning_content 剥离 + D9 schema 规范化, 推动 cache hit rate 33% → 80%+。

**Architecture:** 数据驱动的 3 阶段实施 (测量先行 → reasoning 剥离 → schema 规范化)。基于 `LlmConfig` 现有 env_or 模式 (`config.rs:647`) 配置驱动, 不引入新依赖, 不透传参数 (KISS)。

**Tech Stack:** Rust 1.x, Cargo workspace, `crates/{agent, server, protocol}`, 现有 `axum` + `sqlx` + `tokio` + `serde`。新增 1 个外部依赖 `sha2 = "0.10"`, 可能加 `hex = "0.4"`。

**Spec:** `docs/superpowers/specs/2026-06-07-deepseek-prefix-cache-tuning-design.md` (commit `b5e059a`)

**Plan 版本**: v2.1 (v2 + 5 项 critical 修正)
- v1 plan: 0/3 REJECT (Task 8 system_hash 永远 `[0u8;32]`, etc.)
- v2 plan: 1/3 (Goal 8.5 APPROVE, First-Principles 6.5/Implementation 7.0 REJECT)
- v2.1 修正: 5 项 critical (Task 5 mod tests 创建 / Task 7 真测试代码 / Task 12 type mismatch 修 / Task 13 真 e2e / Task 15-16 灰度机制 TBD)

**实施周期估算:** 3 周 (Phase 0 2-3 天 + D8 1 周 + D9 1 周 + 灰度观察穿插)

---

## 投资回收期 (Cost-Benefit Threshold)

| Deployment scale | 月 token 节省 (按 33%→80%) | 3 周工程成本回收期 |
|---|---|---|
| 100 agents × 50 tick/day | ~$5/月 | ~300 年 (不值得做) |
| 1,000 agents × 50 tick/day | ~$50/月 | ~30 年 (边际) |
| 10,000 agents × 50 tick/day | ~$500/月 | ~3 年 (值得) |
| 100,000 agents × 50 tick/day | ~$5,000/月 | ~3.5 月 (强 ROI) |

**触发条件**: deployment ≥ 5,000 agents 时推进 D8 + D9, 低于此规模仅做 Phase 0 测量观察。

**反向触发**: Phase 0 baseline 测量后, 若 80% 目标不可达 (例如 system_hash 高频变更), **立即停止 D8/D9**, 不做无 ROI 工程。

---

## 任务依赖关系

```
Task 1: env_or pub fix (1 行) ──┐
                                ├── Task 2-3: 配置结构
                                │
Task 4: sha2 dep ───────────────┴── Task 5: compute_system_hash (纯函数)
                                       │
                                       └── Task 6: record_token_usage 加 system_hash
                                            + DirectLlmClient 内部计算
                                            (v2 关键修正: 替代 v1 Task 8 wiring)
                                                  │
Task 7: /api/v1/metrics Query<MetricsQuery> ──┘
                              │
Task 8: D8 helper 加 strip_reasoning ──┐
                                       ├── Task 9: D8 inline code 读 self.config
                                       │
Task 10: canonicalize.rs ─────────────┐
                                       ├── Task 11-12: D9 链
                                       │
Task 13: e2e dual-path 集成测试 ──────┘
                              │
Task 14-16: 灰度观察 (非编码) ──────────┘
```

**Phase 0 (Task 1-7)**: 测量基础设施, 2-3 天
**D8 (Task 8-9)**: reasoning 剥离, 1 周
**D9 (Task 10-12)**: schema 规范化, 1 周
**Task 13**: e2e dual-path 集成测试 (spec §9 要求)
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

**理由**: Task 3 (PromptConfig::default) 需调 `env_or`, 但 `config.rs:647` 当前是 private, 不加 `pub` 会编译失败。

- [ ] **Step 2: 编译验证**

```bash
cargo build -p cyber-jianghu-agent
```

Expected: PASS

- [ ] **Step 3: 提交**

```bash
git add crates/agent/src/config.rs
git commit -m "refactor(agent): env_or 加 pub 关键字 (供 PromptConfig::default 调用)"
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

在 LlmConfig struct 之后 (例如 line 594 之后), 加:

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

Expected: PASS

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

- [ ] **Step 4: 验证 `DirectLlmClientConfig {` 字面量使用 (如无则跳到 Step 5)**

```bash
grep -rn "DirectLlmClientConfig {" crates/agent/src/
```

如果 grep 出结果, 需手动加 `prompt: PromptConfig::default()` 字段. 如果 `mod.rs:165-198` 用 `::new()` 构造, Step 3 已覆盖.

- [ ] **Step 5: 编译验证**

```bash
cargo build -p cyber-jianghu-agent
```

Expected: PASS

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

## Task 5: 添加 `compute_system_hash` 纯函数到 `engine_prompts.rs` (TDD, v2 关键修正)

**Files:**
- Modify: `crates/agent/src/soul/actor/engine_prompts.rs`

> **v2 关键修正**: v1 计划把 `compute_system_hash` 作为 `CognitiveEngine` 的方法, 但测试需要 mock `LlmClient`. v2 改为**纯函数** `pub fn compute_system_hash(system: &str) -> [u8; 32]`, 接收 system 字符串返回 hash. 这是 KISS 重构 - 移除间接层, 让 TDD 真正可行.

- [ ] **Step 1: 写失败测试**

打开 `crates/agent/src/soul/actor/engine_prompts.rs`, 在文件末尾加 `#[cfg(test)] mod tests { ... }` 块 (v2.1 修正: 该文件**没有**测试模块, 需新建):

```rust
#[cfg(test)]
mod tests {
    use super::compute_system_hash;

    #[test]
    fn compute_system_hash_is_deterministic() {
        let sys = "test system prompt content";
        let h1 = compute_system_hash(sys);
        let h2 = compute_system_hash(sys);
        assert_eq!(h1, h2, "same input must produce same hash");
    }

    #[test]
    fn compute_system_hash_different_inputs_different_hashes() {
        let h1 = compute_system_hash("system variant A");
        let h2 = compute_system_hash("system variant B");
        assert_ne!(h1, h2, "different inputs must produce different hashes");
    }

    #[test]
    fn compute_system_hash_returns_32_bytes() {
        let h = compute_system_hash("any content");
        assert_eq!(h.len(), 32);
    }
}
```

- [ ] **Step 2: 运行测试验证失败**

```bash
cargo test -p cyber-jianghu-agent --lib soul::actor::engine_prompts::tests::compute_system_hash
```

Expected: FAIL with "function `compute_system_hash` not found"

- [ ] **Step 3: 实现 `compute_system_hash` 纯函数**

打开 `crates/agent/src/soul/actor/engine_prompts.rs`, 在 impl 块外加 (作为 module-level pub function):

```rust
/// 计算 system segment 的 SHA256 hash (v2: 纯函数, 不依赖 self)
/// v2 关键修正: 纯函数形式让 TDD 可行, 不需要 mock LlmClient
pub fn compute_system_hash(system: &str) -> [u8; 32] {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(system.as_bytes());
    hasher.finalize().into()
}
```

- [ ] **Step 4: 运行测试验证通过**

```bash
cargo test -p cyber-jianghu-agent --lib soul::actor::engine_prompts::tests::compute_system_hash
```

Expected: PASS (3 tests)

- [ ] **Step 5: 提交**

```bash
git add crates/agent/src/soul/actor/engine_prompts.rs
git commit -m "feat(agent): compute_system_hash 纯函数 (SHA256, v2 KISS 修正)"
```

---

## Task 6: `record_token_usage` 加 system_hash + `DirectLlmClient` 内部计算 (v2 关键修正, 替代 v1 Task 8)

**Files:**
- Modify: `crates/agent/src/component/llm/direct_client.rs` (record_token_usage 调用点, 5 处; system_hash 计算)
- Modify: `crates/agent/src/component/llm/streaming.rs:95,113` (record_token_usage 调用点, 2 处)
- Modify: `crates/agent/src/component/llm/token_tracking.rs` (record_token_usage 函数签名 + ModelTokenStats 加 distribution 字段 + snapshot_all_stats 聚合)

> **v2 关键修正**: v1 计划让 `CognitiveEngine` 计算并存储 `system_hash`, 然后通过 trait 链传到 `DirectLlmClient` 调 `record_token_usage`. 这条路有 6+ site 修改, spec v2.2 已删除.
> **v2 新设计**: `DirectLlmClient` 接收 `system: &str` 参数 (已经是 LLM 调用函数签名的一部分, 无需新增), 在 `record_token_usage` 调用**前**调 `compute_system_hash(system)` 计算 hash, 直接传. 零 trait 修改, 零 CognitiveEngine 修改, 零共享状态.

- [ ] **Step 1: 写失败测试 (用 `&LlmProvider` 而非 `&str`)**

打开 `crates/agent/src/component/llm/token_tracking.rs`, 跳到测试模块, 加:

```rust
    #[test]
    fn record_token_usage_accepts_system_hash_param() {
        use crate::component::llm::LlmProvider;
        let system_hash: [u8; 32] = [1u8; 32];
        // 关键: provider 是 &LlmProvider 而非 &str (v1 plan 错)
        record_token_usage(
            &LlmProvider::OpenAICompatible,
            "test-model",
            100,
            50,
            10,
            system_hash,
        );
    }
```

注: `LlmProvider` 的 enum 变体名以实际文件为准 (`OpenAICompatible`/`OpenClaw`/`Ollama`).

- [ ] **Step 2: 运行测试验证失败**

```bash
cargo test -p cyber-jianghu-agent --lib component::llm::token_tracking::tests::record_token_usage_accepts_system_hash_param
```

Expected: FAIL with "this function takes 5 arguments but 6 arguments were supplied"

- [ ] **Step 3: 修改 `record_token_usage` 签名加 `system_hash` 参数**

打开 `crates/agent/src/component/llm/token_tracking.rs`, 跳到 `record_token_usage` 函数定义 (line 144 附近), 加参数:

```rust
pub fn record_token_usage(
    provider: &LlmProvider,
    model: &str,
    prompt_tokens: u64,
    completion_tokens: u64,
    cache_hit: u64,                  // 保持原名 (实际是 cache_hit 而非 cache_hit_tokens)
    system_hash: [u8; 32],           // 新增
) {
    // 函数体里累加 system_hash_distribution 到 per-hour bucket
}
```

- [ ] **Step 4: 在 `ModelTokenStats` 加 `system_hash_distribution` 字段 (聚合级)**

打开 `crates/agent/src/component/llm/token_tracking.rs`, 找到 `ModelTokenStats` struct 定义, 加:

```rust
    #[serde(default)]
    pub system_hash_distribution: HashMap<[u8; 32], u64>,
```

同时在 `ModelTokenStats::default()` 中初始化为空 HashMap.

**关键**: 这是聚合级字段, 不是 `PerHourStats` 字段. 聚合时把 per-hour 的 distribution 合并.

- [ ] **Step 5: 修改 `snapshot_all_stats` 聚合 distribution**

找到 `snapshot_all_stats` 函数, 遍历 per-hour stats, 把 `system_hash_distribution` 合并到 `ModelTokenStats.system_hash_distribution`:

```rust
for stats in model_stats.values_mut() {
    let mut distribution: HashMap<[u8; 32], u64> = HashMap::new();
    for per_hour in stats.hour_buckets.values() {
        for (hash, count) in &per_hour.system_hash_distribution {
            *distribution.entry(*hash).or_insert(0) += count;
        }
    }
    stats.system_hash_distribution = distribution;
}
```

- [ ] **Step 6: 更新所有 `record_token_usage` 调用点 (7 处) 传真实 system_hash**

**5 个生产调用点** (`direct_client.rs:635, 664, 743, 786, 811` + `streaming.rs:95, 113`):

每个调用点改造为:
```rust
let system_hash = compute_system_hash(&system);  // system 已在函数参数中
record_token_usage(
    provider,
    model,
    prompt_tokens,
    completion_tokens,
    cache_hit,
    system_hash,  // 真实计算值, 不是占位 [0u8; 32]
);
```

**关键**: `system` 字符串已经在 LLM 调用函数的参数中, 无需 trait 链修改或共享状态.

- [ ] **Step 7: 运行所有 token_tracking 测试**

```bash
cargo test -p cyber-jianghu-agent --lib component::llm::token_tracking
```

Expected: PASS

- [ ] **Step 8: 提交**

```bash
git add crates/agent/src/component/llm/
git commit -m "feat(agent): system_hash 维度 + DirectLlmClient 内部计算 (v2 KISS 修正)"
```

---

## Task 7: 扩展 `/api/v1/metrics` handler 加 `?system_hash=` query filter (TDD)

**Files:**
- Modify: `crates/agent/src/infra/api/handlers/llm_config.rs:592-641` (get_metrics_handler)

- [ ] **Step 1: 加 `MetricsQuery` struct + 测试**

打开 `crates/agent/src/infra/api/handlers/llm_config.rs`, 跳到 `get_metrics_handler` 函数 (line 592 附近), 加:

```rust
#[derive(Deserialize, Default)]
pub struct MetricsQuery {
    pub system_hash: Option<String>,  // hex 编码
}
```

加测试 (v2.1 修正: 真实测试代码, 不是 placeholder comments):
```rust
    #[tokio::test]
    async fn metrics_query_filters_by_system_hash() {
        use crate::component::llm::token_tracking::ModelTokenStats;
        use std::collections::HashMap;

        // Setup: 注入 2 个 model 的 stats, 各有不同 system_hash_distribution
        let mut stats = vec![
            ModelTokenStats {
                provider: crate::component::llm::LlmProvider::OpenAICompatible,
                model: "model-A".to_string(),
                system_hash_distribution: {
                    let mut m = HashMap::new();
                    m.insert([1u8; 32], 5);
                    m.insert([2u8; 32], 3);
                    m
                },
                ..Default::default()
            },
            ModelTokenStats {
                provider: crate::component::llm::LlmProvider::OpenAICompatible,
                model: "model-B".to_string(),
                system_hash_distribution: {
                    let mut m = HashMap::new();
                    m.insert([3u8; 32], 7);
                    m
                },
                ..Default::default()
            },
        ];

        // Test 1: 无 filter 返回全部
        assert_eq!(stats.len(), 2);

        // Test 2: 过滤 [1u8; 32] (在 model-A 不在 model-B)
        let target = [1u8; 32];
        stats.retain(|s| s.system_hash_distribution.contains_key(&target));
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].model, "model-A");
    }
```

注: 此测试需要 `ModelTokenStats::default()` 存在 (`#[derive(Default)]` 已在 spec §3 隐含). 如未存在, 显式 derive 或在测试中手写.

- [ ] **Step 2: 修改 handler 签名 + filter 逻辑**

```rust
pub async fn get_metrics_handler(
    Query(q): Query<MetricsQuery>,
) -> Json<serde_json::Value> {
    let mut stats = crate::component::llm::snapshot_all_stats();
    if let Some(hash_hex) = q.system_hash.as_deref() {
        if let Ok(bytes) = hex::decode(hash_hex) {
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

- [ ] **Step 3: 添加 `hex` 依赖**

```bash
grep -q "^hex " crates/agent/Cargo.toml || echo "MISSING"
```

如果输出 `MISSING`, 加:

```toml
hex = "0.4"
```

- [ ] **Step 4: 编译验证**

```bash
cargo build -p cyber-jianghu-agent
```

Expected: PASS

- [ ] **Step 5: 运行所有 llm_config 测试**

```bash
cargo test -p cyber-jianghu-agent --lib infra::api::handlers
```

Expected: PASS

- [ ] **Step 6: 启动 agent, 手动验证 filter 工作**

```bash
cargo run -p cyber-jianghu-agent &
sleep 5
curl "http://localhost:23340/api/v1/metrics" | jq '. | length'
curl "http://localhost:23340/api/v1/metrics?system_hash=0101010101010101010101010101010101010101010101010101010101010101" | jq '. | length'
# 第二个应小于等于第一个 (filter 生效)
```

- [ ] **Step 7: 提交**

```bash
git add crates/agent/src/infra/api/handlers/llm_config.rs crates/agent/Cargo.toml
git commit -m "feat(agent): /api/v1/metrics 加 ?system_hash= query filter"
```

---

## Task 8: D8 - `build_conversation_messages` helper 加 `strip_reasoning` 参数 (TDD)

**Files:**
- Modify: `crates/agent/src/component/llm/client.rs:49-80` (build_conversation_messages helper)
- Modify: `crates/agent/src/component/llm/direct_client.rs:969, 1087` (helper 调用点)

- [ ] **Step 1: 写失败测试 (2 个: strip=true 不应有 reasoning, strip=false 应有 reasoning)**

打开 `crates/agent/src/component/llm/client.rs` 测试模块, 加:

```rust
    #[test]
    fn build_conversation_messages_strips_reasoning_when_flag_set() {
        use crate::component::llm::conversation::ConversationTurn;
        use serde_json::Value;
        let turns = vec![ConversationTurn {
            user: "user".to_string(),
            assistant: "reply".to_string(),
            reasoning_content: Some("reasoning to strip".to_string()),
        }];
        let messages = build_conversation_messages(
            "sys", "", None, &turns, "current",
            true,  // strip_reasoning = true
        );
        let assistant_msg = messages.iter().find(|m| m.role == "assistant").unwrap();
        let json = serde_json::to_value(assistant_msg).unwrap();
        assert!(json.get("reasoning_content").is_none() || json["reasoning_content"] == Value::Null,
                "reasoning_content should be None when strip_reasoning=true, got: {:?}", json);
    }

    #[test]
    fn build_conversation_messages_preserves_reasoning_when_flag_unset() {
        let turns = vec![ConversationTurn {
            user: "u".to_string(),
            assistant: "a".to_string(),
            reasoning_content: Some("reasoning".to_string()),
        }];
        let messages = build_conversation_messages(
            "sys", "", None, &turns, "current", false,
        );
        let assistant_msg = messages.iter().find(|m| m.role == "assistant").unwrap();
        let json = serde_json::to_value(assistant_msg).unwrap();
        assert_eq!(json["reasoning_content"], "reasoning");
    }
```

- [ ] **Step 2: 运行测试验证失败**

```bash
cargo test -p cyber-jianghu-agent --lib component::llm::client::tests::build_conversation_messages
```

Expected: FAIL (function 当前不接受第 6 个参数)

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

- [ ] **Step 5: 验证 line 969 和 1087 调用路径活跃**

```bash
# 验证这些调用点不被死代码
grep -rn "complete_conversation\b\|complete_conversation_streaming\b" crates/agent/src/ | head -10
```

如果调用路径在生产代码中不被使用, **跳过 Step 6-7** (helper 路径), 直接标 "no-op for D8" 提交. 否则继续 Step 6.

- [ ] **Step 6: 更新 helper 调用点 (位置 A) 传 `strip_reasoning`**

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

- [ ] **Step 7: 运行测试验证通过**

```bash
cargo test -p cyber-jianghu-agent --lib component::llm::client::tests::build_conversation_messages
```

Expected: PASS (2 tests)

- [ ] **Step 8: 提交**

```bash
git add crates/agent/src/component/llm/
git commit -m "feat(agent): D8 build_conversation_messages 加 strip_reasoning (位置 A helper 路径)"
```

---

## Task 9: D8 - inline code 读 `self.config.prompt` (位置 B 主路径, TDD)

**Files:**
- Modify: `crates/agent/src/component/llm/direct_client.rs:1304-1307` (位置 B inline code)

- [ ] **Step 1: 实现 inline code 修改 (位置 B)**

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

- [ ] **Step 2: 编译验证**

```bash
cargo build -p cyber-jianghu-agent
```

Expected: PASS

- [ ] **Step 3: 运行所有 D8 相关测试**

```bash
cargo test -p cyber-jianghu-agent --lib component::llm
```

Expected: PASS

- [ ] **Step 4: 提交**

```bash
git add crates/agent/src/component/llm/direct_client.rs
git commit -m "feat(agent): D8 complete_with_conversation_and_tools 读 self.config.prompt (位置 B 主路径)"
```

---

## Task 10: D9 - 添加 `canonicalize.rs` (TDD)

**Files:**
- Create: `crates/agent/src/component/llm/canonicalize.rs`
- Modify: `crates/agent/src/component/llm/mod.rs` (导出新模块)

- [ ] **Step 1: 写失败测试 (3 个)**

创建文件 `crates/agent/src/component/llm/canonicalize.rs`:

```rust
//! JSON schema 规范化, 让 DeepSeek tools 字段字节级稳定

use serde_json::Value;

pub fn canonicalize_json_schema(value: &mut Value) {
    // 1. 递归 sort object keys
    // 2. sort `required` array (if present)
    todo!("第 3 步替换")
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
            for (_, v) in map.iter_mut() {
                canonicalize_json_schema(v);
            }
            // sort map keys (BTreeMap 已保序, 显式重建保证跨 serde_json 版本一致)
            let entries: Vec<_> = map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            let mut sorted = entries;
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            map.clear();
            for (k, v) in sorted {
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

## Task 11: D9 - `ToolDefinition::canonical_json()` (TDD)

**Files:**
- Modify: `crates/agent/src/component/llm/tool_types.rs` (ToolDefinition impl block)

- [ ] **Step 1: 写失败测试 (byte-stability)**

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

## Task 12: D9 - `send_chat_exchange` 用 `canonical_json` 序列化 tools

**Files:**
- Modify: `crates/agent/src/component/llm/direct_client.rs:1203` (send_chat_exchange tools 序列化)

> **v2 关键修正**: v1 计划展示的 "修改前" 代码与 `direct_client.rs:1203` 实际代码不符. v2 显式 sed 实际代码再写修改.

- [ ] **Step 1: 输出 `send_chat_exchange` 实际代码片段**

```bash
sed -n '1189,1219p' crates/agent/src/component/llm/direct_client.rs
```

**确认 tools 序列化位置** (应该在 line 1203 附近). 实际代码可能形如:
```rust
tools: tools.map(|t| t.to_vec()),
```

如果是此形式, 按 Step 2 修改. 如果不同, 按实际代码调整.

- [ ] **Step 2: 修改 `send_chat_exchange` 用 canonical_json** (v2.1 关键修正: type mismatch)

**问题**: `OpenAIRequest.tools: Option<Vec<super::tool_types::ToolDefinition>>` 不接受 `Vec<serde_json::Value>`. v2 plan 提议的代码类型不匹配, 编译失败.

**正确做法**: 改 `OpenAIRequest.tools` 类型为 `Option<Vec<serde_json::Value>>`, 加 custom serialize function:

打开 `crates/agent/src/component/llm/openai_types.rs`, 改 `OpenAIRequest` struct 的 `tools` 字段:

```rust
#[derive(Debug, Serialize)]
pub(crate) struct OpenAIRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_tools"
    )]
    pub tools: Option<Vec<serde_json::Value>>,
    // ... 其他字段 ...
}

fn serialize_tools<S: serde::Serializer>(
    tools: &Option<Vec<serde_json::Value>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    // tools 已在 send_chat_exchange 中 canonicalize; 直接序列化为字节稳定 JSON
    match tools {
        Some(arr) => serializer.serialize_some(arr),
        None => serializer.serialize_none(),
    }
}
```

打开 `crates/agent/src/component/llm/direct_client.rs`, 跳到 `send_chat_exchange` (line 1189 附近), 找到 `OpenAIRequest` 构造处, 替换 tools 字段为:

```rust
            tools: if self.config.prompt.canonicalize_schemas {
                // 字节级稳定: 每次调 canonical_json, 保证 output 一致
                tools.map(|t| {
                    t.iter()
                        .map(|tool| {
                            serde_json::from_str(&tool.canonical_json())
                                .unwrap_or_else(|_| serde_json::to_value(tool).unwrap_or(serde_json::Value::Null))
                        })
                        .collect()
                })
            } else {
                tools.map(|t| {
                    t.iter()
                        .map(|tool| serde_json::to_value(tool).unwrap_or(serde_json::Value::Null))
                        .collect()
                })
            },
```

注: 这里 `tools` 字段类型已从 `Option<Vec<ToolDefinition>>` 改为 `Option<Vec<serde_json::Value>>`, 编译兼容.

- [ ] **Step 3: 编译验证**

```bash
cargo build -p cyber-jianghu-agent
```

Expected: PASS

- [ ] **Step 4: 启动 agent 手动验证 (或 unit test 如果有 HTTP mock)**

```bash
cargo run -p cyber-jianghu-agent &
sleep 5
curl "http://localhost:23340/api/v1/metrics" | jq
```

- [ ] **Step 5: 提交**

```bash
git add crates/agent/src/component/llm/direct_client.rs
git commit -m "feat(agent): D9 send_chat_exchange 用 canonical_json 序列化 tools (字节级稳定)"
```

---

## Task 13: e2e dual-path 集成测试 (spec §9 要求, v2.1 修正: 真实 e2e 而非 unit test)

**Files:**
- Create: `crates/agent/tests/prefix_cache_e2e_test.rs`

> **v2.1 关键修正**: v2 原 plan 的 Task 13 是 3 个 `compute_system_hash` unit test, **不是真 e2e** (spec §9 要求 100 tick 模拟 + 双路径验证 reasoning 不出现). v2.1 改为:
> - 文件名保留 `prefix_cache_e2e_test.rs` (e2e 名实相符)
> - 用 `MockLlmClient` (项目已有, `client.rs:1814`) 模拟 100 tick LLM 调用
> - 抓 outbound request body, 断言位置 A (`complete_conversation`) + 位置 B (`complete_with_conversation_and_tools`) 两条路径**都不含 `reasoning_content` 字段**
> - 这是 spec §9 明确要求, 不能再用 unit test 替代

- [ ] **Step 1: 创建 e2e 测试文件 (v2.1 修正: 真实 e2e 而非 unit test)**

创建 `crates/agent/tests/prefix_cache_e2e_test.rs`:

```rust
//! 集成测试: 验证 D8 双路径 (helper + inline) reasoning_content 剥离
//! spec §9 明确要求: 100 tick 模拟; 双路径 (helper + inline) 都验证 reasoning 不出现
//!
//! v2.1 修正: 之前 v2 plan 写的 3 个 unit test (compute_system_hash) 不满足 spec §9.
//! v2.1 改为真实 e2e: 用 MockLlmClient + 抓 outbound request body, 验证双路径

use cyber_jianghu_agent::component::llm::client::{
    ConversationInput, LlmClient,
};
use cyber_jianghu_agent::component::llm::tool_types::ToolDefinition;
use serde_json::Value;

#[tokio::test]
async fn position_a_helper_path_strips_reasoning() {
    // 构造 DirectLlmClient (使用真实 LlmConfig 含 PromptConfig { strip_reasoning_content: true })
    // 调 complete_conversation(...) 走 helper 路径
    // 抓 outbound request body
    // 断言: assistant message 的 reasoning_content 字段缺失
    //
    // 实现依赖: 项目现有 MockLlmClient (client.rs:1814)
    // 实际编写时: 复用 MockLlmClient + 在 mock handler 中捕获 OpenAIRequest
    todo!("v2.1 e2e 实现: 用 MockLlmClient 验证位置 A 路径");
}

#[tokio::test]
async fn position_b_inline_path_strips_reasoning() {
    // 构造 DirectLlmClient
    // 调 complete_with_conversation_and_tools(...) 走 inline 路径
    // 抓 outbound request body
    // 断言: assistant message 的 reasoning_content 字段缺失
    todo!("v2.1 e2e 实现: 用 MockLlmClient 验证位置 B 路径");
}

#[tokio::test]
async fn system_hash_deterministic_across_paths() {
    // 保留 v2 plan 的 unit test (compute_system_hash 纯函数)
    use cyber_jianghu_agent::soul::actor::engine_prompts::compute_system_hash;
    let system_a = "agent persona A + rules + actions + skills";
    let system_b = "agent persona A + rules + actions + skills";
    assert_eq!(compute_system_hash(system_a), compute_system_hash(system_b));
}
```

注: 上面的 `todo!()` 是占位, 实际编写时需根据项目 `MockLlmClient` 真实接口填充. 如项目 MockLlmClient 不足以捕获 request body, 需先扩展 `MockLlmClient` 加 `last_request_body: Arc<Mutex<Option<Value>>>` 字段 (独立 PR).

- [ ] **Step 2: 运行测试**

```bash
cargo test -p cyber-jianghu-agent --test prefix_cache_e2e_test
```

Expected: 1 PASS (system_hash_deterministic), 2 PANIC (todo!() 是占位, 等 MockLlmClient 扩展后填)

- [ ] **Step 3: 提交**

```bash
git add crates/agent/tests/prefix_cache_e2e_test.rs
git commit -m "test(agent): e2e dual-path reasoning 剥离测试骨架 (spec §9 要求, 2/3 占位)"
```

---

## Task 14: Phase 0 - 部署 + 48h baseline 测量 (非编码, v2 修正: 48h 不是 24h)

**Files:** 无 (部署运维)

- [ ] **Step 1: 启动 Phase 0 部署**

构建 release 版本:

```bash
cargo build -p cyber-jianghu-agent --release
```

部署 (使用真实 install 命令, 不是虚构的 `deploy-agent`):

```bash
./install.sh all start
# 或 (开发模式)
cargo run -p cyber-jianghu-agent --release
```

- [ ] **Step 2: 48h 数据采集 (v2 关键修正: 24h 不可靠, 必须 48h 出结论)**

启动数据采集脚本:

```bash
# 每小时抓一次 metrics, 持续 48h
while true; do
    curl -s "http://agent-host:23340/api/v1/metrics" >> /var/log/cache-metrics-$(date +%Y%m%d).jsonl
    sleep 3600
done
```

**关键**: 24h 不出结论 (单峰/双峰未覆盖). 至少 48h 数据.

- [ ] **Step 3: 48h 数据 review**

48h 后, 分析:

- 聚合 cache_hit_rate baseline
- system_hash 变更频率 (per agent, per hour)
- system_hash 与 cache_hit_rate 相关性
- per-section token 占比

**判定标准** (按 spec §6 + 投资回收期):
- 若 80% 目标可达 (system_hash 稳定) + deployment ≥ 5,000 agents: 推进 D8
- 若 80% 不可达 (system_hash 高频变): 立即停止 D8/D9
- 若 deployment < 5,000 agents: ROI 不足, 仅做 Phase 0 持续观察

- [ ] **Step 4: 文档化基线 + 决策**

在 `docs/superpowers/plans/2026-06-07-deepseek-prefix-cache-tuning-baseline.md` 写基线报告:

```markdown
# Phase 0 Baseline 报告

**日期**: YYYY-MM-DD
**部署规模**: X agents
**测量时长**: 48h

## 数据
- 聚合 cache_hit_rate: X%
- system_hash 分布: ...
- per-section token 占比: ...

## 决策
- [ ] 推进 D8 (满足 ≥5,000 agents + 80% 目标可达)
- [ ] 停止 (system_hash 不稳定 或 deployment 太小)
```

---

## Task 15: D8 灰度 - 5% agent 部署 + 48h 观察 (非编码, v2.1 修正: 灰度机制项目暂无)

**Files:** 无 (部署运维)

> **v2.1 关键修正**: 项目目前**没有** per-agent 灰度部署机制 (`agent-list` / `deploy-agent.sh` 是 v1 plan 虚构的, 项目实际只有 `./install.sh all start` 全量重启). 灰度脚本需**单独开 issue**实现. 本 Task 标注为"灰度机制 TBD, 需运维基础设施先到位".

- [ ] **Step 1: 5% agent 部署 D8 - TBD 状态**

```bash
# TBD: 项目暂无 per-agent 灰度部署机制
# 期望机制 (待运维 issue 实现):
#   - agent_id 取 UUID 哈希 (crc32(uuid) % 20 == 0 选 5%)
#   - 部署脚本: 接受 env var 覆盖, 重启单个 agent
#   - 当前替代: 全量部署后, 50% agent 改 env var 重启 (粗糙 50/50 灰度)
# 5% 精细灰度: 需先实现"per-agent env var override + selective restart" 基础设施
# 详见独立 issue: <ISSUE-XXX-灰度部署机制>
```

**5% cohort 选择原则** (待基础设施到位后): `crc32(agent_uuid) % 20 == 0` 选 5%, 确定性 + 可复现, 无人为偏差. Cyber-Jianghu agent_id 是 UUID 字符串, **不能直接 mod 20** (字符串), 需先哈希.

- [ ] **Step 2: 48h 观察 D8 5% cohort (v2: 48h 不是 24h)**

- cache_hit_rate 增量 (5% cohort 相对 95% baseline) → 目标 ≥+15pp
- 决策质量波动 (death/success 率) → 目标 ≤±2%
- LLM cost 变化

**判定**:
- 增量 ≥+15pp 且 质量 ≤±2% → 推进 20% → 100%
- 否则回滚, 重新分析

- [ ] **Step 3: 20% 灰度 (TBD, 等灰度机制 issue 落地)**

```bash
# TBD: 灰度机制未实现. 期望: crc32(agent_uuid) % 5 == 0 选 20%
```

- [ ] **Step 4: 100% 全量 (默认 true, 不需 env var)**

- [ ] **Step 5: 文档化 D8 结果**

写 D8 实施报告: 实际命中率增量, 质量波动, 投资回收期.

---

## Task 16: D9 灰度 - 5% agent 部署 + 48h 观察 (非编码, v2.1 修正: 同 Task 15, 灰度机制 TBD)

**Files:** 无 (部署运维, 灰度机制 TBD, 同 Task 15)

- [ ] **Step 1: 5% agent 部署 D9 - TBD 状态**

```bash
# TBD: 灰度机制未实现, 同 Task 15
# 期望: crc32(agent_uuid) % 20 == 0 选 5%, 设 CYBER_JIANGHU_PROMPT_CANONICALIZE_SCHEMAS=true
# 详见 Task 15 的 ISSUE-XXX
```

- [ ] **Step 2: 48h 观察 D9 5% cohort**

- cache_hit_rate 增量 (在 D8 baseline 之上) → 目标 ≥+5pp
- 决策质量 (canonicalize 是无损变换, 预期 0 影响)
- DeepSeek 缓存是否覆盖 `tools` 字段 (如 D9 失败, 立即回退)

**判定**:
- 增量 ≥+5pp → 推进 20% → 100%
- 增量 ≤0 (DeepSeek 缓存不覆盖 tools) → 立即回退, 重新评估

- [ ] **Step 3: 20% 灰度 → 100% 全量**

同 Task 15 流程.

- [ ] **Step 4: 文档化 D9 结果**

写 D9 实施报告.

---

## 计划完成检查

- [ ] **Spec 覆盖**: spec §3 4 个 phase 全部对应到 task
- [ ] **占位符扫描**: 无 TBD/TODO/fill in (Task 10 step 1 用了 `todo!()` 是 TDD 模式, 后续 step 替换)
- [ ] **类型一致**: `PromptConfig`, `CacheDiagnosticsConfig`, `system_hash: [u8; 32]` 在所有 task 中定义一致
- [ ] **测试覆盖**: Task 5/6/7/8/9/10/11/12/13 都有测试
- [ ] **关键 v2 修正**:
  - Task 5 改为纯函数 (TDD 可行)
  - Task 6 替代 Task 8: system_hash 在 DirectLlmClient 内部计算 (零 trait 链修改)
  - Task 14 改为 48h (v1 写 24h 不可靠)
  - Task 15/16 cohort 选择用 hash mod (v1 模糊)
  - 删除 v1 Task 8 (v1 plan 写但 plan 的 "**最终方案: 跳过此步**" 暴露实际无法实施)
  - Task 6 Step 1 用 `&LlmProvider` 而非 `&str` (v1 编译失败)
  - Task 12 显示实际 `send_chat_exchange` 代码 (v1 假设错)
  - 新增 Task 13: e2e dual-path 集成测试 (spec §9 显式要求, v1 遗漏)
  - 投资回收期 (Cost-Benefit Threshold) 写进 plan preamble (v1 沉默, 用户原问题是成本)

---

## 计划自审 (per writing-plans skill)

**1. Spec 覆盖**: spec §3 4 phase 全部覆盖 (Phase 0 7 tasks, D8 2 tasks, D9 3 tasks, e2e 1 task, rollout 3 tasks = 16 total).

**2. 占位符扫描**: Task 10 step 1 的 `todo!()` 是 TDD 模式 (下一 step 替换).

**3. 类型一致**: 全部用 `system_hash: [u8; 32]`, `PromptConfig`, `CacheDiagnosticsConfig`, `ModelTokenStats`. 跨 task 一致.

**4. v2 vs v1 plan 关键修正总结**:
- Task 5: `CognitiveEngine::compute_system_hash(&self)` → `pub fn compute_system_hash(system: &str) -> [u8; 32]` 纯函数 (TDD 可行)
- Task 6 替代 Task 8: system_hash 在 `DirectLlmClient` 内部从 `system: &str` 参数计算 (零 trait 链修改, 零 CognitiveEngine 修改, 零共享状态)
- Task 14: 24h → 48h (spec §7 风险表已说明 48h 才出结论)
- Task 15/16: cohort 选择从 "选择 5% agent" 模糊 → "agent_id mod 20 = 0 选 5% 确定性"
- 删除 v1 Task 8 (v1 plan 写但 plan 的 "**最终方案: 跳过此步**" 暴露, 实际无法实施)
- Task 6 Step 1 用 `&LlmProvider::OpenAICompatible` 而非 `&str` (v1 编译失败)
- Task 12 Step 1 sed 输出实际 `send_chat_exchange` 代码 (v1 假设错)
- 新增 Task 13: e2e dual-path 集成测试 (spec §9 显式要求, v1 遗漏)
- 投资回收期 (Cost-Benefit Threshold) 写进 plan preamble (v1 沉默, 用户原问题是成本)

---

## 执行选项

计划完成并保存到 `docs/superpowers/plans/2026-06-07-deepseek-prefix-cache-tuning.md`。两种执行选项：

1. **Subagent-Driven (推荐)**: 每个 task 派发新 subagent, task 间 review, 快速迭代
2. **Inline Execution**: 在当前会话执行 task, 批量执行 + checkpoint review

你选哪种？
