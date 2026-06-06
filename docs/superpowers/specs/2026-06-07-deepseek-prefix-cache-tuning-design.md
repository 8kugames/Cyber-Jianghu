# DeepSeek 前缀缓存调优 v2：数据驱动的最小可行改造

**日期**: 2026-06-07
**状态**: Draft (v2, 替代 v1)
**前置**: v1 (commit e25903f) 经 3-agent 表决 0/3 通过, 共识问题：Reasonix 原理误读、`system_immutable` 名不副实、D5/D6 为不存在问题设计、Phase 0 逻辑不自洽、ROI 12+ 年、配置驱动不足。v2 重新组织。
**问题**: DeepSeek 缓存命中率仅 ~33%, 长会话 token 成本高
**目标**: 基于真实 telemetry 定位最大破坏点, 用最小改造集推动命中率提升

---

## 0. 用户原问题（v1 漏答, v2 必答）

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
| Session 单例 | **适用** | 项目每 agent 一生命周期, system 段本应只构建一次。当前 `engine_prompts.rs::build_system_message` 每次 tick 调用 — 重复构建是浪费。 |
| 工具 schema 稳定 | **适用** | 关键改造点 — JSON schema 经 `serde_json::to_string` 序列化, key 顺序取决于数据结构。canonicalize 即可。 |
| `reasoning_content` 剥离 | **适用** | **最大单杠杆**。当前 `conversation.rs:27,73,84,86` + `engine.rs:947` 把 `reasoning_content` 序列化进每轮 user/assistant turn。 |
| 压缩触发 0.8 阈值 | **不适用** | 项目是**单 tick 决策** (4-8K prompt), 不是 Reasonix 那种累积 turn 对话 (0-128K 累积)。0.8 阈值 = 永远不触发。**Compactor 对本项目无意义。** |
| 两模型独立 session | **不适用 (成本为负)** | Reasonix planner 是**可选 + 低频**。本项目 ReflectorSoul 每次决策都跑 (per CLAUDE.md), 独立 session = **每次决策 +1 LLM 调用, prefix 命中率不升反降**。 |
| 聚合命中率指标 | **部分适用** | 已有 `cache_hit_tokens` 聚合 (`token_tracking.rs:53,96,144-161`), 缺的是 `system_hash` 维度下钻。 |

**结论**: 采用 Session 单例精神 + 工具 schema canonicalize + `reasoning_content` 剥离。**不采用**压缩 (per-tick 不需要) + ReflectorSoul 独立 (成本为负)。

### (c) 实现方案

见 §3-§4。

---

## 1. 现状基线 (基于已有 telemetry, 不假设)

| 维度 | 现状 | 证据 |
|------|------|------|
| `cache_hit_tokens` 聚合 | **已收集** | `crates/agent/src/component/llm/token_tracking.rs:40,53,96,144-161`；`infra/api/handlers/llm_config.rs:597-639` 通过 `/api/v1/config/llm/usage` 暴露 |
| `system_hash` 维度 | **缺失** | 无任何代码计算 system 段 hash |
| Per-section token 计数 | **已有** | `engine_prompts.rs:293-311` 的 `PromptSectionEstimate` (含 system / semi_static / volatile / world_state / tool_results 段) |
| 命中率 33% 数据源 | **DeepSeek 响应 usage 字段** | `direct_client.rs:633-642` 提取 `prompt_cache_hit_tokens` + `prompt_cache_miss_tokens` |

**v1 错判**: v1 spec §1.1 写"缺 per-prompt 命中诊断" — 实际是缺 `system_hash` 维度下钻, 聚合已有。

---

## 2. 设计决策

| 维度 | 决策 | 否决的备选 |
|------|------|-----------|
| 总体策略 | **测量先行, 数据驱动压缩** | v1 的 "Reasonix 完整范式重构" |
| 顺序 | Phase 0 测量 → D8 剥离 → D9 规范化 → Phase 3 按数据决策 | v1 的"5 阶段 17 文件一气呵成" |
| Session 模型 | **不新建抽象**, 在 `engine_prompts.rs` 加 memoization (HashMap keyed by persona+rules) | v1 D1 新建 `core/session.rs` |
| Tools 位置 | **不文本化**, 留在 API 顶层 `tools` 字段, 改做 schema canonicalize | v1 D2 文本化进 system |
| 工具结果 | **留在主 session**, 不隔离 | v1 D4 ToolSession 副 messages |
| ReflectorSoul session | **不拆分**, 保持现状 | v1 D5 独立 session |
| 上下文压缩 | **不实现**, per-tick 模式不适用 | v1 D6 Compactor |
| Reasoning 处理 | **D8 剥离** (新增, 最大单杠杆) | — |
| Schema 规范化 | **D9 canonicalize** (新增, 第二杠杆) | — |
| 观测 | **扩 `token_tracking.rs`** 加 system_hash 维度, 不新建 DB | v1 D7 新建 SQLite 表 |
| 灰度 | D8/D9 各自独立开关 (`prompt.strip_reasoning_content`, `prompt.canonicalize_schemas`), 5% 阶梯放量 | v1 的多维灰度 |
| 质量风险 | ±2% 波动容忍, D8/D9 各跑 24h 5% agent 对照 | — |

---

## 3. 实施阶段

### Phase 0: 测量先行 (2-3 天, 无新模块)

**目标**: 拿到真实 baseline + 知道 cache miss 集中在哪一段。

**改造** (5 文件, 全部已存在)：

1. `crates/agent/src/soul/actor/prompt_cache.rs`
   - 新增方法 `system_hash() -> [u8; 32]`, 计算 `persona + rules + actions + skills` 段的 SHA256
   - **不改 system 段内容** (persona 动态字段先不动, 让 baseline 反映真实波动)
2. `crates/agent/src/soul/actor/engine.rs`
   - LLM 调用前取 `system_hash`, 塞进 `LlmCallContext`
3. `crates/agent/src/component/llm/direct_client.rs`
   - 把 `system_hash` 透传到 token_tracking
4. `crates/agent/src/component/llm/token_tracking.rs`
   - 已有 `cache_hit_tokens`, 加 `system_hash: [u8; 32]` 字段
   - 持久化追加 (现 JSON 文件, 不用新建 SQLite)
5. `crates/agent/src/infra/api/handlers/llm_config.rs`
   - 扩展现有 `/api/v1/config/llm/usage` 加 `?system_hash=` query

**配置键** (`crates/server/config/llm.yaml`)：
```yaml
cache_diagnostics:
  enabled: true
  system_hash_dimension: true  # Phase 0 默认 true, 后续可关
```

**验证** (24h 数据)：
- 平均 cache hit rate (按 model/hour 聚合)
- `system_hash` 变更频率 (每 agent 每天变几次)
- hit rate 与 system_hash 稳定性的相关性
- Per-section token 占比 (用 `PromptSectionEstimate`)

**不达标则不进入 Phase 1+**, 重新分析。

### Phase 1: D8 reasoning_content 剥离 (1 周, 1-2 文件)

**目标**: 消除每轮 ~500 tok 的 reasoning 回传。

**杠杆来源**: Reasonix `openai.go:158-161` (经验值: 每次节省 ~500 tok)。

**改造** (2 文件)：

1. `crates/agent/src/component/llm/conversation.rs`
   - `ConversationTurn` 保留 `reasoning_content` 字段 (用于 session 存档 / UI 显示)
   - `serialize_for_api()` 方法新增, **剥离 `reasoning_content`** (只发 `role + content + tool_calls`)
2. `crates/agent/src/component/llm/direct_client.rs`
   - 用 `serialize_for_api()` 替代现有 message 序列化
   - 在 `record_token_usage` 加 log: `(stripped_tokens: u64, original_tokens: u64)`

**配置键**：
```yaml
prompt:
  strip_reasoning_content: true  # 默认 true
```

**风险与缓解**：
- LLM 失去上一轮 reasoning 影响后续决策 → **5% 阶梯灰度 (5% → 20% → 100%)** + 24h 决策质量对比 (死亡率/成功率)
- 回滚开关: `prompt.strip_reasoning_content: false`

**预期**: 33% → 55-65% (按 Reasonix 实测的 ~500 tok/turn 节省折算)。

### Phase 2: D9 schema canonicalization (1 周, 1 新文件 + 1-2 现有)

**目标**: 让 `tools` 字段在 API 顶层实现字节级稳定, 触发 DeepSeek cache。

**杠杆来源**: Reasonix `internal/provider/schema_canonicalize.go` (推断位置, 需实施时验证)。

**改造** (1 新 + 2 改)：

1. `crates/agent/src/component/llm/canonicalize.rs` **(新, ≤100 行)**
   - `fn canonicalize_json_schema(value: &mut serde_json::Value)`: 递归 sort object keys, sort `required` array, 标准化 `additionalProperties: false`, 移除 `default` 字段以外的元数据噪声
   - **无配置**: 算法固定, 不暴露参数
2. `crates/agent/src/component/llm/tool_types.rs`
   - `ToolDefinition::canonical_json() -> String` 新方法, 内部调 `canonicalize`
3. `crates/agent/src/component/llm/direct_client.rs`
   - 在序列化 `tools` 字段前调 `canonical_json()`, 不变 schema 数据结构
4. `crates/agent/src/component/llm/mod.rs`
   - 导出新模块

**配置键**：
```yaml
prompt:
  canonicalize_schemas: true  # 默认 true
```

**风险与缓解**：
- LLM 看到 canonicalize 后的 schema 跟原 schema 不完全一样 → **实际不会** (canonicalize 是无损变换, 语义等价)
- 序列化耗时 → benchmark, 应 <1ms/tool

**预期**: 在 D8 基础上再 +10-15pp (具体数值 Phase 0 数据校准)。

### Phase 3: 数据驱动压缩 (1-2 周, 视 Phase 0 数据决定)

**不预设方案**。Phase 0 数据出来后, 按实际破坏点针对性修。候选：
- `events_log` 治理 (如果数据显示 prompt 中 events 占大头)
- `world_state_section` 字段裁剪 (如果显示 WorldState 全量是大头)
- Session message memoization (如果显示 build_system_message 重复构建是瓶颈)

**所有候选方案都必须有 Phase 0 数据支撑, 否则不做。**

---

## 4. 文件级改造清单 (小, 全部基于现有模块)

```
Phase 0 (5 文件, 全部已存在):
1.  crates/agent/src/soul/actor/prompt_cache.rs        (扩: 加 system_hash() 方法)
2.  crates/agent/src/soul/actor/engine.rs              (扩: 透传 system_hash)
3.  crates/agent/src/component/llm/direct_client.rs    (扩: 接收 system_hash)
4.  crates/agent/src/component/llm/token_tracking.rs   (扩: 加 system_hash 字段)
5.  crates/agent/src/infra/api/handlers/llm_config.rs (扩: 加 ?system_hash= query)
6.  crates/server/config/llm.yaml                      (扩: cache_diagnostics section)

D8 (2 文件):
7.  crates/agent/src/component/llm/conversation.rs     (扩: serialize_for_api 剥离 reasoning)
8.  crates/agent/src/component/llm/direct_client.rs    (扩: 用 serialize_for_api)
9.  crates/server/config/llm.yaml                      (扩: prompt.strip_reasoning_content)

D9 (1 新 + 3 改):
10. crates/agent/src/component/llm/canonicalize.rs     (新, ≤100 行)
11. crates/agent/src/component/llm/tool_types.rs       (扩: canonical_json())
12. crates/agent/src/component/llm/direct_client.rs    (扩: 序列化前调 canonicalize)
13. crates/agent/src/component/llm/mod.rs              (扩: 导出 canonicalize)
14. crates/server/config/llm.yaml                      (扩: prompt.canonicalize_schemas)

Phase 3 (TBD): 由 Phase 0 数据决定
```

**对比 v1**: 17 文件 / 5 新模块 / 3 新表 → **14 文件 / 1 新文件 / 0 新表**。

---

## 5. Magic Value 审计 (零硬编码)

| 配置键 | 默认 | 位置 |
|--------|------|------|
| `cache_diagnostics.enabled` | `true` | `llm.yaml` |
| `cache_diagnostics.system_hash_dimension` | `true` | `llm.yaml` |
| `prompt.strip_reasoning_content` | `true` | `llm.yaml` |
| `prompt.canonicalize_schemas` | `true` | `llm.yaml` |
| 灰度阶梯 5%/20%/100% | **无配置, 写死在灰度 Playbook** | spec §7 |
| 决策质量波动阈值 ±2% | **无配置, 写死在 spec §6 验证** | spec §6 |
| Phase 0 24h 数据采集窗口 | **24h, 写死在 Phase 0 流程** | spec §3 |

**无代码内魔法值**。所有运行时可调项都进 `llm.yaml`。灰度/质量阈值属"决策框架", 不需要运行时调, 写在 spec。

---

## 6. 验证指标 (数据驱动, 不预设目标)

| 阶段 | 指标 | 数据来源 | 目标设定方式 |
|------|------|---------|------------|
| Phase 0 (24h) | 聚合 cache_hit_rate | `token_tracking` | 测量 baseline, 不设目标 |
| Phase 0 (24h) | system_hash 变更频率 | 同上 | 测量, 若 > 1/agent/h 说明 persona 段需要解耦 |
| Phase 0 (24h) | per-section token 占比 | `PromptSectionEstimate` | 测量, 定位最大段 |
| D8 (24h 5% 灰度) | cache_hit_rate 增量 | `token_tracking` | **基于 Phase 0 baseline, 设定 ≥+15pp** |
| D8 (24h 5% 灰度) | 决策质量波动 | death/success 率 | ≤ ±2% |
| D9 (24h 5% 灰度) | cache_hit_rate 增量 | `token_tracking` | **基于 D8 baseline, 设定 ≥+10pp** |
| Phase 3 (TBD) | 按数据 | 按方案 | 按方案 |

**关键反转**: 不在 spec 写 "80%" 这种不可证伪目标。每个阶段目标基于前阶段数据, 写进实施 PR/issue。

---

## 7. 风险与缓解

| 风险 | 缓解 |
|------|------|
| D8 剥离 reasoning 导致 LLM 决策质量下降 | 5% 阶梯灰度 + 24h 对照, 回滚开关 `prompt.strip_reasoning_content: false` |
| D9 canonicalize 改变 schema 实际效果 | 单元测试验证 canonicalize 前/后语义等价 (与 `serde_json::from_str` 输出相同) |
| Phase 0 数据噪声大, 看不出趋势 | 24h 滚动平均, 至少跑 48h 才出结论 |
| system_hash 高频变更 (persona 动态) | Phase 0 数据出来后, Phase 3 决策是否要把 persona 段从 system 拆到 volatile |
| LLM 调用 cost 反增 (新 LLM 调用, 如摘要) | **v2 无任何 LLM 新调用**, v1 ROI 12+ 年问题不存在 |
| 工程成本超 2.5-3 周 (v1 估算) | v2 拆为 2-3 天 + 1 周 + 1 周 + TBD, 每阶段可独立停止 |
| 现有 `engine.rs` 1646 行超 CLAUDE.md 800 行限制 | **不在本 spec 修复** (避免 scope creep, 单独开 issue) |

**YAML 热更新**: 4 个新配置键 (`cache_diagnostics.enabled` / `system_hash_dimension` / `prompt.strip_reasoning_content` / `prompt.canonicalize_schemas`) 全部走 `crates/server/config/llm.yaml`, 通过现有 `ConfigUpdate` WebSocket 消息推送 (机制见 `crates/agent/src/infra/transport/websocket.rs:99-102, 727-749`)。无需新加 WS 分发逻辑, 现有 deserializer 已是 LLM config 通配。

**日志约定**: 沿用项目惯例 `tracing::info!` / `tracing::warn!` / `tracing::debug!` (见 `tool_loop.rs:65`、`token_tracking.rs:351`)。Phase 0 的 system_hash 写入用 `debug!` (高频), D8/D9 灰度决策点用 `info!`, 告警用 `warn!` / `error!`。

---

## 8. 实施时间表 (增量, 可独立停止)

```
Day 1-3:   Phase 0 测量 (5 文件改动, 无新模块)
Day 4-5:   Phase 0 24h 数据采集 (后台跑, 不阻塞)
Day 6:     Phase 0 数据 review, 决定是否进 D8
Day 7-10:  D8 reasoning_content 剥离 (2 文件)
Day 11:    D8 5% 灰度放量, 24h 观察
Day 12-13: D8 20% → 100% (若指标正常)
Day 14-18: D9 schema canonicalization (1 新 + 3 改)
Day 19:    D9 5% 灰度放量, 24h 观察
Day 20-21: D9 20% → 100% (若指标正常)
Day 22+:   Phase 3 (TBD, 由数据驱动)
```

总计: 3 周 (含 2 段 24h 观察)。

**v1 估算**: 2.5-3 周, 17 文件, 5 新模块。**v2 估算**: 3 周, 14 文件 (其中 1 新), 0 新表, 0 新 LLM 调用。

工程量基本相当, 但 v2 风险面**显著低于 v1**:
- 不动 Session 抽象 (低架构风险)
- 不动 ReflectorSoul 流程 (低决策质量风险)
- 不动 Compactor (避免无意义 LLM 摘要调用)
- 0 新 DB schema (低迁移风险)
- 每阶段可独立停止 (低沉没成本风险)

---

## 9. 测试规划 (spec 内显式声明, 避免 v1 漏)

| 新增/改动 | 测试文件 | 覆盖 |
|----------|---------|------|
| `prompt_cache.rs::system_hash()` | `prompt_cache_test.rs` 追加 (现有) | 相同输入 → 相同 hash; persona 变更 → hash 变更 |
| `conversation.rs::serialize_for_api()` | `conversation_test.rs` 追加 (现有) | 验证 `reasoning_content` 被剥离, 其他字段保留 |
| `canonicalize.rs` (新) | `canonicalize_test.rs` (新) | 验证：sort `required`; 稳定 key 顺序; 移除 `default`; 语义不变 (与 `serde_json::from_str` 结果相同) |
| `tool_types.rs::canonical_json()` | `tool_types_test.rs` 追加 (现有) | 多次调用 → 相同输出 (字节级) |
| `direct_client.rs` 集成 | `direct_client_test.rs` 追加 (现有) | mock 验证：第二次调用 tools 字段与第一次 byte-identical |
| `token_tracking.rs` system_hash 维度 | `token_tracking_test.rs` 追加 (现有) | 验证 system_hash 字段正确写入聚合 |
| `llm_config.rs` 新 query | `llm_config_test.rs` 追加 (现有) | 验证 `?system_hash=` filter 工作 |
| **集成测试** (新) | `tests/prefix_cache_e2e_test.rs` (新) | 模拟 100 tick, 验证 cache_hit_rate 随 system_hash 稳定而提升; D8 验证 reasoning 不出现在 API 请求中 |

**新增测试文件**: 2 (`canonicalize_test.rs`, `tests/prefix_cache_e2e_test.rs`)
**追加测试**: 6 个现有文件

---

## 10. 与既有 spec 关系

- `docs/superpowers/specs/2026-05-14-token-optimization-design.md`: Token 优化 (注意力门控 + Tool-First)。**互补, 不冲突**。本 spec 关注 cache 命中, 旧 spec 关注 prompt 体积。两者可同时进行。
- v1 spec (commit e25903f): **superseded by v2**。在 git 中保留作为失败案例参考。

---

## 11. 引用

- DeepSeek 官方文档: https://api-docs.deepseek.com/guides/kv_cache
- Reasonix 源码: https://github.com/esengine/deepseek-reasonix
- Reasonix 关键文件 (v2 比 v1 更精确引用):
  - `internal/agent/session.go:17-24` (Session 单例化)
  - `internal/agent/agent.go:80-93, 334-335` (聚合命中率)
  - `internal/agent/compact.go:24-35, 113-123` (压缩策略, **本项目不采用**)
  - `internal/provider/openai.go:150-168` (reasoning_content 剥离, **最大单杠杆**)
  - `internal/provider/schema_canonicalize.go` (推断, **D9 改造目标**)
  - `internal/agent/cachehit_e2e_test.go:108-148` (前缀稳定性验证测试模式)

## 12. 待确认事项

1. **Phase 0 完成后是否进 D8, 取决于数据**。若 Phase 0 显示 system_hash 变更频率 > 1/agent/h, 需先解耦 persona 段, 再进 D8。
2. **D8 灰度放量决策** = 5% 24h → 20% 24h → 100%, 决策依据为 `cache_hit_rate` 增量 ≥ +15pp 且决策质量 ≤ ±2% 波动。
3. **D9 灰度放量决策** 同 D8, 增量 ≥ +10pp。
4. **Phase 3 不在本 spec 范围**, 需另起 spec 评审。
