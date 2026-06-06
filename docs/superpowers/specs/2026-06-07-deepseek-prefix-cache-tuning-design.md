# DeepSeek 前缀缓存调优设计：Reasonix-范式重构

**日期**: 2026-06-07
**状态**: Draft
**问题**: DeepSeek 缓存命中率仅 ~33%, 长会话 token 成本高
**目标**: 命中率 33% → 80%+, LLM 决策质量波动 ≤ 2%

## 1. 根因分析

DeepSeek 文档显示, system 段稳定 + ≥1024 tok 时单 prefix 命中率应 >80%。当前 33% 提示
**`system_size / (system + user + tool_results) ≈ 1/3`**, 即 system 段实际占比就是
三分之一。问题不在 system 段本身不稳定, 而在分母被以下因素放大:

| # | 破坏点 | 文件 | 影响机制 |
|---|--------|------|---------|
| 1 | `tools` schema 在 API 顶层字段, 不在 message prefix | `crates/agent/src/component/llm/openai_types.rs:11-35` | DeepSeek 缓存只覆盖 `messages`; 每次重新处理 500~2000 tok 的 tool schema |
| 2 | `events_log` 单调增长 | `crates/agent/src/soul/actor/engine_prompts.rs:411-416` | user 消息体随 tick 累积, system 占比越来越小 |
| 3 | EarthSoul 多轮 tool calling 累积 | `crates/agent/src/soul/earth/tool_loop.rs:33-261` | 第 2 轮起 prefix 失效 (多出 assistant + tool_result) |
| 4 | 缺 per-prompt 命中诊断 | `crates/agent/src/component/llm/direct_client.rs:643-648` | 只能看聚合, 无法定位"哪个 prompt 段"失配 |
| 5 | action/skill 索引只有名称, 浪费可缓存空间 | `crates/agent/src/soul/actor/engine_prompts.rs:181-206` | 稳定 prefix 体积偏小, 缓存单元 (64 tok) 利用率低 |

### Reasonix 范式的核心不变量 (参考)

> System prompt + tools schema 必须**字节级稳定**贯穿整个 session。任何 mutation =
> 缓存失效, 命中率为 0。
> 来源: https://github.com/esengine/deepseek-reasonix , internal/agent/session.go,
> internal/provider/openai.go

Reasonix 的关键设计:
- `Session.Messages` 系统段 (index 0) 永不动
- 工具 schema 在 `tools` 字段但不变, 且 tools 不进 message prefix
- `reasoning_content` 不回传 (每次省 ~500 tok)
- 上下文压缩是**唯一**的 cache reset 点, 设计为罕见事件
- 聚合命中率 (`Σhit / Σ(hit+miss)`) 是稳定指标

## 2. 设计决策

| 维度 | 决策 |
|------|------|
| 总体方案 | Reasonix 完整范式重构 (方案 C) |
| Session 模型 | 新建 `crates/agent/src/core/session.rs`, system 段永不变 |
| tools schema 位置 | 文本化进入 system 段, 走 prefix 缓存; API `tools` 字段仅留极简版 |
| events_log 治理 | 弹出 WorldState, 写 episodic memory; prompt 只显示最近 5 条概要 |
| tool calling | 中间轮次隔离到 `tool_session`, 不污染主 Session |
| ReflectorSoul | 独立 Session, 不与 ActorSoul 共享 system |
| 上下文压缩 | system 段永不被压缩, 只压缩 user/assistant turns |
| 观测 | 三层诊断 (实时日志 / 聚合指标 / 慢查询告警), system_hash 维度 |
| 质量风险 | 可接受 ±2% 波动, 关键改造 (D2/D4/D6) 走阶梯灰度 5% → 20% → 100% |

## 3. 整体架构

```
┌──────────────────────────────────────────────────────────────────────┐
│  PromptCache (新建)                                                   │
│  ├─ system_immutable: OnceCell<String>      ← 生命周期只 set 一次     │
│  ├─ system_semi_static: ArcSwap<String>     ← ConfigUpdate 触发换     │
│  ├─ system_hash: OnceCell<[u8; 32]>        ← SHA256 监控              │
│  └─ session_messages: Mutex<Vec<Message>>  ← 增量追加                 │
├──────────────────────────────────────────────────────────────────────┤
│  Session (新建)                                                       │
│  ├─ new(): 一次性 build_immutable + semi_static                      │
│  ├─ mutate_semi_static(): 仅 ConfigUpdate 触发                       │
│  └─ append_turn(role, content): 唯一追加入口                         │
├──────────────────────────────────────────────────────────────────────┤
│  ToolSession (新建, EarthSoul 内部)                                   │
│  ├─ 副 messages 数组, 隔离多轮 tool 调用的中间结果                    │
│  └─ 结束时把关键结论 (≤ tool_session.summary_max_chars, 默认 200 字)   │
│     写回主 Session                                                    │
├──────────────────────────────────────────────────────────────────────┤
│  ContextCompactor (新建)                                              │
│  ├─ 触发: session_messages.len() > compactor.threshold (默认 16)     │
│  ├─ 策略: system_immutable + system_semi_static + 旧 turns 摘要 +     │
│  │        最近 compactor.keep_turns 轮 (默认 8)                       │
│  └─ 约束: system 段**永不**进入压缩范围                              │
├──────────────────────────────────────────────────────────────────────┤
│  CacheDiagnostics (新建, infra crate)                                 │
│  ├─ SQLite 表: prompt_cache_log (ts, agent_id, system_hash,          │
│  │                              hit, miss, total, prompt_kind)       │
│  └─ Endpoint: GET /api/v1/config/llm/cache-stats                     │
└──────────────────────────────────────────────────────────────────────┘
```

## 4. 详细设计

### D1. Session 单例化, system 永不动

- **新文件**: `crates/agent/src/core/session.rs`
- 持有三态: `system_immutable: OnceCell<String>`, `system_semi_static: ArcSwap<String>`, `session_messages: Mutex<Vec<Message>>`
- 任何对 `system_immutable` 的写操作 → `panic!` (invariant 违反)
- ActorSoul / EarthSoul / ReflectorSoul 都从 Session 读取 system 段
- `build_system_message()` 拆为 `build_immutable()` + `build_semi_static()`, 由 Session 缓存

### D2. tools schema 文本化进 system

- **新文件**: `crates/agent/src/soul/earth/text_renderer.rs`
- JSON schema 渲染为 TypeScript 函数签名风格, 例:
  ```
  ## 可用工具
  ### query_world(query: string): WorldSnapshot
    描述: 查询世界状态。返回当前 tick 的状态快照。
  ### search_memory(query: string, top_k: int = 5): MemoryHit[]
    描述: 语义检索长期记忆。
  ```
- 文本进入 `system_immutable` (跨 tick 稳定), 进入 prefix 缓存
- API 层的 `tools` 字段保留**极简版** (`{name, params: ["query"]}`), 仅用于 OpenAI 协议兼容
- system 段加规则: "严格按上文函数签名调用, 参数名严格匹配"
- **风险**: LLM 忽略极简 schema 报错 → 阶梯灰度 (5% → 20% → 100%, 每档 24h 观察)
- **回滚开关**: 配置项 `prompt.tools_text_enabled: bool` (默认 true); 设为 false 时回退到原 JSON schema 路径

### D3. events_log 治理: 环形 + 持久化迁移

- `engine_prompts.rs::build_world_state_section` **移除** `events_log` 渲染
- 事件写入 episodic memory 后立即从 `WorldState.events_log` 弹出
- prompt 中 events 仅显示"最近 N 条概要" (单行) + "累计统计: 死亡 X / 攻击 Y / 交易 Z"
  - N = `prompt.events_max_recent` (默认 5)
  - 累计统计按 `event.event_type` 聚合 (death/attack/trade/dialogue/other)
- 配置项: `prompt.events_max_recent: 5` (YAML 外化, 魔法值消灭)
- **新文件**: `crates/agent/src/component/memory/event_drain.rs` 负责 WorldState → episodic memory → 弹出的管道

### D4. EarthSoul 工具调用隔离

- **新文件**: `crates/agent/src/soul/earth/tool_session.rs`
- tool calling 期间的 `assistant(tool_calls)` 和 `tool_result` 全部在**副 messages 数组** (`ToolSession.messages`) 中
- 主 Session 只见 tool 调用的**最终结果摘要** (单条 assistant 消息: "已查询世界, 结论是...")
- 现有 `tool_loop.rs` 重构, 接入 ToolSession
- `max_tool_rounds` 失效 (或保留为兜底, 防止无限循环)
- 异常处理: tool 失败/重试在 ToolSession 内部完成, 不污染主 Session
- **风险**: LLM 后续决策缺信息 → ToolSession 结束时把关键结论 (≤200 字) 写回主 Session

### D5. ReflectorSoul 独立 Session

- `crates/agent/src/soul/reflector/mod.rs` 持有独立 Session 实例
- ReflectorSoul 的 system 段独立构建 (短: "你是动作审查者...")
- 避免 ReflectorSoul 调用污染 ActorSoul 的 prefix
- **不共享** ActorSoul 的 persona / rules / action index

### D6. 上下文压缩 (LLM 摘要)

- **新文件**: `crates/agent/src/core/compactor.rs`
- 触发条件: `session_messages.len() > compactor.threshold` (默认 16, user/assistant turns 数, 不含 tool)
- 压缩策略: 最早的 `compactor.compress_turns` 轮 (默认 8) → LLM 生成 ≤ `compactor.summary_max_chars` (默认 200) 字摘要 → 替换为单条 `user` 消息 ("[对话历史摘要]\n...")
- **关键约束**: `system_immutable` 段和 `system_semi_static` 段**永不**进入压缩范围
- 压缩后的 Session 形态:
  ```
  [system_immutable] [system_semi_static] [summary_message] [recent compactor.keep_turns turns] [current user]
  ```
  其中 `compactor.keep_turns` 默认 8
- 压缩频率监控: log 每次压缩的 (before_len, after_len, summary_chars)
- **风险**: 摘要质量影响后续 LLM 决策 → 阶梯灰度 (5% → 20% → 100%) + 100 轮决策质量对比

### D7. 缓存诊断三层

| 层 | 位置 | 写入 | 暴露 |
|----|------|------|------|
| 实时日志 | `direct_client.rs` 流式结束回调 | `(system_hash, hit, miss, total, prompt_kind)` → SQLite | `GET /api/v1/config/llm/cache-stats?agent_id=X&range=1h` |
| 聚合指标 | `token_tracking.rs` | 已有, 加 `system_hash` 维度 | dashboard |
| 慢查询 | 同上 | `hit_rate < 30%` 触发 warn log | log file |

- `system_hash` = SHA256(`system_immutable + system_semi_static`)
- 该 hash 在 agent 整个生命周期不变, 便于追踪"系统段稳定时" vs "系统段变更时" 两种命中的差异
- **新文件**:
  - `crates/agent/src/infra/api/handlers/cache_stats.rs` (Endpoint)
  - `crates/server/src/migrations/2026XXXX_prompt_cache_log.sql` (SQLite 表)
- 写入策略: 按小时聚合写, 不要每请求写 (避免 IO 压力)

## 5. 文件级改造清单

按依赖顺序:

```
1.  crates/agent/src/core/session.rs                       (新建)
2.  crates/agent/src/core/prompt_cache.rs                  (新建, 替换部分 soul/actor/prompt_cache.rs 职责)
3.  crates/agent/src/core/compactor.rs                     (新建)
4.  crates/agent/src/component/memory/event_drain.rs       (新建)
5.  crates/agent/src/soul/actor/engine_prompts.rs          (重构: build_system_message 拆分)
6.  crates/agent/src/soul/actor/prompt_template.rs         (扩展: immutable/semi_static/volatile 区域)
7.  crates/agent/src/soul/earth/text_renderer.rs           (新建, JSON schema → TypeScript 文本)
8.  crates/agent/src/soul/earth/tool_session.rs            (新建, 隔离主 Session)
9.  crates/agent/src/soul/earth/tool_loop.rs               (重构, 接入 tool_session)
10. crates/agent/src/soul/reflector/mod.rs                 (独立 Session)
11. crates/agent/src/component/llm/direct_client.rs        (扩展: system_hash 透传 + 落库)
12. crates/agent/src/component/llm/token_tracking.rs       (扩展: per-system-hash 聚合)
13. crates/agent/src/infra/api/handlers/cache_stats.rs     (新建)
14. crates/server/src/migrations/2026XXXX_prompt_cache_log.sql  (新建)
15. crates/server/config/prompt_templates.yaml             (区域化: immutable/semi_static/volatile)
16. crates/server/config/llm.yaml                          (新增 cache_diagnostics.enabled)
17. crates/server/static/admin/                            (前端 dashboard 接入新指标)
```

## 6. 验证指标

| 阶段 | 指标 | 目标 |
|------|------|------|
| Phase 0 (D7) | 缓存诊断上线 | baseline 测量就绪 |
| Phase 1 (D1+D2+D3) | 聚合 cache_hit_rate | 33% → 60% |
| Phase 2 (D4+D5) | 单 prefix cache_hit_rate | 60% → 75% |
| Phase 3 (D6) | 100 轮长会话 cache_hit_rate | 75% → 80% |
| 全程 | LLM 决策质量 (死亡率/成功率/剧情丰富度) | 波动 ≤ 2% |

### 监控告警阈值

- `cache_hit_rate < 50%` (持续 1h) → warn, 通知
- `cache_hit_rate < 30%` (持续 10min) → error, 立即排查
- `system_hash 变更次数 > 1/h/agent` → warn, 检查 ConfigUpdate 风暴

## 7. 风险与缓解

| 风险 | 影响 | 缓解 |
|------|------|------|
| tools schema 文本化导致 LLM 调用格式错乱 | LLM 输出破坏, 决策质量下降 | system 段加严格规则 + 阶梯灰度 (5% → 20% → 100%) + 24h 观察 |
| 上下文压缩摘要质量差 | 长会话决策质量下降 | A/B 测试, 对比压缩前后 100 轮决策质量 |
| EarthSoul 工具结果不进主 Session 导致 LLM 后续决策缺信息 | 决策依据不全 | ToolSession 结束时把关键结论 (≤200 字) 写回主 Session |
| system_hash 落库 IO 压力 | 数据库写入瓶颈 | 按小时聚合写; 不要每请求写 |
| 多 system message 兼容性 | 部分 provider 拒绝 | DeepSeek 已确认支持; OpenAI 兼容层也支持; 实施前实测 |
| events 弹出过快导致 episodic memory 写入风暴 | DB 写入压力 | 批量写入 (每 10 条或每 30s flush 一次) |

## 8. 实施阶段

```
Phase 0 (1d):   D7 缓存诊断上线 (baseline 测量)
Phase 1 (1w):   D1 + D2 + D3 (33% → 60%)
Phase 2 (1w):   D4 + D5 (60% → 75%)
Phase 3 (3-5d): D6 (75% → 80%+)
Phase 4 (2d):   D7 dashboard 完善 + 全量放量
```

总计: 约 2.5-3 周

### 灰度策略

每条改造 (D2/D4/D6 关键路径) 走阶梯放量:

```
5%  agent 启用新路径 (24h 观察)
   ↓ 指标无回退
20% agent 启用新路径 (24h 观察)
   ↓ 指标无回退
100% 全量启用
```

期间对照组 = 剩余 95%/80%/0% 的 agent 走旧路径。每阶段结束对比:

- 命中率 (`cache_hit_rate`)
- 决策质量 (死亡率/成功率/剧情丰富度)
- 系统稳定性 (P99 延迟 / 错误率 / DB IO)

## 9. 引用

- DeepSeek 官方文档: https://api-docs.deepseek.com/guides/kv_cache
- Reasonix 源码: https://github.com/esengine/deepseek-reasonix
- Reasonix 关键文件:
  - `internal/agent/session.go:1-30` (Session 单例化)
  - `internal/provider/openai.go:150-168` (reasoning_content 剥离)
  - `internal/agent/agent.go:80-93, 334-335` (聚合命中率)
  - `internal/agent/compact.go:24-35, 113-123` (压缩策略)
  - `internal/agent/cachehit_e2e_test.go:108-148` (前缀稳定性验证)
- 本项目既有 spec: `docs/superpowers/specs/2026-05-14-token-optimization-design.md`

## 10. 待确认事项

无。设计决策 D1-D7 全部经用户 ACK (2026-06-07)。
