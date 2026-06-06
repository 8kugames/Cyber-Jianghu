# DeepSeek 前缀缓存调优 v2.2：数据驱动的最小可行改造

**日期**: 2026-06-07
**状态**: Draft (v2.2, 替代 v2.1)
**前置**:
- v1 (e25903f) 0/3 REJECT
- v2 (1c1c73d) 2/3 通过 (Implementation 5.5/10 REJECT)
- v2.1 (1a95fd2) 1/3 通过 (Architecture 7.0/10 REJECT, Implementation 6.5/10 REJECT, Goal 7.7/10 APPROVE)
- v2.2 修正 v2.1 的 2 项 CRITICAL + 3 项 MINOR, 不动架构决策
**问题**: DeepSeek 缓存命中率仅 ~33%
**目标**: 基于真实 telemetry 定位最大破坏点, 用最小改造集推动命中率提升

---

## 0. 用户原问题（v1 漏答, v2/v2.1/v2.2 必答）

1. **Reasonix 是如何实现前缀缓存调优的？** (研究)
2. **当前项目是否有参考性？** (评估)
3. **当前项目如何实现前缀缓存调优？** (实现)

### (a) Reasonix 原理 (基于实际源码)

| 设计 | 源码位置 | 实际机制 |
|------|---------|---------|
| Session 单例 | `internal/agent/session.go:17-24` | `NewSession(system)` 一次性塞 system, 后续只 `Add` 增量 |
| 工具 schema 稳定 | `internal/provider/schema_canonicalize.go` (推断) | **JSON schema 做 canonicalize** (sort `required`, 稳定 key 顺序) |
| 工具在 `tools` 字段 | `internal/provider/openai.go:151-158` | Reasonix 也用 API 顶层 `tools` 字段 |
| `reasoning_content` 剥离 | `internal/provider/openai.go:158-161` | "DeepSeek accepts it but counts it as ordinary prompt input (~500 extra tokens per turn)" |
| 压缩作为唯一 cache reset | `internal/agent/compact.go:24-35, 113-123` | `system + summary + recent tail` 三段 |
| 聚合命中率 | `internal/agent/agent.go:80-93, 334-335` | `sessCacheHit / sessCacheMiss` 累积 |

**最大单杠杆**: `reasoning_content` 不回传 (~500 tok/turn 节省)。

### (b) 当前项目参考性评估

| Reasonix 设计 | 项目适用性 | 决策 |
|-------------|-----------|------|
| Session 单例 | 适用 | 每 tick 重复构建 system 段是浪费, 但本 spec 暂不抽象 Session |
| 工具 schema 稳定 | 适用 | D9 canonicalize 即可 |
| `reasoning_content` 剥离 | 适用, **最大杠杆** | 当前 `direct_client.rs:1304-1307` (inline) + `client.rs:73-76` (helper) + `openai_types.rs:80-89` (字段定义) 把 reasoning 序列化进每轮 assistant turn |
| 压缩触发 0.8 阈值 | **不适用** | per-tick 4-8K, 永远不触发 |
| 两模型独立 session | **不适用 (成本为负)** | ReflectorSoul 每次决策都跑, 拆 = +1 LLM 调用/tick |
| 聚合命中率指标 | 部分适用 | 已有 `cache_hit_tokens` 聚合 |

**结论**: 采用 reasoning_content 剥离 + schema canonicalize + system_hash 维度。**不采用**压缩 + ReflectorSoul 独立。

### (c) 实现方案

见 §3-§4。

---

## 1. 现状基线

| 维度 | 现状 | 证据 |
|------|------|------|
| `cache_hit_tokens` 单值聚合 | 已收集 | `direct_client.rs:634` `usage.cache_hit_tokens()` (来自 `openai_types.rs:131-133` `prompt_tokens_details.cached_tokens`); `token_tracking.rs:144-161` `record_token_usage` |
| `system_hash` 维度 | 缺失 | 无 |
| Per-section token 计数 | 已有 | `engine_prompts.rs:293-311` `PromptSectionEstimate` |

---

## 2. 设计决策

| 维度 | 决策 | 否决的备选 |
|------|------|-----------|
| 总体策略 | 测量先行, 数据驱动压缩 | v1 "Reasonix 完整范式重构" |
| 顺序 | Phase 0 测量 → D8 剥离 → D9 规范化 → Phase 3 按数据决策 | v1 "5 阶段一气呵成" |
| Tools 位置 | 不文本化, 留 API 顶层, 做 schema canonicalize | v1 D2 文本化 |
| ReflectorSoul session | 不拆分 | v1 D5 独立 |
| 上下文压缩 | 不实现, per-tick 不适用 | v1 D6 Compactor |
| Reasoning 处理 | D8 剥离 (最大单杠杆) | — |
| Schema 规范化 | D9 canonicalize (第二杠杆) | — |
| 观测 | 扩 `token_tracking.rs` 加 system_hash 维度, 不新建 DB | v1 D7 新表 |
| 配置键归属 | Agent 端 LlmConfig | v2 误指 server `llm.yaml` |
| Hot-reload | **agent 重启** (WS 无 `llm` 分支) | v2 误说 ConfigUpdate WS |
| 灰度 | **部署时 env var 区分** (`CYBER_JIANGHU_PROMPT_STRIP_REASONING_CONTENT` 5% agent 设 true) | v2/v2.1 未明 |

**v2.2 vs v2.1 关键差异**:
- D8 实施**双路径**同步: `client.rs::build_conversation_messages` (helper) + `direct_client.rs:1283-1308` (`complete_with_conversation_and_tools` inline code), 两处都加 `strip_reasoning` 参数透传
- `ChatMessage::assistant` 构造器缺失 → 改用现有 `ChatMessage::assistant_with_reasoning(&content, None)` (语义等价)
- 5% 阶梯机制明确: **env var 部署时区分**, 不是运行时随机采样
- `compute_system_hash` 动态跟随 `llm_client.supports_tool_calling()`, 不写死 `true`
- `crates/server/Cargo.toml:18` → `:47` (sha2 实际行)

---

## 3. 实施阶段

### Phase 0: 测量先行 (2-3 天, 1 新依赖)

**目标**: 拿到真实 baseline + 知道 cache miss 集中在哪一段。

**改造** (5 文件 + 1 Cargo.toml + 1 LlmConfig 扩展)：

1. `crates/agent/src/soul/actor/engine_prompts.rs` (扩)
   - 新增 `pub(super) fn compute_system_hash(&self) -> [u8; 32]`
   - 内部: `let use_tool = self.llm_client.supports_tool_calling(); let sys = self.build_system_message(use_tool); sha2::Sha256::digest(sys.as_bytes()).into()` — **动态跟随** use_tool_calling 状态
2. `crates/agent/src/soul/actor/engine.rs` (扩)
   - LLM 调用前取 `system_hash`, 存新增 `last_system_hash: [u8; 32]` (与现有 `last_reasoning_content: Mutex<Option<String>>` 同级)
3. `crates/agent/src/component/llm/direct_client.rs` (扩)
   - `record_token_usage` 加 `system_hash: [u8; 32]` 参数
4. `crates/agent/src/component/llm/token_tracking.rs` (扩)
   - `ModelTokenStats` 加 `system_hash_distribution: HashMap<[u8;32], u64>`
5. `crates/agent/src/infra/api/handlers/llm_config.rs` (扩)
   - `/api/v1/metrics` handler 改签名 `Query<MetricsQuery>` (新增 `system_hash: Option<[u8;32]>` query)
6. `crates/agent/Cargo.toml` (扩)
   - 加 `sha2 = "0.10"` (与 `crates/server/Cargo.toml:47` 对齐)
7. `crates/agent/src/config.rs` (扩)
   - `LlmConfig` 加 2 子结构:
     ```rust
     pub cache_diagnostics: CacheDiagnosticsConfig,
     pub prompt: PromptConfig,
     pub struct CacheDiagnosticsConfig {
         pub enabled: bool,                  // 默认 true
         pub system_hash_dimension: bool,    // 默认 true
     }
     pub struct PromptConfig {
         pub strip_reasoning_content: bool,  // 默认 true (D8)
         pub canonicalize_schemas: bool,     // 默认 true (D9)
     }
     ```

**Hot-reload**: agent 重启 (走 env var 启动覆盖)。WebSocket `websocket.rs:745-822` 仅 5 个 config_type 分支 (skills/actions/game_rules/world_building_rules/prompt_templates), **无 `llm` 分支**。

**验证** (24h 数据)：
- 平均 cache hit rate
- system_hash 变更频率
- hit rate 与 system_hash 稳定性的相关性
- Per-section token 占比

### Phase 1: D8 reasoning_content 剥离 (1 周, 3 文件)

**目标**: 消除每轮 ~500 tok 的 reasoning 回传。

**关键发现 (v2.1 漏)**: reasoning_content 序列化发生在**两个**位置, 需同步修改:

| 位置 | 文件:行 | 调用方 |
|------|---------|--------|
| **位置 A**: helper 函数 | `client.rs:73-76` (在 `build_conversation_messages` 内) | `direct_client.rs:969` `complete_conversation_streaming` + `:1087` `complete_conversation` |
| **位置 B**: inline code | `direct_client.rs:1304-1307` (在 `complete_with_conversation_and_tools` 内) | `direct_client.rs:1272` `complete_with_conversation_and_tools` (主路径) |

`engine.rs:995` (主决策调用) → `client.rs:926` `complete_with_conversation_and_tools` → `direct_client.rs:1283-1308` inline code (位置 B)。**`use_tool_calling=true` 时主路径走位置 B, 不经位置 A 的 helper**。

**改造** (3 文件)：

1. `crates/agent/src/component/llm/client.rs` (扩)
   - `build_conversation_messages` 加 `strip_reasoning: bool` 参数 (第 6 个参数)
   - 当 `strip_reasoning=true`, 调用 `ChatMessage::assistant_with_reasoning(&turn.assistant, None)` (line 73-76 处)
   - **注**: 不存在独立 `ChatMessage::assistant(content)` 构造器 (v2.1 误引), 复用 `assistant_with_reasoning` 传 `None` 即可
2. `crates/agent/src/component/llm/direct_client.rs` (扩)
   - 位置 A 调用点 (line 969, 1087): 透传 `strip_reasoning` 到 `build_conversation_messages`
   - **位置 B inline code (line 1304-1307)**: 同样改 `assistant_with_reasoning(..., if strip_reasoning { None } else { turn.reasoning_content.clone() })`, inline 块加 `strip_reasoning: bool` 参数
   - `complete_with_conversation_and_tools` 函数签名 (line 1272) 加 `strip_reasoning: bool` 参数
   - 读 `prompt_config.strip_reasoning_content`
3. `crates/agent/src/component/llm/direct_client.rs` (扩, 透传到 engine.rs 调用)
   - `engine.rs:995` `complete_json_with_conversation_and_tools` 调用处加 `strip_reasoning` 参数

**配置键**: `prompt.strip_reasoning_content: true`

**5% 灰度机制 (v2.1 漏)**: **env var 部署时区分**
- 默认: 全部 agent 用 `prompt.strip_reasoning_content: false`
- 5% 灰度: 5% agent 部署时设 `CYBER_JIANGHU_PROMPT_STRIP_REASONING_CONTENT=true` 覆盖
- 20%: 20% agent 设
- 100%: 全量设
- spec 不引入运行时随机采样; rollout 是部署时决策, 不是代码内 bucketing

**风险与缓解**:
- LLM 失去 reasoning 影响决策 → **env var 分批部署 5% → 20% → 100%** + 24h 决策质量对比
- 回滚: 重新部署 agent 时不设 env var (默认 false)

**预期**: 33% → 55-65% (按 Reasonix ~500 tok/turn 折算)

### Phase 2: D9 schema canonicalization (1 周, 1 新文件 + 2 现有)

**目标**: 让 `tools` 字段在 API 顶层字节级稳定。

**改造** (1 新 + 2 改)：

1. `crates/agent/src/component/llm/canonicalize.rs` (新, ≤100 行)
   - `fn canonicalize_json_schema(value: &mut serde_json::Value)`: sort object keys, sort `required` array, 标准化 `additionalProperties: false`, 移除 `default` 以外的元数据噪声
2. `crates/agent/src/component/llm/tool_types.rs` (扩)
   - `ToolDefinition::canonical_json() -> String` 新方法
3. `crates/agent/src/component/llm/direct_client.rs` (扩)
   - 序列化 `tools` 字段前调 `tool.canonical_json()`
   - 读 `prompt_config.canonicalize_schemas` 开关

**灰度机制**: 同 D8, env var 部署区分 (`CYBER_JIANGHU_PROMPT_CANONICALIZE_SCHEMAS`)

**风险**: DeepSeek 缓存是否覆盖 `tools` 字段未在官方文档明确。**5% 灰度实测验证 hit rate 增量 > 0 才保留**; 否则回退。

**预期**: D8 基础上再 +5-10pp。

### Phase 3: 数据驱动压缩 (TBD, 由 Phase 0 数据决定)

不预设方案。

---

## 4. 文件级改造清单 (15 改动, 1 新文件)

```
Phase 0 (7 改动):
1.  crates/agent/src/soul/actor/engine_prompts.rs    (扩: compute_system_hash 动态跟随 use_tool)
2.  crates/agent/src/soul/actor/engine.rs            (扩: last_system_hash 字段)
3.  crates/agent/src/component/llm/direct_client.rs  (扩: record_token_usage 加 system_hash)
4.  crates/agent/src/component/llm/token_tracking.rs (扩: system_hash_distribution 字段)
5.  crates/agent/src/infra/api/handlers/llm_config.rs (扩: /api/v1/metrics 改 Query<MetricsQuery>)
6.  crates/agent/Cargo.toml                          (扩: sha2 = "0.10")
7.  crates/agent/src/config.rs                       (扩: LlmConfig 加 cache_diagnostics + prompt 子结构)

D8 (3 改动, **双路径同步**):
8.  crates/agent/src/component/llm/client.rs         (扩: build_conversation_messages 加 strip_reasoning)
9.  crates/agent/src/component/llm/direct_client.rs  (扩: 位置 A + 位置 B 两处都改)
10. crates/agent/src/component/llm/direct_client.rs  (扩: complete_with_conversation_and_tools 加 strip_reasoning 参数)

D9 (3 改动 + 1 新):
11. crates/agent/src/component/llm/canonicalize.rs   (新, ≤100 行)
12. crates/agent/src/component/llm/tool_types.rs     (扩: canonical_json())
13. crates/agent/src/component/llm/direct_client.rs  (扩: 序列化前调 canonicalize)
14. crates/agent/src/component/llm/mod.rs            (扩: 导出 canonicalize)

Phase 3 (TBD): 由 Phase 0 数据决定
```

**对比 v2.1**: 14 改动 → 15 改动 (D8 显式分 2 个改动点)。其他一致。

---

## 5. Magic Value 审计 (零硬编码)

| 配置键 | 默认 | 位置 | env var 覆盖 |
|--------|------|------|--------------|
| `cache_diagnostics.enabled` | true | `LlmConfig::cache_diagnostics` | `CYBER_JIANGHU_CACHE_DIAGNOSTICS_ENABLED` |
| `cache_diagnostics.system_hash_dimension` | true | 同上 | `CYBER_JIANGHU_CACHE_DIAGNOSTICS_SYSTEM_HASH_DIMENSION` |
| `prompt.strip_reasoning_content` | true | `LlmConfig::prompt` | `CYBER_JIANGHU_PROMPT_STRIP_REASONING_CONTENT` (D8 5% 灰度用) |
| `prompt.canonicalize_schemas` | true | 同上 | `CYBER_JIANGHU_PROMPT_CANONICALIZE_SCHEMAS` (D9 5% 灰度用) |

**所有运行时可调项** + **5% 灰度覆盖**全部走 env var。无代码内魔法值。

---

## 6. 验证指标 (数据驱动)

| 阶段 | 指标 | 数据来源 | 目标设定 |
|------|------|---------|---------|
| Phase 0 (24h) | 聚合 cache_hit_rate | `token_tracking` | 测量 baseline |
| Phase 0 (24h) | system_hash 变更频率 | 同上 | 测量 |
| Phase 0 (24h) | per-section token 占比 | `PromptSectionEstimate` | 测量 |
| D8 (24h 5% env) | cache_hit_rate 增量 | `token_tracking` | ≥+15pp |
| D8 (24h 5% env) | 决策质量波动 | death/success 率 | ≤ ±2% |
| D9 (24h 5% env) | cache_hit_rate 增量 | `token_tracking` | ≥+5pp |
| Phase 3 | 按数据 | 按方案 | 按方案 |

---

## 7. 风险与缓解

| 风险 | 缓解 |
|------|------|
| D8 剥离 reasoning 影响决策 | env var 5% → 20% → 100% 分批部署 + 24h 对照, 回滚 = 不设 env var |
| D9 canonicalize 改变 schema | 单元测试验证语义等价 |
| D9 假设 `tools` 字段在 DeepSeek 缓存前缀中未明 | 5% 灰度实测 hit rate 增量; 若 +0pp 立即回退 |
| Phase 0 数据噪声 | 24h 滚动平均, 至少 48h 出结论 |
| system_hash 高频变更 (persona 动态) | Phase 0 数据出来后, Phase 3 决策是否拆 persona 段 |
| LLM cost 反增 | **v2.2 无任何 LLM 新调用** |
| 工程成本超 3 周 | 每阶段可独立停止 |
| `engine.rs` 1645 行超 800 行限制 | 不在本 spec 修复, 单独开 issue |

**日志约定**: `tracing::info!` / `warn!` / `debug!` (见 `tool_loop.rs:65`, `token_tracking.rs:351`)

**YAML 热更新**: 4 键通过 env var 启动覆盖, **不走 WebSocket** (无 `llm` 分支)

---

## 8. 实施时间表

```
Day 1-3:   Phase 0 测量 (7 改动, 1 新 Cargo 依赖)
Day 4-5:   Phase 0 24h 数据采集
Day 6:     Phase 0 数据 review
Day 7-10:  D8 reasoning_content 剥离 (3 改动, 双路径同步)
Day 11:    D8 5% env 灰度, 24h 观察
Day 12-13: D8 20% → 100% (若指标正常)
Day 14-18: D9 schema canonicalization (1 新 + 3 改)
Day 19:    D9 5% env 灰度, 24h 观察
Day 20-21: D9 20% → 100% (若指标正常)
Day 22+:   Phase 3 (TBD)
```

总计: 3 周。

---

## 9. 测试规划

| 新增/改动 | 测试文件 | 覆盖 |
|----------|---------|------|
| `engine_prompts.rs::compute_system_hash` | `engine_prompts_test.rs` 追加 | 动态跟随 use_tool; 相同输入 → 相同 hash; 字段变 → hash 变 |
| `client.rs::build_conversation_messages(strip_reasoning=true)` | `client_test.rs` 追加 | reasoning_content 不出现; `strip_reasoning=false` 时保留 |
| `direct_client.rs` 位置 A (`complete_conversation`) | `direct_client_test.rs` 追加 | mock 验证 reasoning 不在 API 请求中 |
| `direct_client.rs` 位置 B (`complete_with_conversation_and_tools`) | `direct_client_test.rs` 追加 | **主路径** mock 验证 reasoning 不在 API 请求中 |
| `canonicalize.rs` (新) | `canonicalize_test.rs` (新) | sort required; 稳定 key 顺序; 移除 default; 语义不变 |
| `tool_types.rs::canonical_json` | `tool_types_test.rs` 追加 | 多次调用 → 相同输出 |
| `direct_client.rs` tools canonicalize 集成 | `direct_client_test.rs` 追加 | 第二次调用 tools 字段 byte-identical |
| `token_tracking.rs` system_hash_distribution | `token_tracking_test.rs` 追加 | 按 system_hash 聚合计数 |
| `llm_config.rs` ?system_hash= query | `llm_config_test.rs` 追加 | filter 工作 |
| `config.rs::LlmConfig` 新字段 | `config_test.rs` 追加 | 默认值正确; env var 覆盖正确 |
| **集成测试** (新) | `tests/prefix_cache_e2e_test.rs` (新) | 100 tick 模拟; **双路径** (helper + inline) 都验证 reasoning 不出现 |

**新增测试文件**: 2
**追加测试**: 9 个现有文件

---

## 10. 与既有 spec 关系

- `2026-05-14-token-optimization-design.md`: 互补, 不冲突
- v1 (e25903f): superseded
- v2 (1c1c73d): superseded
- v2.1 (1a95fd2): superseded by v2.2

---

## 11. 引用

- DeepSeek KV cache: https://api-docs.deepseek.com/guides/kv_cache
- Reasonix: https://github.com/esengine/deepseek-reasonix
- 关键文件: `internal/agent/session.go:17-24`, `internal/provider/openai.go:150-168`, `internal/agent/agent.go:80-93, 334-335`, `internal/agent/compact.go:24-35, 113-123`

## 12. v1/v2/v2.1 → v2.2 修正清单

| # | v2.1 表述 | v2.2 修正 | 证据 |
|---|---------|-----------|------|
| **CRIT 1** | D8 只改 `client.rs::build_conversation_messages` helper | **双路径同步**: helper (位置 A, line 73-76) + inline code (位置 B, `direct_client.rs:1304-1307`) 都改 | `engine.rs:995` 主路径走 inline, helper 路径不被触发; 不改 inline 则 D8 在 tool-calling 模式完全失效 |
| **CRIT 2** | 调用 `ChatMessage::assistant(&turn.assistant)` | 改用 `ChatMessage::assistant_with_reasoning(&turn.assistant, None)` | `openai_types.rs:57-101` 只有 4 个构造器, 无 `assistant(content)` |
| **CRIT 3** | 5% 阶梯机制未明 | **env var 部署时区分** (`CYBER_JIANGHU_PROMPT_STRIP_REASONING_CONTENT` / `..._CANONICALIZE_SCHEMAS`); 5% agent 部署时设 true | LlmConfig 全局, 无运行时随机采样 |
| MIN 1 | `compute_system_hash` 写死 `use_tool_calling=true` | 动态 `self.llm_client.supports_tool_calling()` | `engine.rs:679` `use_tool_calling` 依 LLM 客户端变 |
| MIN 2 | `crates/server/Cargo.toml:18` (sha2) | `crates/server/Cargo.toml:47` (实际行) | grep 验证 |
| MIN 3 | spec §0(b) 行号 `direct_client.rs:1301-1308` 经 helper | 拆为: `direct_client.rs:1304-1307` (inline) + `client.rs:73-76` (helper) + `direct_client.rs:969, 1087` (helper 调用点) | `direct_client.rs:1283` 注释 "不使用 build_conversation_messages" |

## 13. 待确认事项

1. Phase 0 → D8 取决于数据。若 system_hash 变更频率 > 1/agent/h, 需先解耦 persona 段
2. D8 放量决策: 5% env 24h → 20% env 24h → 100% env, 依据 `cache_hit_rate` 增量 ≥+15pp 且决策质量 ≤±2%
3. D9 放量决策: 同 D8, 增量 ≥+5pp (DeepSeek 是否覆盖 tools 未明, 下限更保守)
4. D9 失败回退: 若 hit rate 增量 ≤0, 立即不设 `CYBER_JIANGHU_PROMPT_CANONICALIZE_SCHEMAS` env var, 重新评估
5. Phase 3 不在本 spec 范围, 需另起 spec 评审
