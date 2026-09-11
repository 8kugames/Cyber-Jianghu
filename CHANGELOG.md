# Cyber-Jianghu 更新日志

---

## [Unreleased]

### Refactoring

- **时间换算真源收敛完结 + 全局措辞清理**（server）：`try_game_day` 补 `ticks_per_hour` 非法校验（fail-fast 契约闭合）；新增 `TimeRegistry::game_datetime`（年月日展开唯一真源，日历模型 = time.yaml），收编最后两处内联换算——chronicle generator `format_tick_range_chinese` 与 dashboard stats（stats 原自建 30×12 日历且缺 rspt 因子，按 shipped rspt=120 显示比真源快 120 倍，本修为与 WorldState 同构）；broadcaster `compute_game_time` 改为真源委托；`real_seconds_per_game_day` 随调用方归零删除；860 行 `decay.rs` 拆分为 `decay/{mod,death,age}.rs`（衰减引擎/死亡通知/年龄换算，re-export 保持 `tick::decay::*` 路径不变）；全仓清除 Reward 模块旧标签措辞（reward.yaml/reward 模块/summary 等 7 处，统一为「生存 Reward」）；修正 skill_mutator 指向已删 broadcaster.rs 的注释路径。（终审建议项 1-7 落地）
- **地点/时间换算单源化 + 广播器拆分**（server）：新增 `TimeRegistry::game_hours`/`try_game_day` 作为全仓唯一 tick→游戏时/日换算真源，收编业务侧 4 处内联公式拷贝（chronicle `calculate_game_days`/`collect_agents` 周期日、broadcaster `compute_game_time`、decay `compute_age_years`，另 time_registry 内部 3 处归一；补等价性钉死测试）；修复 `from_config` 丢弃节点级 `implicit_travel_cost` 的既有 bug（shipped 配置无此字段，零行为变更）；`resolved_description` 接线 dashboard `GET /api/dashboard/locations`（additive：`game_day` + `resolved_descriptions`）；947 行基线 `broadcaster.rs` 拆分为 `broadcaster/{mod,world_state,time,recipes}.rs`（对外路径不变），`cache.rs` 同步拆出 `location_registry.rs`（两文件均回落 800 行上限内）。

### Features

- **地点时代显隐 time_variants**（protocol+server）：`LocationNode` 新增可选 `time_variants`（游戏日闭区间，首个命中生效），命中 `visible: false` 时段的地块从 WorldState 邻接中隐去且移动终点校验拒绝（起点豁免，与传灯录同语义）；命中时段 `description` 覆盖基础描述。时间基准统一为 `TimeRegistry::game_day(tick)`（executor 与 broadcaster 同源）。PROTOCOL_VERSION 3.0.0 → 3.1.0（additive optional field）。
- **地点图数据画像守卫**（server）：新增 `locations_graph_integrity_test`（全图连通/单向边画像/travel_cost>=1/任意两点最短<=6 tick 预算/出生点全域可达/时代变体自洽）；`load_locations` 加载时 fail-fast 校验悬空边端点、悬空 parent_id、重复 node_id、倒置区间，非对称边仅告警。语义唯一事实文档见 `crates/server/docs/architecture/p0_core/locations_graph.md`。（模式吸收自传灯录拓扑地图）

### Bug Fixes

- **意图失败原因误报"状态持久化失败"**（server）：验证/执行失败经 rollback 后一律误报 persist_failed，掩盖真实拒绝原因（实测"未知的动作类型"被包装成持久化故障，误导诊断且 Agent 无法自纠）。修复：SingleProcessingResult 增加 failure_reason，验证/执行失败走 action_failed 回传具体原因（含治理分类码），仅真实落库失败才报 persist_failed。
- **Agent 动作词表漂移无法自愈**（server+agent）：server 部署/热更新动作配置后，运行中 Agent 仍持旧词表提交已下线动作（实测 Agent 持 20 旧名 vs 部署 actions.yaml 12 新名，全量拒绝）；且 action_update_callback 从未接线，server 的动作 ConfigUpdate 推送全部落空。修复：接线回调（刷新引擎词表+持久化 actions.json）；server 在 UnknownAction 拒绝时即时推送最新动作配置，形成自愈闭环。
- **理智末日时钟（全员永久混沌）**（server+agent）：`get_recovering_attributes` 的 decay≠0 守卫使 sanity 的 recovery_formula 永不生效——理智每 tick 净流失 1 点且无任何恢复路径，跌破混沌阈值 30 后 ChaosGenerator 丢弃全部 LLM 推理决策（实测 4 agent 理智 7-20，"推理与裴沈对话 → 实际提交随机使用/修炼"）。修复：恢复公式按语义分层——decay=0 属性无条件恢复，decay≠0 属性（sanity）仅休息 tick（本 tick 窗口无 intent）恢复；idle-skip 空转/离线/思考间隙均自然计为休息（身体没在做事 = 休息）。IntentWorker 新增 last_intent_ticks 追踪作为休息判定的反向信号。
- **注册 nil 永久等待转生**（agent）：容器重启/断连期间角色死亡后重连，注册返回 nil 即进入等待转生模式且无人唤醒（实测 agent-3 断连 45 分钟无重连、角色死 3 小时未转世）。修复：该入口在 auto_rebirth 开启且本地角色为 Dead 时自动调度转世（game_rules 未到达时回退出厂默认 5 tick）；等待循环增加 5s 轮询，转世完成后立即重连（peek-成功后消费，失败保留下轮重试）。

### Bug Fixes（前序）

- **动作统计泄露 Agent 主观记忆**（BREAKING，agent+server+protocol）：移除 `ServerMessage::DailySummaryData` 推送链路（scheduler 广播 → callbacks 写入 episodic）。客观动作统计（计数/成败/足迹）进入主观记忆后以 0.8 重要性霸榜日记取材 top-K，导致日记复读与 OOC 污染（"今日动作统计仅两次"式叙述），并回流决策上下文与 SFT 导出。PROTOCOL_VERSION 2.0.0 → 3.0.0。附带：Agent 启动时幂等清理遗留 `daily_action_stats` 记忆；日记取材排除 `daily_summary`/`daily_action_stats` 元条目，切断"改写昨日日记"复读环。

## [0.1.297] - 2026-09-10

### Major Features

- **client P1-P7 契约前置**（agent+server）：state/stream SSE 复合流、独立 `protocol_version` 双侧 version 端点、静态 token 并集鉴权（`CYBER_JIANGHU_AGENT_TOKEN` ∪ device token）、契约 JSON Schema 片段（docs/contracts/）、memory `?since=` 增量补帧、characters id 路由
- **物品/配方全面 uuid 化**：物品与配方标识统一为 uuid，展示名统一为 `名称[短uuid]`

### Bug Fixes

- **死亡链路与目击传播**（BREAKING，agent+server）：修复死亡事件传播链路，移除教训广播改为纯涌现（f4a2da1c）
- **refresh_auth_token 读锁自死锁**（agent）：读锁未释放导致注册重试自死锁（fd661a5c）
- **dynamic_persona 死接线**（agent）：接通断线调用，state/stream 恢复主角名/情绪显示（4abf90ef）
- **LLM 调用弹性**（agent）：429 分级冷却自愈、认知重试退避、认知/日记 schema 容忍（a11049a5）
- **群像传记 modal 滚动条**（admin，555a26fc）

### Performance

- **tick 调速与成本杠杆**：tick 120s + 空转跳过昼夜节律 + 四条 LLM 成本杠杆（53a2e8b9）

### CI

- **quality 质量门**：发布管线新增 fmt/clippy/nextest 门并与 build 并行，结果串入 docker-build/release 守卫——杜绝 v0.1.292 式"tag 发布成功而 main/dev clippy 红"的带病发布；同步修正 job 注释（1da6c229）
- **pre-commit 对齐**：不再尝试入库 Cargo.lock（对齐 untrack 决策），输出消歧显式播报（e511c49d、736a3ec0）

### Build

- **依赖版本地板三段化**：Cargo.lock 不入库前提下，workspace.dependencies 与各 crate 直接依赖全部收紧到当前最新解析的三段版本地板（tokio 1.53.1、clap 4.6.6、candle 0.10.2 等），收窄 CI 解析漂移（9941f313）

### Tooling

- **/release 流程与 hook 对齐**、**pre-commit 版本号控制落地**（承接自上周期 Unreleased 条目，随本版发布）
- **联调工具链**：一键部署注册、离线构建、健康快照（check-round.sh）、监控链整合（9007cd2f 等）
- **.gitignore 补 `!.github` 例外**：`.*` 通配误伤 workflow 目录，新 workflow 文件不再被静默忽略（c1464d04）
- **验收快照输出迁移 `./.tmp`**：对齐 .gitignore/.dockerignore 忽略规则（363d8d13）

### Refactor

- **soul_cycle 托梦域拆分至 dream.rs**（agent，文件行数治理，55293309）

### Tooling

- **/release 流程与 hook 对齐**（`.claude/skills/release/SKILL.md`）：重排为「先提交全部代码（hook bump 落定）→ 读定稿版本转正 CHANGELOG → 仅含 CHANGELOG.md 的发版提交（不触发 bump）→ 打 tag」，保证 tag v<VER> = 打 tag 时 Cargo.toml = CHANGELOG [VER]，消除旧流程 tag 落后 1 个 patch 的错位
- **pre-commit 版本号控制落地**（`githooks/pre-commit`）：吸收并删除 `scripts/version-bump.sh`——暂存区含 `crates/<crate>` 的 .rs/.toml/.yaml 变更时该 crate 版本 patch +1，同步依赖方 path 依赖版本，被改动的 Cargo.toml 与 Cargo.lock（`cargo update -w` 同步刷新）自动 git add 并入本次 commit（补齐原脚本“声称已 add 但未实现”的缺口）；CRATES 清单补上 workspace 成员 embedding；移除会吞依赖名的 sed 回退分支，统一走 perl 并前置快速失败；patch 段 10# 强制十进制防前导零。激活：`git config core.hooksPath githooks`（`install.sh` 任意命令执行时幂等自动激活）；跳过：`git commit --no-verify`

## [0.1.292] - 2026-09-06

### Major Features

- **WitnessedDeath 目击死亡特质演化**（白皮书 05_心智模型 §5 压力源落地）：死亡通知具名化（`witness_description`），死亡善后向同位置存活 Agent 的 events_log 写入 DeathNotification（记忆 + 特质演化规范通道，best-effort）；persona_event_rules 新增 WitnessedDeath→恐惧/沮丧规则（26→28 条，EXPECTED_RULE_COUNT fail-fast 门禁同步）；query_world↔compactor 合同测试锁定 execute_query_world 输出与 compact_tool_result 读取的 JSON 键契约，防 compactor 静默失效

### Bug Fixes

- **目击者假死回归**（agent）：lifecycle 路径 1 的死亡检测原用 `any(DeathNotification)` 判定自身死亡、不校验死者——目击他人死亡即触发 handle_death，is_dead 永久置位，而 server auto-rebirth 按 `status='dead'` 守卫拒绝活体重生，目击者永久停在决策跳过循环。新增 `find_self_death` 比对 `metadata.agent_id` 与自身 ID；身份无法核对时 fail-safe 不触发（误报不可逆，漏报由 AgentDied 回调路径兜底）；6 个单元测试锁定（目击不假死、自身死亡仍可检测、同 tick 混合日志、WitnessedDeath 特质演化联合合同）

### Tooling

- **验收 run 观测快照脚本**（`scripts/acceptance_snapshot.sh`）：定时抓取 健康度看板与涌现检测端点落盘 JSON 快照，只读观测不干预；配套 `docs/reports/acceptance-run-config-2026-09.md` 验收配置锁定与 run 纪律（涌现基线回溯报告按项目约定留盘 docs/reports/，不进 git）

## [0.1.291] - 2026-08-06

### Performance

- **LLM prefix cache 稳定化**（agent）：skill index 固定排序保证 system prompt 前缀字节级稳定；新增 system_hash 漂移告警（`cache_diagnostics` target）观测 provider prefix cache 失效

### Bug Fixes

- **prefix 漂移告警伪报**（agent，WI-010）：actor/validator 等调用类型共享同一 DirectLlmClient 并交替使用不同 prompt，单值"上一次 hash"比对在合法交替时持续误报。改为已知 hash 集合语义（容量 64，超限重置），仅全新前缀告警
- **诊断路径多字节 panic**（agent，WI-011）：tool_calls 预览切片与 JSON parse 错误日志在内容含中文（多字节）时按字节偏移切片 panic。新增 `utf8_safe_end` 回退到字符边界；JSON parse 错误详情直写 message
- **test-agents virtiofs 权限**：agent-5-longcat 目录经 virtiofs 映射为 root 属主，entrypoint chown 撞只读 trace.yaml 失败。设 `CONTAINER_UID/GID=0` 跳过权限修复

### CI

- pr-check 补 push 触发，gate main/dev 集成
- Docker job 移除 `cache-to`，根治 GHA cache not_found

## [0.1.290] - 2026-07-27

### Major Features

- **专用模型训练数据管线（reward + trace + SFT export）**：围绕"将 Agent 产生的 LLM 调用用于专用模型训练"目标，构建完整的数据采集→回传→导出闭环。哲学锚点：天道无为——reward 纯锚定生存因果，声望/关系/心境等主观认知不进 reward。
  - 生存 Reward（server）：每日结算（生存+生理+天魂审查三分量）+ 一生结算（寿数+统一死亡 penalty）+ 周期聚合 + 仪表盘 API（`GET /api/dashboard/reward/trends`、`/reward/lifetime/{id}`）；强制配置 `reward.yaml`（fail-fast），走 game_data 标准管线零硬编码；数据源改用 `agent_state_cache`（DashMap）消除时序竞态
  - 训练 Trace 结构化落盘（agent）：人魂/天魂 LLM 调用 JSONL（含 agent_id+tick_id+prompt/response 全文+soul_stage+persona+wall_clock），persona 字段替代 system_prompt 全文（~200B vs ~15KB），日志滚动覆盖（`max_size_mb` 配置，LRU 删旧）
  - **SFT 导出管线**（`src/training_export/`）：config 加载（复刻 action_evolution 模式 + env 覆盖）+ db 查询（`fetch_soul_cycle_metadata`，DISTINCT ON + UNNEST + SET LOCAL）+ runner（`run_once` 五步数据流）+ scheduler 后台 task（双层 timeout + sweep + shutdown）+ sft_transform 纯函数（对齐 Python `--no-db-filter`）+ checkpoint trace_id 集合与日期分桶 TTL；6 个 HTTP 端点（`/api/v1/training/export|exports|exports/{run_id}|exports/{run_id}/download|checkpoint`，写/读权限按 method 隔离）；cancel-aware 原子写 + 全量 env 覆盖 + per-bucket 容量 + 黄金对照测试基线
- **关系图谱（C1-C4 数据可达性）**：`agent_relationships` 表（迁移 022）+ Strategy B 全量快照同步（Agent 每游戏日上报，天然幂等）+ 关系/世界快照/地点/对话/死亡 dashboard 端点 + C2 鉴权档（`require_client_read_token`，`CLIENT_READ_TOKEN` 与 admin read 分离）
- **涌现检测接入 server**：causal_emergence / co_occurrence 检测 + 健康度看板 + Chronicle 叙事打磨；Validator 增强（物品/人员 ID 存在性校验，三端打通）
- **三魂元数据随 intent 提交**：消除独立 SoulCycleReport 的丢失风险（与 intent 同消息到达），执行结果回填

### Refactors（数据诚实化 / protocol 为唯一真相源）

- 枚举化穷尽匹配，消除 String 闭集与静默跳过：`OocRisk`、`ConfigType`、`EffectType`、`RequirementType`、`Operation`、`ValidationType`、`ItemType`（统一 5 变体，修复 Material/Tool 降级，删除 armor 死代码）、`LocationNodeData.node_type`（修复 region 被吞成 Map）
- 合并重复类型：`NarrativeThreshold`/`AttributeDriveConfig`（→ protocol）；`LocationNodeData` 改为 `LocationNode` 别名；parent_id 哨兵消除 + wall_clock 时间戳统一为 i64 毫秒
- `upsert_agent_state` 纳入 tx——消除跨表部分提交窗口

### Bug Fixes

- **agent WS 重连风暴**（`8d130537` 回归）：`handle_worldstate_send_failure` 误在零 receiver 时清空 `worldstate_tx`，而零 receiver 是 select! 间隙/初始 WorldState 推送的正常瞬时态→ `receive_world_state` 误判"Not connected"→ ~3Hz 重连死循环、零 intent。改为仅分级记日志、保留 sender
- **chronicle_id 生成 SQL**：`format('C-%03d', n)` 用了 PG 不支持的 C printf 说明符（PG 仅 `%s/%I/%L`）→ 每次群像传记落库失败。改用 `'C-' || lpad(n::text, 3, '0')`
- **tick/scheduler 游戏日边界漂移**：`tick_counter` ordinal 替代 `current_tick_id` modulo 墙钟秒，消除重启后偶发漂移
- **dashboard 经历日志 model_id**：归一到直读字段，根除 JSONB 解析脆弱性
- **reflector 硬性 OOC 词**：绝对禁止项不受生存凌驾/人设边界豁免（仅封顶到行为类意图）
- **run_migrations**：改用 `sqlx::raw_sql` 支持多语句，修复 dev 启动阻塞
- **emergence fetch_health**：`EXTRACT(EPOCH)` 返回 numeric 导致 sqlx panic
- **agent-web llm-disabled 持久化**：SSE 认证/协议断层（8 层根因修复）
- **chronicle get_chronicle**：读 raw_data 改用 Option 防 NULL 崩溃

### Quality

- **cargo fmt 全局归一化**：对齐当前 stable 的 let-chain 格式（CI `@stable` fmt 门），含 ReflectorSoul OOC 禁词表微调（移除"属性"，放宽元游戏过滤）
- clippy `-D warnings` 累积 lint 清理，dev 上线前质量门

## [0.1.267] - 2026-07-01

### Bug Fixes

- **tick/scheduler**: 修复游戏日边界检测（生存奖励结算+编年史）在服务器重启后的偶发漂移。使用 `tick_counter` ordinal 计数器（每 tick +1）替代 `current_tick_id` modulo 墙钟秒判断，消除 modulo 对齐对墙钟余数的偶发依赖，确保每 N 个 tick 精确触发一次

### 专用模型训练数据管线（reward + trace + 导出）

围绕"将 Agent 产生的 LLM 调用用于专用模型训练"目标，构建完整的数据采集→回传→导出管线。哲学锚点：天道无为——reward 纯锚定生存因果，声望/关系/心境是众生主观认知不进 reward。

- **生存 Reward**（server 侧）：
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
>
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
