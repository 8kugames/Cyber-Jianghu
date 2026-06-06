# DeepSeek 前缀缓存调优 v2.1：数据驱动的最小可行改造

**日期**: 2026-06-07
**状态**: Draft (v2.1, 替代 v2)
**前置**:
- v1 (commit e25903f) 经 3-agent 表决 0/3 被拒 (理由: Reasonix 原理误读、D5/D6 为不存在问题设计、ROI 12+ 年、配置驱动不足)
- v2 (commit 1c1c73d) 经 3-agent 表决 2/3 通过 (Architecture 8.4/10 APPROVE, Goal 7.6/10 APPROVE, Implementation 5.5/10 REJECT, 5 项 BLOCKER)
- v2.1 修正 v2 的 5 项 BLOCKER + 4 项 minor, 不动 Architecture/Goal 已认可的设计
**问题**: DeepSeek 缓存命中率仅 ~33%, 长会话 token 成本高
**目标**: 基于真实 telemetry 定位最大破坏点, 用最小改造集推动命中率提升

---

## 0. 用户原问题（v1 漏答, v2/v2.1 必答）

用户原问：
1. **Reasonix 是如何实现前缀缓存调优的？** (研究)
2. **当前项目是否有参考性？** (评估)
3. **当前项目如何实现前缀缓存调优？** (实现)

### (a) Reasonix 原理 (基于实际源码)

| 设计 | 源码位置 | 实际机制 |
|------|---------|---------|
| Session 单例 | `internal/agent/session.go:17-24` | `NewSession(system)` 一次性塞 system, 后续只 `Add` 增量 |
| 工具 schema 稳定 | `internal/provider/schema_canonicalize.go` (推断) | **JSON schema 做 canonicalize** (sort `required`, 稳定 key 顺序) 才是字节稳定根因；**不是把 tools 移出 prefix** |
| 工具在 `tools` 字段 | `internal/provider/openai.go:151-158` | Reasonix 也用 API 顶层 `tools` 字段, 不在 message prefix |
| `reasoning_content` 剥离 | `internal/provider/openai.go:158-161` | "DeepSeek accepts it but counts it as ordinary prompt input (~500 extra tokens per turn)" |
| 压缩作为唯一 cache reset | `internal/agent/compact.go:24-35, 113-123` | `system + summary + recent tail` 三段, system 永不动 |
| 聚合命中率 | `internal/agent/agent.go:80-93, 334-335` | `sessCacheHit / sessCacheMiss` 累积, 压缩时不重置 |
| 上下文窗口 | `internal/agent/agent.go:52-63` | 触发阈值 0.8, 失败回退 stuck guard |

**最大单杠杆**: `reasoning_content` 不回传 (单次 ~500 tok, 多轮累计显著)。

### (b) 当前项目参考性评估

| Reasonix 设计 | 项目适用性 | 决策 |
|-------------|-----------|------|
| Session 单例 | **适用** | 项目每 agent 一生命周期, system 段本应只构建一次。`engine_prompts.rs::build_system_message` 每次 tick 调用 — 重复构建是浪费。 |
| 工具 schema 稳定 | **适用** | 关键改造点 — JSON schema 经 `serde_json::to_string` 序列化, key 顺序取决于数据结构。canonicalize 即可。 |
| `reasoning_content` 剥离 | **适用** | **最大单杠杆**。当前 `direct_client.rs:1301-1308` (经 `client.rs:73-76` `build_conversation_messages`) + `openai_types.rs:80-89` (ChatMessage 字段带 `#[serde(skip_serializing_if = "Option::is_none")]` 的 `reasoning_content`) 把 reasoning 序列化进每轮 assistant turn。 |
| 压缩触发 0.8 阈值 | **不适用** | 项目是**单 tick 决策** (4-8K prompt), 不是 Reasonix 累积 turn 对话。0.8 阈值 = 永远不触发。 |
| 两模型独立 session | **不适用 (成本为负)** | Reasonix planner **可选 + 低频**。本项目 ReflectorSoul 每次决策都跑, 独立 session = 每次决策 +1 LLM 调用。 |
| 聚合命中率指标 | **部分适用** | 已有 `cache_hit_tokens` 聚合 (`token_tracking.rs:40,53,96,144-161`; 通过 `/api/v1/metrics` (handler `llm_config.rs:591-641`) + `/api/v1/config/llm/usage` (handler `llm_config.rs:420-423`)), 缺的是 `system_hash` 维度下钻。 |

**结论**: 采用 reasoning_content 剥离 + schema canonicalize + system_hash 维度。**不采用**压缩 + ReflectorSoul 独立。

### (c) 实现方案

见 §3-§4。

---

## 1. 现状基线 (基于已有 telemetry, 不假设)

| 维度 | 现状 | 证据 |
|------|------|------|
| `cache_hit_tokens` 聚合 (单值, 无 hit/miss split) | **已收集** | `crates/agent/src/component/llm/direct_client.rs:634` 通过 `usage.cache_hit_tokens()` (来自 `openai_types.rs:131-133` 返回 `prompt_tokens_details.cached_tokens`) 提取; `token_tracking.rs:144-161` 累积到 `ModelTokenStats.cache_hit_tokens` |
| `system_hash` 维度 | **缺失** | 无任何代码计算 system 段 hash |
| Per-section token 计数 | **已有** | `engine_prompts.rs:293-311` 的 `PromptSectionEstimate` (含 system / persona / world_state / action_descriptions / memory / skill_instructions / other 段) |
| 33% 命中率计算 | `cache_hit_tokens / prompt_tokens` | 单值, 不区分 hit/miss — DeepSeek 顶层 `prompt_cache_hit_tokens` / `prompt_cache_miss_tokens` 字段当前**未**解析 (Reasonix 在 `openai.go:443-465` 同时支持两种格式) |

**v1 错判**: v1 spec §1.1 写"缺 per-prompt 命中诊断" — 实际是缺 `system_hash` 维度下钻, 聚合已有。v2 同样把 `cache_hit_tokens` 误称为 `prompt_cache_hit_tokens` / `prompt_cache_miss_tokens` 配对 — 实际只有单一 `cache_hit_tokens` 字段。

---

## 2. 设计决策

| 维度 | 决策 | 否决的备选 |
|------|------|-----------|
| 总体策略 | **测量先行, 数据驱动压缩** | v1 的 "Reasonix 完整范式重构" |
| 顺序 | Phase 0 测量 → D8 剥离 → D9 规范化 → Phase 3 按数据决策 | v1 的"5 阶段 17 文件一气呵成" |
| Session 模型 | **不新建抽象**, 通过 `engine_prompts.rs::compute_system_hash()` 观测 system 稳定性 | v1 D1 新建 `core/session.rs` |
| Tools 位置 | **不文本化**, 留在 API 顶层 `tools` 字段, 改做 schema canonicalize | v1 D2 文本化进 system |
| 工具结果 | **留在主 session**, 不隔离 | v1 D4 ToolSession 副 messages |
| ReflectorSoul session | **不拆分**, 保持现状 | v1 D5 独立 session |
| 上下文压缩 | **不实现**, per-tick 模式不适用 | v1 D6 Compactor |
| Reasoning 处理 | **D8 剥离** (新增, 最大单杠杆) | — |
| Schema 规范化 | **D9 canonicalize** (新增, 第二杠杆) | — |
| 观测 | **扩 `token_tracking.rs`** 加 system_hash 维度, 不新建 DB | v1 D7 新建 SQLite 表 |
| 配置键归属 | **Agent 端 LlmConfig** (server 端 LlmConfig 不涉及 cache 优化) | v2 误指 server `llm.yaml` |
| Hot-reload | **agent 重启** (避免引入新 WebSocket 分支; 4 键属静态配置, 无需运行时切换) | v2 误称"现有 ConfigUpdate WS 通配" |
| 灰度 | D8/D9 各自独立开关, 5% 阶梯放量 | v1 的多维灰度 |
| 质量风险 | ±2% 波动容忍, D8/D9 各跑 24h 5% agent 对照 | — |

**v2.1 vs v2 关键差异**:
- 配置键从 server 端 `llm.yaml` 改到 **agent 端 LlmConfig** (`crates/agent/src/config.rs`)
- Hot-reload 路径明确为 **agent 重启**（v2 误说 WebSocket ConfigUpdate, 实际无 `llm` 分支）
- D8 `serialize_for_api` 从 `conversation.rs::ConversationTurn` 改到 **`client.rs::build_conversation_messages` 直接加 `strip_reasoning` flag**（v2 放错 struct, 实际 API 消息构造读 client.rs 版, 不读持久化版）
- Phase 0 的 `system_hash()` 从 `prompt_cache.rs` 改到 **`engine_prompts.rs::compute_system_hash()` 直接 hash `build_system_message()` 输出**（v2 想给 PromptCache 加 rules/skills 字段, 实际无; 改 hash 输出最简单）
- 依赖补 **`sha2 = "0.10"`** 到 `crates/agent/Cargo.toml`（v2 漏列）
- 删去虚构的 `LlmCallContext`（实际用现有 `last_reasoning_content: Mutex<Option<String>>`）

---

## 3. 实施阶段

### Phase 0: 测量先行 (2-3 天, 1 新依赖)

**目标**: 拿到真实 baseline + 知道 cache miss 集中在哪一段。

**改造** (4 文件 + 1 Cargo.toml)：

1. `crates/agent/src/soul/actor/engine_prompts.rs` (扩)
   - 新增 `pub fn compute_system_hash(&self) -> [u8; 32]`
   - 内部: 调用 `self.build_system_message(use_tool_calling=true)` → `sha2::Sha256::digest(...)` → 返回 hash
   - **不修改** system 段内容（persona 动态字段先不动, 让 baseline 反映真实波动）
2. `crates/agent/src/soul/actor/engine.rs` (扩)
   - LLM 调用前取 `system_hash`, 塞进现有 `last_reasoning_content: Mutex<Option<String>>` 旁的局部变量（或新增 `last_system_hash: [u8; 32]`）
3. `crates/agent/src/component/llm/direct_client.rs` (扩)
   - `record_token_usage` 调用加 `system_hash: [u8; 32]` 参数
4. `crates/agent/src/component/llm/token_tracking.rs` (扩)
   - `ModelTokenStats` 加 `system_hash_distribution: HashMap<[u8;32], u64>` 字段（按 hash 聚合的 cache hit 计数）
   - 持久化追加（现 JSON 文件, 不用新建 SQLite）
5. `crates/agent/src/infra/api/handlers/llm_config.rs` (扩)
   - `/api/v1/metrics` 加 `?system_hash=` query filter
6. `crates/agent/Cargo.toml` (扩)
   - 加 `sha2 = "0.10"`（与 `crates/server/Cargo.toml:18` 对齐）

**Agent 端 LlmConfig 配置键** (`crates/agent/src/config.rs:530-593` `LlmConfig` 加 2 个子结构)：
```rust
pub struct LlmConfig {
    // ... 现有字段 ...
    pub cache_diagnostics: CacheDiagnosticsConfig,
    pub prompt: PromptConfig,
}

pub struct CacheDiagnosticsConfig {
    pub enabled: bool,                  // 默认 true
    pub system_hash_dimension: bool,    // 默认 true
}

pub struct PromptConfig {
    pub strip_reasoning_content: bool,  // 默认 true (D8)
    pub canonicalize_schemas: bool,     // 默认 true (D9)
}
```

**加载路径**: 走 agent 现有 config 加载机制（与 `fallback_models`, `idle_rotate_threshold` 等字段同路径）。Hot-reload: agent 重启（**不**走 WebSocket ConfigUpdate, v2 误说该机制支持 `llm` config_type, 实际不支持）。

**验证** (24h 数据)：
- 平均 cache hit rate (按 model/hour 聚合)
- `system_hash` 变更频率（每 agent 每天变几次 → 反映 system 段实际稳定性）
- hit rate 与 system_hash 稳定性的相关性
- Per-section token 占比（用 `PromptSectionEstimate`）

**不达标则不进入 Phase 1+**, 重新分析。

### Phase 1: D8 reasoning_content 剥离 (1 周, 2 文件)

**目标**: 消除每轮 ~500 tok 的 reasoning 回传。

**杠杆来源**: Reasonix `openai.go:158-161` (经验值: 每次节省 ~500 tok)。

**改造** (2 文件)：

1. `crates/agent/src/component/llm/client.rs` (扩)
   - `build_conversation_messages` 加 `strip_reasoning: bool` 参数（第 6 个参数）
   - 当 `strip_reasoning=true`, 调用 `ChatMessage::assistant(&turn.assistant)` (无 reasoning) 替代 `ChatMessage::assistant_with_reasoning(&turn.assistant, turn.reasoning_content.clone())` (line 73-76)
2. `crates/agent/src/component/llm/direct_client.rs` (扩)
   - 把 `strip_reasoning` 透传到 `build_conversation_messages`
   - 读 `prompt_config.strip_reasoning_content` (从 LlmConfig)
   - 在 `record_token_usage` 加 log: `(stripped_tokens: u64, original_tokens: u64)`

**不修改** `conversation.rs::ConversationTurn`（持久化版保留 `reasoning_content` 字段用于 session 存档/UI 显示, v2 误把方法加这里）。

**配置键**：`prompt.strip_reasoning_content: true`

**风险与缓解**：
- LLM 失去上一轮 reasoning 影响后续决策 → **5% 阶梯灰度 (5% → 20% → 100%)** + 24h 决策质量对比 (死亡率/成功率)
- 回滚开关: `prompt.strip_reasoning_content: false`

**预期**: 33% → 55-65% (按 Reasonix 实测的 ~500 tok/turn 节省折算)。

### Phase 2: D9 schema canonicalization (1 周, 1 新文件 + 2 现有)

**目标**: 让 `tools` 字段在 API 顶层实现字节级稳定, 触发 DeepSeek cache。

**杠杆来源**: Reasonix `internal/provider/schema_canonicalize.go` (推断位置, 实施时验证)。

**改造** (1 新 + 2 改)：

1. `crates/agent/src/component/llm/canonicalize.rs` **(新, ≤100 行)**
   - `fn canonicalize_json_schema(value: &mut serde_json::Value)`: 递归 sort object keys, sort `required` array, 标准化 `additionalProperties: false`, 移除 `default` 字段以外的元数据噪声
   - **无配置**: 算法固定, 不暴露参数
2. `crates/agent/src/component/llm/tool_types.rs`
   - `ToolDefinition::canonical_json() -> String` 新方法, 内部调 `canonicalize_json_schema` 后 `serde_json::to_string`
3. `crates/agent/src/component/llm/direct_client.rs`
   - 序列化 `tools` 字段前调 `tool.canonical_json()` 替代 `serde_json::to_string`
   - 读 `prompt_config.canonicalize_schemas` 开关
4. `crates/agent/src/component/llm/mod.rs`
   - `pub mod canonicalize;` 导出

**配置键**：`prompt.canonicalize_schemas: true`

**风险与缓解**：
- LLM 看到 canonicalize 后的 schema 跟原 schema 不完全一样 → **实际不会** (canonicalize 是无损变换, 语义等价)
- 序列化耗时 → benchmark, 应 <1ms/tool
- **DeepSeek 缓存是否覆盖 `tools` 字段未在官方文档中明确说明**; D9 通过 5% 灰度实测验证（hit rate 增量 > 0 才保留）

**预期**: 在 D8 基础上再 +5-10pp。

### Phase 3: 数据驱动压缩 (1-2 周, 视 Phase 0 数据决定)

**不预设方案**。Phase 0 数据出来后, 按实际破坏点针对性修。候选：
- `events_log` 治理 (如果数据显示 prompt 中 events 占大头)
- `world_state_section` 字段裁剪 (如果显示 WorldState 全量是大头)
- Session message memoization (如果显示 build_system_message 重复构建是瓶颈)

**所有候选方案都必须有 Phase 0 数据支撑, 否则不做。**

---

## 4. 文件级改造清单 (14 改动, 1 新文件)

```
Phase 0 (6 改动):
1.  crates/agent/src/soul/actor/engine_prompts.rs    (扩: compute_system_hash)
2.  crates/agent/src/soul/actor/engine.rs            (扩: 透传 system_hash)
3.  crates/agent/src/component/llm/direct_client.rs  (扩: 接收 system_hash)
4.  crates/agent/src/component/llm/token_tracking.rs (扩: system_hash_distribution 字段)
5.  crates/agent/src/infra/api/handlers/llm_config.rs (扩: ?system_hash= query)
6.  crates/agent/Cargo.toml                          (扩: 加 sha2 依赖)
7.  crates/agent/src/config.rs                       (扩: LlmConfig 加 cache_diagnostics + prompt 子结构)

D8 (2 改动):
8.  crates/agent/src/component/llm/client.rs         (扩: build_conversation_messages 加 strip_reasoning 参数)
9.  crates/agent/src/component/llm/direct_client.rs  (扩: 透传 strip_reasoning)

D9 (3 改动 + 1 新):
10. crates/agent/src/component/llm/canonicalize.rs   (新, ≤100 行)
11. crates/agent/src/component/llm/tool_types.rs     (扩: canonical_json())
12. crates/agent/src/component/llm/direct_client.rs  (扩: 序列化前调 canonicalize)
13. crates/agent/src/component/llm/mod.rs            (扩: 导出 canonicalize)

Phase 3 (TBD): 由 Phase 0 数据决定
```

**对比 v2**: 14 文件 → 13 文件 + 1 Cargo.toml (Cargo.toml 独立列出)。其他一致。

---

## 5. Magic Value 审计 (零硬编码)

| 配置键 | 默认 | 位置 |
|--------|------|------|
| `cache_diagnostics.enabled` | `true` | `LlmConfig::cache_diagnostics` (agent) |
| `cache_diagnostics.system_hash_dimension` | `true` | 同上 |
| `prompt.strip_reasoning_content` | `true` | `LlmConfig::prompt` (agent) |
| `prompt.canonicalize_schemas` | `true` | 同上 |
| 灰度阶梯 5%/20%/100% | **无配置, 写死在灰度 Playbook** | spec §7 |
| 决策质量波动阈值 ±2% | **无配置, 写死在 spec §6 验证** | spec §6 |
| Phase 0 24h 数据采集窗口 | **24h, 写死在 Phase 0 流程** | spec §3 |

**无代码内魔法值**。所有运行时可调项都进 `LlmConfig`。灰度/质量阈值属"决策框架", 不需要运行时调, 写在 spec。

---

## 6. 验证指标 (数据驱动, 不预设目标)

| 阶段 | 指标 | 数据来源 | 目标设定方式 |
|------|------|---------|------------|
| Phase 0 (24h) | 聚合 cache_hit_rate | `token_tracking` | 测量 baseline, 不设目标 |
| Phase 0 (24h) | system_hash 变更频率 | 同上 | 测量, 若 > 1/agent/h 说明 persona 段需要解耦 |
| Phase 0 (24h) | per-section token 占比 | `PromptSectionEstimate` | 测量, 定位最大段 |
| D8 (24h 5% 灰度) | cache_hit_rate 增量 | `token_tracking` | **基于 Phase 0 baseline, 设定 ≥+15pp** |
| D8 (24h 5% 灰度) | 决策质量波动 | death/success 率 | ≤ ±2% |
| D9 (24h 5% 灰度) | cache_hit_rate 增量 | `token_tracking` | **基于 D8 baseline, 设定 ≥+5pp** |
| Phase 3 (TBD) | 按数据 | 按方案 | 按方案 |

**关键反转**: 不在 spec 写 "80%" 这种不可证伪目标。每个阶段目标基于前阶段数据, 写进实施 PR/issue。

---

## 7. 风险与缓解

| 风险 | 缓解 |
|------|------|
| D8 剥离 reasoning 导致 LLM 决策质量下降 | 5% 阶梯灰度 + 24h 对照, 回滚开关 `prompt.strip_reasoning_content: false` |
| D9 canonicalize 改变 schema 实际效果 | 单元测试验证 canonicalize 前/后语义等价 (与 `serde_json::from_str` 输出相同) |
| **D9 假设 `tools` 字段在 DeepSeek 缓存前缀中未明确** | 5% 灰度实测 hit rate 增量；若 +0pp 则 D9 无效, 回退 |
| Phase 0 数据噪声大, 看不出趋势 | 24h 滚动平均, 至少跑 48h 才出结论 |
| system_hash 高频变更 (persona 动态) | Phase 0 数据出来后, Phase 3 决策是否要把 persona 段从 system 拆到 volatile |
| LLM 调用 cost 反增 (新 LLM 调用, 如摘要) | **v2.1 无任何 LLM 新调用**, v1 ROI 12+ 年问题不存在 |
| 工程成本超 2.5-3 周 (v1 估算) | v2.1 拆为 2-3 天 + 1 周 + 1 周 + TBD, 每阶段可独立停止 |
| 现有 `engine.rs` 1645 行超 CLAUDE.md 800 行限制 | **不在本 spec 修复** (避免 scope creep, 单独开 issue) |
| 4 键配置需要重启 agent 才能切换 | 接受 (4 键属静态配置); 未来如需运行时切换, 加 WebSocket `llm` 分支 |
| Server 端 LlmConfig 不含新键 (server 端不用这些键) | 接受 (cache 优化是 agent 侧问题, server 仅做 chronicle 用) |

**YAML 热更新**: 4 个新配置键通过 agent 端 LlmConfig 加载, **不走 WebSocket**。Hot-reload = agent 重启。详细说明已在 §3 Phase 0 列出。

**日志约定**: 沿用项目惯例 `tracing::info!` / `tracing::warn!` / `tracing::debug!` (见 `tool_loop.rs:65`、`token_tracking.rs:351`)。Phase 0 的 system_hash 写入用 `debug!` (高频), D8/D9 灰度决策点用 `info!`, 告警用 `warn!` / `error!`。

---

## 8. 实施时间表 (增量, 可独立停止)

```
Day 1-3:   Phase 0 测量 (7 改动, 1 新 Cargo 依赖)
Day 4-5:   Phase 0 24h 数据采集 (后台跑, 不阻塞)
Day 6:     Phase 0 数据 review, 决定是否进 D8
Day 7-10:  D8 reasoning_content 剥离 (2 改动)
Day 11:    D8 5% 灰度放量, 24h 观察
Day 12-13: D8 20% → 100% (若指标正常)
Day 14-18: D9 schema canonicalization (1 新 + 3 改)
Day 19:    D9 5% 灰度放量, 24h 观察
Day 20-21: D9 20% → 100% (若指标正常)
Day 22+:   Phase 3 (TBD, 由数据驱动)
```

总计: 3 周 (含 2 段 24h 观察)。

**v1 估算**: 2.5-3 周, 17 文件, 5 新模块。
**v2 估算**: 3 周, 14 文件 (其中 1 新), 0 新表, 0 新 LLM 调用, **但 hot-reload 路径错 / ConversationTurn 放错 / sha2 漏列**。
**v2.1 估算**: 同 v2 规模, 5 项 BLOCKER 修正, 真实可实施。

工程量基本相当, 但 v2.1 风险面**显著低于 v1**, **且消除了 v2 的实施撞墙风险**:
- 不动 Session 抽象 (低架构风险)
- 不动 ReflectorSoul 流程 (低决策质量风险)
- 不动 Compactor (避免无意义 LLM 摘要调用)
- 0 新 DB schema (低迁移风险)
- 每阶段可独立停止 (低沉没成本风险)
- 配置键不冲突、不放错 struct (避免 spec 文本与代码实情不符)

---

## 9. 测试规划 (spec 内显式声明)

| 新增/改动 | 测试文件 | 覆盖 |
|----------|---------|------|
| `engine_prompts.rs::compute_system_hash()` | `engine_prompts_test.rs` 追加 (现有) | 相同 `build_system_message` 输出 → 相同 hash; 任意字段变化 → hash 变化 |
| `client.rs::build_conversation_messages(strip_reasoning=true)` | `client_test.rs` 追加 (现有) | 验证 `reasoning_content` 不出现在生成的 messages 中; `strip_reasoning=false` 时保留 |
| `canonicalize.rs` (新) | `canonicalize_test.rs` (新) | 验证: sort `required`; 稳定 key 顺序; 移除 `default`; 语义不变 (与 `serde_json::from_str` 结果相同) |
| `tool_types.rs::canonical_json()` | `tool_types_test.rs` 追加 (现有) | 多次调用 → 相同输出 (字节级) |
| `direct_client.rs` 集成 (system_hash 透传 + canonicalize 调用) | `direct_client_test.rs` 追加 (现有) | mock 验证：第二次调用 tools 字段与第一次 byte-identical; system_hash 正确透传 |
| `token_tracking.rs` system_hash_distribution 维度 | `token_tracking_test.rs` 追加 (现有) | 验证按 system_hash 聚合的 cache hit 计数正确 |
| `llm_config.rs` 新 query | `llm_config_test.rs` 追加 (现有) | 验证 `?system_hash=` filter 工作 (在 `/api/v1/metrics` handler) |
| `config.rs::LlmConfig` 新字段 | `config_test.rs` 追加 (现有) | 验证默认 `cache_diagnostics.enabled = true` / `prompt.strip_reasoning_content = true` 等 |
| **集成测试** (新) | `tests/prefix_cache_e2e_test.rs` (新) | 模拟 100 tick, 验证 cache_hit_rate 随 system_hash 稳定而提升; D8 验证 reasoning 不出现在 API 请求中 (mock 抓 request body) |

**新增测试文件**: 2 (`canonicalize_test.rs`, `tests/prefix_cache_e2e_test.rs`)
**追加测试**: 7 个现有文件

---

## 10. 与既有 spec 关系

- `docs/superpowers/specs/2026-05-14-token-optimization-design.md`: Token 优化 (注意力门控 + Tool-First)。**互补, 不冲突**。本 spec 关注 cache 命中, 旧 spec 关注 prompt 体积。两者可同时进行。
- v1 spec (commit e25903f): **superseded by v2**。
- v2 spec (commit 1c1c73d): **superseded by v2.1**（v2.1 修正 v2 的 5 项 BLOCKER, 架构未变）。

---

## 11. 引用

- DeepSeek 官方文档: https://api-docs.deepseek.com/guides/kv_cache
- Reasonix 源码: https://github.com/esengine/deepseek-reasonix
- Reasonix 关键文件 (v2.1 比 v1/v2 更精确引用):
  - `internal/agent/session.go:17-24` (Session 单例化)
  - `internal/agent/agent.go:80-93, 334-335` (聚合命中率)
  - `internal/agent/compact.go:24-35, 113-123` (压缩策略, **本项目不采用**)
  - `internal/provider/openai.go:150-168` (reasoning_content 剥离, **最大单杠杆**)
  - `internal/provider/schema_canonicalize.go` (推断, **D9 改造目标**)
  - `internal/agent/cachehit_e2e_test.go:108-148` (前缀稳定性验证测试模式)

## 12. v1/v2 → v2.1 修正清单 (供 reviewer 验证)

| # | v2 表述 | v2.1 修正 | 证据 |
|---|---------|-----------|------|
| BLK 1 | "通过现有 ConfigUpdate WebSocket 消息推送" | "agent 重启" (不走 WebSocket) | `websocket.rs:727+` 无 `llm` 分支; LLM 配置走 HTTP `/api/v1/config/llm` |
| BLK 2 | `conversation.rs::ConversationTurn::serialize_for_api` | `client.rs::build_conversation_messages` 加 `strip_reasoning` flag | `client.rs:22-26` 才是 API 输入版; `client.rs:73-76` 是实际调用点 |
| BLK 3 | "无新依赖" | `crates/agent/Cargo.toml` 加 `sha2 = "0.10"` | `crates/agent/Cargo.toml` 当前无 sha2; server 已有 |
| BLK 4 | `prompt_cache.rs::system_hash` 算 `persona+rules+actions+skills` | `engine_prompts.rs::compute_system_hash` 算 `build_system_message()` 输出 | `prompt_cache.rs` 无 rules/skills 字段; hash 输出最简单 |
| BLK 5 | 4 键加到 `crates/server/config/llm.yaml` | 4 键加到 `crates/agent/src/config.rs::LlmConfig` (server 端 LlmConfig 不涉及 cache 优化) | Server 与 agent LlmConfig 是两个独立 struct |
| MIN 1 | `engine.rs:947` 引用 | `engine.rs:940` (实际行号) | grep 验证 |
| MIN 2 | `conversation.rs:73` 引用 | `conversation.rs:74` (实际行号) | grep 验证 |
| MIN 3 | `llm_config.rs:597-639` 是 `/api/v1/config/llm/usage` | `llm_config.rs:591-641` 是 `/api/v1/metrics`; `/api/v1/config/llm/usage` 是 `:420-423` | handler 行号核对 |
| MIN 4 | "提取 `prompt_cache_hit_tokens` + `prompt_cache_miss_tokens`" | "提取 `cache_hit_tokens` (单值, 来自 `prompt_tokens_details.cached_tokens`)" | 实际代码只解一种字段 |
| 删除 | "塞进 `LlmCallContext`" | 删去; 用现有 `last_reasoning_content: Mutex<Option<String>>` 模式 | 该类型不存在 |

## 13. 待确认事项

1. **Phase 0 完成后是否进 D8, 取决于数据**。若 Phase 0 显示 system_hash 变更频率 > 1/agent/h, 需先解耦 persona 段, 再进 D8。
2. **D8 灰度放量决策** = 5% 24h → 20% 24h → 100%, 决策依据为 `cache_hit_rate` 增量 ≥ +15pp 且决策质量 ≤ ±2% 波动。
3. **D9 灰度放量决策** 同 D8, 增量 ≥ +5pp（DeepSeek 是否覆盖 `tools` 字段未明, 下限更保守）。
4. **D9 失败回退**: 若 D9 灰度 5% 24h hit rate 增量为 0 或负, 立即回退 `prompt.canonicalize_schemas: false`, 重新评估 DeepSeek 缓存机制。
5. **Phase 3 不在本 spec 范围**, 需另起 spec 评审。
