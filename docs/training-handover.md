# 训练数据管线交接文档

> 更新：2026-09-28。本文档面向接手「专用小模型训练」工作的人，覆盖：数据链路现状、服务器环境、已修复问题、操作手册、MLX 本地训练方案与验收标准。

## 1. 目标与当前状态

目标：用游戏运行期采集的 Agent-LLM 交互数据（trace），微调文本小模型替代通用大模型的部分调用，降低 token 成本。项目设计口径见 `docs/DATA_USAGE.md`；本管线是它的实现。

当前状态（2026-09-28）：

| 环节 | 状态 |
| --- | --- |
| 采集（agent 本地 trace 落盘） | 运行中，4 个 agent，`output.enabled=true` |
| 回传（agent → server traces/） | 运行中，双格式（毫秒/RFC3339）均已兼容 |
| 存量归集 | 39,480 条已入 server 汇聚点 + 本地备份（`training_data/archive/`） |
| SFT 导出 | 已打通，首次全量 run `01M3HT7J8R6JEEWZYHN42KHB1Z`：29,409 traces → 6,620 samples（33MB） |
| 天魂标签解析 | 完成：10,284 条，approve 7,912 / reject 2,372（23.06%），产物在 `training_data/tianhun_labels/` |
| 训练实验 | 未开始（本机 MLX 方案见第 6 节） |

## 2. 数据链路

```text
agent 每 5s flush ──> 本地 traces/soul=X/agent=<id>/date=<d>.jsonl
        │ upload.enabled=true（TraceReport over WebSocket）
        ▼
server data/traces/soul=X/agent=<id>/date=<d>.jsonl   ← 汇聚点（SFT 导出输入）
        │ training_export scheduler（6h）或 POST /api/v1/training/export
        ▼
data/training_exports/sft/run=<ulid>.jsonl            ← SFT 产物（messages 格式）
        │
        ▼
本地训练（MLX LoRA）→ mlx_lm.server（OpenAI 兼容）→ agent yaml 接入
```

关键语义：

- SFT 导出只扫 **人魂**（`traces/soul=renhun`），天魂走标签/分类路线。
- 样本准入：trace 的 `(agent_id, tick_id, attempt)` 必须能在 server DB `agent_action_logs.soul_cycle_metadata` 关联到天魂裁决，且裁决为 approved；其余标记 processed 不产出。
- checkpoint（`training_exports/sft_checkpoint.json`）按日期桶记录已处理 trace_id，`retain_days=7`；`force_full=true` 绕过日期下限与去重（全量重跑会重复产出已导样本，run 文件各自独立不受影响）。
- wall_clock 双格式：agent 本地是 RFC3339 字符串，server 回传落盘是毫秒 i64，runner 侧 `TraceLine` 宽松解码（2026-09-28 修复）。

## 3. 服务器环境（47.102.120.116）

- SSH：`ssh 47.102.120.116`（`~/.ssh/config` 有别名，User=admin；常用操作 sudo 免密）。
- Docker（userns 重映射：容器 root = 宿主机 uid 1000，**宿主机目录属主给 1000 才可写**）：

| 容器 | 说明 | compose |
| --- | --- | --- |
| cyber-jianghu-server | 游戏服务器 :23333，v0.1.366+（含 2026-09-28 修复） | `/home/admin/Cyber-Jianghu/crates/server/docker-compose.prod.yml` |
| cj-agent-2/3/4/5 | 4 个常驻 agent | `/home/admin/cj-review-agents/docker-compose.yml` |
| cyber-jianghu-postgres | DB（POSTGRES_USER=postgres / DB=cyber_jianghu） | server compose 内 |
| cyber-jianghu-embedding | 向量服务 :23350 | agent compose 内 |

- 关键路径：
  - server 汇聚 trace：`/home/admin/Cyber-Jianghu/crates/server/data/traces/`
  - SFT 产物：`/home/admin/Cyber-Jianghu/crates/server/data/training_exports/sft/`
  - agent 本地 trace：`/home/admin/cj-review-agents/agent-N/data/traces/`
  - agent 配置：`/home/admin/cj-review-agents/agent-N/config/`（`trace.yaml` 必须非空，见第 4 节事故 1）
  - Admin token（每次 server 重启后重新生成）：`/home/admin/Cyber-Jianghu/crates/server/logs/cyber_jianghu_admin.tmp`
- 注意：agent-5 的 LLM 已被手动停用（token 耗尽），恢复后才有 trace 产出。

## 4. 已修复问题台账（2026-09-28，均可从 git log 追溯）

1. **trace.yaml 空文件事故**：7 月 10 日部署搭建时创建 0 字节占位 `trace.yaml`，潜伏至 9 月 18 日 agent 重启后配置加载失败、采集静默停止 12 天。教训：部署时配置文件必须来自仓库模板，禁止手搓空占位；agent 重启后应核对日志含 `[trace] trace 已启用`。
2. **SET LOCAL 绑定参数**（server `training_export/runner.rs`）：`SET LOCAL statement_timeout = $1` 语法错误——SET 是工具语句不接受绑定参数。已改为 u64 字面量拼接。
3. **wall_clock 双格式**（同文件）：runner 只认毫秒 i64，agent 本地 RFC3339 全部解析失败（29,369 行静默跳过）。已加 `TraceLine` 宽松解码。
4. **OnceLock 回传通道**（agent `infra/api/trace.rs`）：`UPLOAD_SENDER` 原为 OnceLock，server 重启后重连无法重新注入 sender，回传静默失效。已改 `RwLock` 可重注入。**注意：部署新版 agent 二进制后此修复才在生产生效**（当前生产 agent 仍是旧二进制，server 重启后仍需滚动重启 agents）。

## 5. 操作手册

```bash
# 触发全量导出（服务器本机；run_id 由 server 生成，从响应取）
WRITE=$(awk '/Write Token/{getline; gsub(/^[ \t]+/,""); print; exit}' \
  /home/admin/Cyber-Jianghu/crates/server/logs/cyber_jianghu_admin.tmp)
curl -s -X POST http://localhost:23333/api/v1/training/export \
  -H "Authorization: Bearer $WRITE" -H "Content-Type: application/json" -d '{"force_full":true}'
# 查询/下载
curl -s "http://localhost:23333/api/v1/training/exports/<run_id>" -H "Authorization: Bearer $WRITE"
scp '47.102.120.116:/home/admin/Cyber-Jianghu/crates/server/data/training_exports/sft/run=<id>.jsonl' ./training_data/

# 天魂标签分布（本地，输入为解包后的 agent trace 树或归集目录）
python3 scripts/parse_tianhun_labels.py <extracted_root> <output_dir>

# 部署（本地 Mac 交叉编译 → 服务器；会全量同步 config/ 与 compose）
SERVER=root@47.102.120.116 REMOTE_PROJECT=/home/admin/Cyber-Jianghu \
  bash scripts/deploy/ship-server-binary.sh
# 部署后：若只动了 server，滚动重启 agents 一次（旧版 agent 有 OnceLock 问题；新版修复后可省）
```

## 6. 训练交接：MLX LoRA（本机 0.6B 起步）

结论：**0.6B 在本机 M 系列芯片上训练完全可行**（模型 1.2GB bf16，16GB 内存即可，全程数小时级）。把它当作廉价的首期实验而非终态：预期格式（JSON 合法率）会很快达标，决策质量是验收关键，不达标按 0.6B → 1.7B → 4B 阶梯上移（1.7B 仍 16GB 可跑，4B 建议 ≥32GB 或上量化）。

### 6.1 数据准备

```bash
scp '47.102.120.116:/home/admin/Cyber-Jianghu/crates/server/data/training_exports/sft/run=01M3HT7J8R6JEEWZYHN42KHB1Z.jsonl' ./training_data/
python3 scripts/prepare_sft_mlx.py training_data/run=01M3HT7J8R6JEEWZYHN42KHB1Z.jsonl ./sft_data
# 产出 sft_data/train.jsonl（95%）+ valid.jsonl（5%），纯 {"messages":[...]} 格式
```

### 6.2 训练与推理（mlx-lm）

```bash
pip install mlx-lm

python -m mlx_lm lora \
  --model mlx-community/Qwen3-0.6B-bf16 \
  --train ./sft_data/train.jsonl --valid ./sft_data/valid.jsonl \
  --fine-tune-type lora --num-layers 12 \
  --batch-size 4 --iters 3000 --learning-rate 1e-4 \
  --steps-per-eval 200 --adapter-path ./adapters/qwen3-06b-sft-v1
# 具体参数名随 mlx-lm 版本可能微调，以 python -m mlx_lm lora --help 为准

# 起本地 OpenAI 兼容服务（可直接被 agent 的 llm 配置消费，provider: ollama/openai_compatible）
python -m mlx_lm server --model mlx-community/Qwen3-0.6B-bf16 \
  --adapter ./adapters/qwen3-06b-sft-v1 --port 8080
```

### 6.3 验收指标（held-out 5%，与底座 zero-shot 对照）

| 指标 | 含义 | 判定 |
| --- | --- | --- |
| JSON 合法率 | assistant 输出可 `json.loads` 且含 `actions` | 硬门槛 ≥95%，不达标先加 epoch |
| 动作类型合法率 | `action_type` ∈ actions.yaml 动作集 | 硬门槛 ≥90% |
| 实体一致率 | `action_data` 目标 ID 与 held-out 教师输出一致 | 首期只测基线，观测后定阈值；持续偏低即上移 1.7B |

评测脚本对齐 `scripts/parse_tianhun_labels.py` 的解析口径。对照基线务必跑：同批 held-out 上未微调底座的同三项指标（呼应 NanoJev 卡的 Untuned Qwen 对照法）。

## 7. 遗留风险

- 生产 agent 二进制尚未包含 OnceLock 修复，下次部署 agent 时携带（`push_server.sh` / agent 镜像构建流程）。
- 服务器磁盘 40G 已用 ~17G；trace LRU 上限单 agent 1GB，归集后的 server 汇聚树无 LRU（`enforce_max_size` 只作用于 agent 本地），长期需关注。
- 导出样本中部分 user prompt 含未替换的 `{summary_context}` 占位符（训练/推理分布一致，无害，但建议排查 prompt 模板构建）。
- 语义重复占天魂驳回 64%，本地 embedding 预筛是候选优化，但属过渡期方案，待训练验证出结果后按实测 token 账单再议。
