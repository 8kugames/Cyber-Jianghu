# Cyber-Jianghu 更新日志

---

## [Unreleased]

## [0.1.267] - 2026-07-01

### Bug Fixes

- **tick/scheduler**: 修复游戏日边界检测（生存奖励结算+编年史）在服务器重启后的偶发漂移。使用 `tick_counter` ordinal 计数器（每 tick +1）替代 `current_tick_id` modulo 墙钟秒判断，消除 modulo 对齐对墙钟余数的偶发依赖，确保每 N 个 tick 精确触发一次



### 专用模型训练数据管线（reward + trace + 导出）

围绕"将 Agent 产生的 LLM 调用用于专用模型训练"目标，构建完整的数据采集→回传→导出管线。哲学锚点：天道无为——reward 纯锚定生存因果，声望/关系/心境是众生主观认知不进 reward。

- **生存 Reward 天道账本**（server 侧）：
  - 每日结算（每游戏日=12tick）：生存分量 + 生理分量（satiation/hydration 归一化）+ 天魂审查分量（approved/rejected）
  - 一生结算（死亡时）：寿数 + 统一死亡 penalty（不分死因），不完整日按完整 `compute_daily_reward` 补算（生存按比例+生理死亡真值+天魂真值）
  - 周期聚合（复用 chronicle 7 日周期）+ 仪表盘 API（`GET /api/dashboard/reward/trends`）
  - 强制配置（reward.yaml，fail-fast），走 game_data 标准管线，零硬编码
  - 数据源改用 `agent_state_cache`（DashMap）消除时序竞态；`get_config`/`get_attribute_max_value` 失败显式 error 日志（非静默）
  - **BREAKING**：reward.yaml 为强制配置，缺失即中止启动（对齐 display_messages_loader fail-fast 模式）

- **训练 Trace 结构化落盘**（agent 侧）：
  - 人魂/天魂 LLM 调用结构化 JSONL（含 agent_id UUID + tick_id + prompt/response 全文 + soul_stage + attempt + persona_name/description + wall_clock）
  - persona 字段替代 system_prompt 全文（~200 bytes vs ~15KB，静态模板由配置复用，解决训练-推理分布不匹配）
  - 日志滚动覆盖：`max_size_mb` 配置（默认 1024MB=1G），超限按 LRU 删除最旧文件
  - SoulStage 枚举仅 Renhun/Tianhun（地魂不是独立调用方——其 tool-calling 是人魂内部轮次）
  - 强制配置（trace.yaml），默认开启采集+回传，零开销（Mutex 聚合 + 异步 flush）

- **Trace 回传 server**：
  - 协议新增 `ClientMessage::TraceReport` + `TraceEntry`，复用 websocket 回传
  - sender 注入：连接成功后 `set_upload_sender`（解决 init 在连接前的时序问题）
  - server `handle_trace_report` 落盘到 `traces/`（与 rewards/ 同目录树）
  - 原文记录（无脱敏——玩家角色均为 LLM 驱动，无隐私内容）
  - 并发修复：文件名含 agent_id，消除多 agent 同机并发写冲突

- **attempt 透传（DPO 配对根基解）**：
  - **BREAKING**：`DecisionWithChainCallback` 类型 + `think_direct` + `think_with_memory_and_feedback` + `cognitive_decision_with_chain` + `self_correct_intent` 签名均新增 `soul_cycle_attempt: i32` 参数
  - trace 记录真实外层 soul_cycle attempt（非 wall_clock 重建），DPO 配对精确可靠

- **训练数据导出脚本**（离线工具，只读）：
  - `scripts/build_sft_data.py`：筛天魂 approved 样本 → messages JSONL（system role 从 persona 重建），可选 --top-longevity
  - `scripts/build_dpo_data.py`：天魂 reject→approve 偏好对 → chosen/rejected JSONL（prompt 为含 system persona 的 messages 数组）
  - `scripts/analyze_social_structure.py`：恩怨双图 PageRank（观察工具，不写回 agent 状态）

- **端侧关系认知**（agent 侧，万物自化）：
  - 人魂 prompt"附近的人"段落注入 agent 对此人的主观关系认知（好感度+等级）
  - 完全本地：每 agent 只查自己的 relationship_store，尊重不对称，不进 reward

- **用户数据使用说明**：
  - Readme 新增「用户数据使用说明」章节
  - `docs/DATA_USAGE.md` 数据使用透明文档（采集内容/Opt-out 选择权）

**验证**：814 测试全绿，clippy 0 warning。双签 review 通过。

### CHANGELOG 版本转正 + release skill

- `scripts/version-bump.sh` 保持 pre-commit 模式（只 bump patch，不动 CHANGELOG）
- `/release` 指令（`.claude/skills/release/SKILL.md`）新增 Step 1：CHANGELOG `[Unreleased]` → `[版本号] - 日期` 转正

### 审计残留 4 项根治（P0-2 / P0-11b / clippy / warn! 测试）

针对 `logs/audit/audit-report-2026-06-24.md` 中 4 项残留问题的第一性原理根治：

- **P0-2 action_log 纳入 Saga 事务**：`batch_insert_action_logs` 签名从 `&PgPool` 改为 `&mut sqlx::PgConnection`，调用点移到 `tx.commit()` **之前**。若 action_log 插入失败，整个 Saga 回滚（state 不落库），消除"state 已提交但 log 丢失"的可观测性缺口。删除 4 行 `[RAW-DEBUG-batch]` 调试残留。新增 `test_p0_2_action_log_insert_before_commit_and_uses_tx` 源码契约测试。
- **P0-11(b) Agent HTTP API 认证层**：新增 `crates/agent/src/infra/api/auth.rs` 中间件（`require_device_token`），镜像 server 端 `require_*_token` 模式，通过 `Authorization: Bearer <token>` 验证 device auth_token。两个入口点（`run_http_server` + `run_ws_server`）均挂载。白名单：health/静态资源/setup。fail-closed：device 未配置时返 503。`setup/status` 端点暴露 token 供本地 Web 面板（`api.js` 新增 `buildHeaders`/`refreshAuthToken`）。新增 14 个认证测试。**BREAKING**：之前无认证的端点现在需要 Bearer token。
- **19 个 clippy warning 清零**：14 个 `collapsible_if`（let-chains）、4 个 `explicit_auto_deref`（`&mut *tx` → `&mut tx`）、1 个 `too_many_arguments`（`auto_rebirth_agent` 引入 `AutoRebirthParams` 结构体）。**BREAKING**：`auto_rebirth_agent` 签名变更（后 5 参数打包为结构体）。
- **42 处 warn! 行为测试**：新增 `tracing-test` dev-dep + 6 个代表性 warn! 契约测试（broadcast/mpsc/watch 三类 channel × 失败/成功两路）。**RED-GREEN 验证**：临时移除 warn! 代码确认测试 FAIL，恢复后确认 PASS。

**验证**：864 测试全绿（agent 495 + server lib 174 + 其他），6 PG 测试 ignored，clippy 0 warning，workspace 编译干净。

### Agent 死亡链路硬化

agent 死亡与转世重生全链路 P0 修复，消除幽灵状态与 auto_rebirth 永久拒绝风险：

- **Path 4 死亡检测**（`e66caafd`）：在原有 `is_dead` 原子标志（WebSocket `AgentDied` 回调）之外，新增从 intent 执行结果 error 字符串检测 `"is dead"` / `"not in cache"` 模式的双路径检测。修复"WebSocket 未收到 AgentDied 消息 → is_dead 永不为 true → agent 空转 + auto_rebirth 不触发"链路（联调测试 0615.docker.1 中 5 次死亡 3 次命中）。
- **转世重建 MemoryManager + PersonaStore**（`e8a16f96`, #51）：前世记忆/人格在新 agent_id 下未清空导致仍引用旧 DB；重生时清空情景/语义/工作记忆与人格/经验。
- **active 校验 + action 致死善后**（`a9d91016`）：
  - `get_all_alive_agents_latest_states` 加 `a.status='active'` 过滤，避免 retired/dead 历史 agent 启动时被加载进 DashMap (#49)
  - `IntentWorker` 处理前对 DB 二次校验 status='active'，命中残留即自愈移除 DashMap 条目 (#50)
  - step 11 + subsequent 路径检测 `is_alive=false` 时复用 `handle_deaths` 统一回写 status='dead'

### 武侠化叙事感知

让 LLM 真正以江湖中人视角感知世界，剥离游戏化术语与数值：

- **隐藏属性数值与术语**（`3ce60c09`）：prompt 增加"禁止元游戏术语"约束（HP/SAN/属性/数值/状态栏），`engine_prompts` / `context` 完全移除原始属性名和数值，仅输出叙事描述。
- **narrative_config 细化低值分段**（`3ce60c09`）：hp / satiation / hydration / sanity 低值分段（10-19 与 0-9 拆分），更精细的叙事化降级。
- **clippy + fmt 修复**（`f7e96d11`）：`values()` 迭代 + realtime fmt 修正。

### CI/CD 关键修复

- **embedding Dockerfile 致命 bug**（`ffbfbba7`, P0）：复制 workspace `Cargo.toml` + protocol/embedding/server 三个 crate 的 `Cargo.toml`，但缺失 `crates/agent/Cargo.toml`，导致整个 server stack 构建失败（`error: failed to load manifest for workspace member`）。补 COPY 行对齐 server Dockerfile。
- **Dockerfile.ci 同步**（`dcf8010c`）：同步上述 Dockerfile 修复到 Dockerfile.ci（embedding crate workspace）。
- **release job 依赖 docker-build**（`22c0000b`）：release job 必须等待 docker-build 成功，防止 docker 失败但 release 幽灵发布。

### 仓库维护

- **untrack Cargo.lock**（`f0834eb7`）：`.gitignore` 已列出但历史 force-added，解除跟踪后 cargo 构建不再污染 working tree。

### 白皮书

- **三皇共治描述顺序调整**（`489e5f58`）：白皮书 `05_宏观模型.md` 段落顺序调整（立场 → 决策机制 → 分权制衡），无内容变更。

---

> **BREAKING**（治理数据流重大重构）：
> - 删除 `ProposedActionIR` + `IRSource` 类型（protocol crate 0.1.73 → 后续版本）
> - DB migration 013 删除 `action_evolution_proposals` 表 IR 字段（actor_arity / target_arity / tick_span / phase_count / protocol_kind / effect_refs / requirement_refs）
> - DB migration 014 新增 `action_evolution_proposal_groups.stage` 列
> - `ProposalRequest` 字段变更：删除 `ir: Option<ProposedActionIR>`，新增 `action_data: serde_json::Value`
> - `ReviewVerdict` 新增 `reject_reason: Option<RejectReason>` + `inferred_action_config: Option<InferredActionConfig>` 字段
> - `GroupVote.vote` 类型从 `ProposalStatus` 改为 `VoteChoice`（与 votes 表字符串对齐）
> - `SoulsReviewConfig` 删除 `reject_threshold` 字段（管道不再使用）

### 三皇共审管道（Three-Soul Pipeline）

火云洞天宏观治理智能——三皇各司其职共审动作演化提案：

- **伏羲氏（演化之主）**：世界多样性 + 演化方向，倾向引入新变量。初审 + 终审双角色。
- **神农氏（生存之主）**：种群生存率 + 资源平衡，倾向稳健生态策略。同辈并行审议。
- **轩辕氏（秩序之主）**：世界观稳定秩序（天道法则自洽 + 世界循环稳定），不审查个体 agent 命运。同辈并行审议。

**三阶段管道**（每个 group 按 stage 持久化推进）：

```text
阶段 1：伏羲初审（awaiting_fuxi_initial）
  ├─ 拒绝 → 整组关单
  └─ 批准（含 inferred_action_config）→ 推进阶段 2

阶段 2：神农 ‖ 轩辕并行（awaiting_peer，tokio::join!）
  ├─ 全部拒绝 → 整组关单
  └─ ≥approve_threshold（默认 2/3）→ 推进阶段 3

阶段 3：伏羲终审（awaiting_fuxi_final，注入同辈反馈）
  ├─ dissent_log 阈值检查 → 升级 EscalatedAdmin
  ├─ 写入 actions.yaml 失败 → 保持 awaiting_fuxi_final 等下轮重试
  └─ 写入成功 → Approved + Done
```

**关键设计**：
- 禁止弃权（LLM 超时/失败强制 Reject）
- 同 similarity_key 多 proposal 共享 fate
- stage 持久化，重启可断点续跑
- close_stale_groups 仅关闭 awaiting_fuxi_initial 超时 group
- 写入失败保护：避免 group 标 Approved 但 actions.yaml 未写入的状态分裂

### 配置变更（souls.yaml）

- 启用 shennong（survival）+ xuanyuan（order）
- `topic_to_soul` + `topic_priority` 三皇完整映射
- `approve_threshold: 2`（含伏羲初审 + 至少一票同辈批准）
- 删除 `source_bindings` 配置（Phase 2 多 soul metric 监控延后）
- 删除 `reject_threshold`（管道不用，仅 approve_threshold 生效）

### 延后项（明确登记，Phase 2 实施）

- 神农氏核心职责：种群生存率/资源平衡/生态稳健的指标监控
- 轩辕氏核心职责：世界观稳定秩序监控（法则自洽/循环稳定/规则套利防御）

### 审计修复（前置 commit）

本次变更前已修复伏羲审议全链路审计问题（commits `f750b64d` ~ `ca463a08`），详见 git log。

---

## [历史归档]

> **BREAKING**: `ExecutionResult` 新增 `governance_code` 字段（向后兼容：`Option<GovernanceCode>` 序列化时 `skip_serializing_if`）。protocol crate 版本从 0.1.68 升级到 0.1.69。

### 治理系统 (Governance)

- **Soul 审议引擎 (SoulReviewEngine)**
  - 新增 `SoulReviewEngine`：基于 `souls.yaml` 配置的投票式提案审核引擎，支持硬性规则 (hard_reject_if / hard_approve_if) 自动裁定。
  - 引入 `ProposalStore`：提案组生命周期管理（PendingReview → UnderReview → Approved/Rejected/EscalatedAdmin）。
  - `TopicClassifier`：基于 `action_evolution.yaml` 规则的治理主题分类器，自动路由提案到对应 Soul。
  - 审议结果广播：Approved 提案组通过 `ConfigUpdate` 广播到所有在线 Agent。
- **动作演化 API (Action Evolution API)**
  - 新增 `POST /api/v1/action-evolution/propose`：Agent 提交动作演化提案。
- **数据库表**
  - `action_evolution_proposals`：动作演化提案表。
  - `action_evolution_proposal_groups`：提案组表。
  - `soul_review_votes`：Soul 审议投票记录表。

### 核心架构 (Core Architecture)

- **实时 Intent 处理管道 (Realtime Pipeline)**
  - 彻底退役 Tick 批处理，Intent 实现实时执行。
  - 引入 `IntentWorker`：单消费者 MPSC Channel 设计，消除所有状态锁竞争，确保完全无竞态。
  - `StateProcessor` 实现单 Intent Saga 事务回滚，保证 `DashMap` (内存 Write-Through 缓存) 与 PostgreSQL 绝对一致。
- **设备身份与角色分离 (Device-Character Separation)**
  - 角色生命周期重构：死亡角色保持 `dead` 状态，转世重生 (Auto-Rebirth) 从 UPDATE-in-place 改为 INSERT 全新 `agent_id`，保留设备与新角色的映射。
  - 归隐语义 (`retired`) 严格专属玩家主动操作，消除幽灵重生错误。
- **数据驱动的动作体系 (Data-Driven ActionType)**
  - `ActionType` 彻底数据驱动化。`transmission` (Broadcast/Session/Silent)、`display_name`、`validator_kind` 剥离硬编码，由 `actions.yaml` 定义。
- **动作系统 v2 原子化重构 — BREAKING**
  - 从 20 个语义化动作精简为 10 个原子原语：予/取/用/移动/说话/观察/攻击/休整/制造/教导（原给予/偷窃/进食/饮水/拾取/丢弃/采集/私语/大喊/打坐/修炼 已移除）。
  - 予/取/用 替代所有物品交互（出背包/入背包/消耗），纯方向性无社会语义。
  - 说话 通过 channel 参数（public/private/broadcast）统一三种形态。
  - 所有旧动作名从代码、配置、提示词、前端、测试中彻底移除，不做向后兼容。
  - 遗留数据中旧 action_type 字符串（如 "eat", "give"）将触发"未知的动作类型"错误。
- **关系图谱迁移 (RelationshipStore PRAGMA Migration)**
  - SQLite PRAGMA `user_version` 自动迁移落地，`relationships` 表从 5 列扩展为 7 列（新增 `self_description`, `description_tick`），代码量精简。

### 智能体认知 (Agent Cognitive)

- **Embedding 服务独立化 (Embedding Service Extraction)**
  - 提取独立 `crates/embedding/` crate，支持双模式部署：Docker 环境使用远程 HTTP 服务（端口 23350），进程部署使用本地内嵌推理。
  - `EmbedderService` 自动检测 `CYBER_JIANGHU_EMBEDDER_REMOTE_URL` 环境变量选择 Local/Remote/Unavailable 三种 provider。
  - 模型下载使用 reqwest + SHA256 校验（无 hf-hub 依赖），Dockerfile 构建时预下载模型。
  - Remote 模式 fast fail，不静默降级；OnceLock + async `ensure_initialized()` 消除 TOCTOU 竞态与 `block_in_place` 反模式。
- **Token 与注意力优化 (Attention & Token Optimization)**
  - 引入 `WorldStateStore` 与 `DeltaEngine` 记录状态增量，基于 `AttentionController` 输出 `FocusSummary`，替代全量 WorldState 注入 Prompt。
  - DeepSeek 前缀缓存调优：基于 system_hash 监控指标、D8 Reasoning 剥离、D9 JSON Schema 递归排序标准化，提高缓存命中率。
  - 规则按需检索 (Rule-On-Demand)：系统提示词仅保留索引，LLM 通过 `query_rules` 检索全文。
- **三魂架构演进 (Three-Soul Evolution)**
  - ReflectorSoul (天魂) 三层审查统一，作为唯一的 Intent 合规性验证入口。
  - 剥离所有别名容错，要求 LLM 输出精准 ID，错误由 ReflectorSoul 以叙事形式反馈拦截。
- **情绪-记忆联动 (CoreAffect)**
  - 基于 Barrett 情绪建构论，实现效价×唤醒度的核心情感计算，情绪门控增强记忆重要度，实现心境一致性检索偏置。

### 控制台与运维 (Admin & Control Panel)

- **Agent Web Panel SPA 重构**
  - 6 个分散的 HTML 重构为 3 页 SPA (#/dashboard, #/characters, #/settings)，消减冗余 CSS，实现细粒度组件渲染。
  - 暴露完整的 LLM 参数控制与热重载接口，支持自动重生配置动态开关。
- **指标与监控**
  - 完善 `/api/v1/metrics?system_hash=<hex>` 接口，支持精准的 LLM 性能度量与 Token 消耗分析。
