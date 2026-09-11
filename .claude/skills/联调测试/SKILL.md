# 联调测试 SKILL v2 — 工具化执行 + LLM 分析

## 定位分工（先读）

| 层     | 负责方               | 内容                                                                          |
| ------ | -------------------- | ----------------------------------------------------------------------------- |
| 执行层 | `.test-agents/` 脚本 | 镜像构建（离线）、容器重启、角色归隐/注册/验证、监控采集、健康快照            |
| 分析层 | 本 SKILL + LLM       | 调用脚本 → 读摘要 → 异常处置（按异常手册）→ 死亡/恶性 Bug 判定 → Phase 4 报告 |

**禁止事项**：LLM 不得绕过脚本手工 curl 编排部署/注册流程（历史教训：端口漂移、缺认证、token 竞态、路径双前缀全部源于此）。脚本故障时修复脚本本身，保持工具链为唯一执行路径。

## 工具链

| 工具                                        | 用途                                                         | 典型调用                                                                                            |
| ------------------------------------------- | ------------------------------------------------------------ | --------------------------------------------------------------------------------------------------- |
| `.test-agents/restart.sh`                   | 部署一站式：离线构建 + 重启 + 归隐 + 注册新角色 + 验证       | `.test-agents/restart.sh --build --register`                                                        |
| `.test-agents/monitor-tools/monitor-24h.sh` | 24h 数据底座（10min/轮，写入 `.test-agents/logs/dev24h-*/`） | `nohup bash .test-agents/monitor-tools/monitor-24h.sh > .test-agents/logs/monitor-nohup.log 2>&1 &` |
| `.test-agents/monitor-tools/check-round.sh` | 单轮健康快照（抽查用，只观测）                               | `.test-agents/monitor-tools/check-round.sh 10`                                                      |

### restart.sh 语义

- `--build`：离线构建（本地基镜像通道，自动自举缺失的基镜像；无需外网；全量编译约 15-25 分钟）
- `--register`：server 端归隐旧角色（幂等）→ LLM 生成新角色（5 次重试）→ 注册 → 查询验证 → 汇总表
- `--no-register`：仅重启（保留现有角色）
- 定向模式：`restart.sh --register agent-3` 只处理单个 agent
- 退出码：0 = 全部成功；1 = 有失败（读汇总表定位）
- server 地址自动从 `docker-compose.yml` 解析（`CYBER_JIANGHU_SERVER_HTTP_URL`），换 server 零脚本改动

## 标准流程

### 全新轮次启动 / 代码变更后重启

```bash
.test-agents/restart.sh --build --register
```

预期输出：4 个 agent 依次 RETIRE → GEN → REG → DONE，汇总 `4 注册并验证`。
然后启动监控：

```bash
nohup bash .test-agents/monitor-tools/monitor-24h.sh > .test-agents/logs/monitor-nohup.log 2>&1 &
```

### 运行中抽查

```bash
.test-agents/monitor-tools/check-round.sh 10
```

读健康表：Hunger 持续下降属正常（生存压力）；重点看 status 非 alive、Sanity 骤降、错误日志新增模式。

### 停止监控

```bash
pkill -f monitor-24h.sh
```

## 异常处置手册

### 1. token mismatch / 401 瞬时拒绝

- **根源**：server 在 device verify/WS 重连时轮换 token；agent 内存值与 device.yaml 文件值不一致
- **规则**：一切调用 token 源 = `GET localhost:<port>/api/v1/setup/status` 的 `auth_token`（内存权威值）。device.yaml 文件 token 禁止用作调用凭证（已从脚本全部移除）
- **注意**：agent 本地 `has_character=false` 但 server 仍有活跃角色时，agent 侧 rebirth 会自拒（"无法读取角色状态"）——此时用 server 端 retire（restart.sh 已内置）

### 2. 注册返回 500 空体「服务器拒绝」

- **根源**：server 端「该设备已有活跃角色，请先归隐」约束（0.1.296+）或注册事务异常
- **处置**：先 server 端归隐再注册（restart.sh --register 已内置）；仍 500 则读 server 日志定位：
  ```bash
  ssh -o BatchMode=yes admin@47.102.120.116 'docker logs cyber-jianghu-server --since 30m 2>&1 | grep -iE "register|ERROR" | tail'
  ```

### 3. sensenova 429 风暴（自维持循环）

- **根源**：认知重试 + 生成重试消耗 RPM（429 请求也计数）→ 自我维持
- **配方**（已验证）：
  ```bash
  # 1) 静默（停掉该 agent 全部 LLM 流量，让 RPM 窗口清空）
  curl -X POST localhost:<port>/api/v1/config/llm-disabled -H "Authorization: Bearer <setup/status token>" -H "Content-Type: application/json" -d '{"llm_disabled": true}'
  # 2) 等 70s
  # 3) 单次 generate → register
  # 4) 恢复 {"llm_disabled": false}
  ```
- 注：MiniMax/agnes 限额模型同理；60s 冷却自愈机制（rate_limit 冷却 60s，其他 3600s）已内建于 agent

### 4. 生成 JSON 畸形（缺字段 / 非法 JSON）

- 已容忍：`key_observations`/`primary_drive` 缺失有默认值
- 未容忍：`action_type` 等意图本体字段（不可默认），依赖重试。偶发属模型质量波动，连续失败按 provider 维度上报

### 5. 死亡处理（Phase 3.4）

- 检测：check-round.sh 表格 status 非 alive，或 monitor 轮日志死亡事件
- 处置：记录（agent/tick/hunger/HP/死亡前行为）→ agent 配置 auto_rebirth 时等待自动重生；否则 rebirth → `restart.sh --register agent-N`（定向注册）→ 记入死亡事件表

### 6. 恶性 Bug 判定（Phase 3.5）

满足任一 → `pkill -f monitor-24h.sh; .test-agents/restart.sh`（停止监控与容器）→ 记录复现步骤 → 报告用户：

- 关键流程无法推进（注册链路死锁 / action 死循环）
- 数据损坏不可恢复
- \>50% agent 同时持续异常（瞬时风暴不算，看持续窗口）

### 7. 行为坍缩（决策分布熵，Phase 3/4 固定分析项）

- 数据：check-round.sh 的行为分布表（香农熵 H / 最大熵比 r = H/log2(k)），monitor 轮次的 `actions.csv` 提供时间序列
- 阈值：r < 0.3 坍缩疑似；0.3-0.6 收缩；> 0.6 健康（总决策数 ≥ 5 才有意义）
- **判读必须交叉生存状态**：
  - r 低 + hunger/thirst 紧迫（<40）+ 认知失败率高 → 行为坍缩真信号
  - r 低 + 需求满足（饱食安稳，如 hunger>90）+ 认知成功率高 → 合理惰性，非故障
    （实例：饱食的沈云鹤 18/19 决策均为观察）
- 注意双重来源：休整/观察 占比高可能是「认知失败兑底」也可能是「合理行为」，
  需与 Attempt failed 率交叉验证
- 已知根因案例：server config 与二进制词表版本错配 → 全员单动作坍缩
  （2026-09-10 实例：server 日志「该设备已有活跃角色」+「未知的动作类型」；
  修复 = ship 脚本补 config 同步）

### 8. 注册路径挂起（401 刷新后静默）

- 根因（已修复入库 fd661a5c）：`refresh_auth_token` 读锁 guard 被 shadow 后
  存活到函数尾，函数尾取写锁自死锁。若复发，检查该函数读锁作用域
- 运维层：重启容器让 boot verify 对齐 token 后立即操作（稳定窗口内 register 仅
  数十 ms）

## Phase 4 报告

数据源：`.test-agents/logs/dev24h-*/`（monitor 轮次 + 每 agent 原始 JSON）+ check-round 快照。

### 结构

1. §9.1 部署基线：git commit、镜像 tag、server version、agent 配置矩阵（各 agent.yaml 的 model/provider）
2. §9.2 运行时间线：启动/死亡/重生/干预事件
3. §9.3 角色生存统计：Hunger 生存曲线（check-round 轮次对比）、死亡原因分布
4. §9.4 Token 消耗：per agent/model 增量表（注意 token_cost_count.tmp 为累计值，需差分）
5. §9.5 异常与缺陷：按 agent/server/config 分层，附日志证据
6. §9.6 行为分布熵：actions.csv 时间序列 → 每 agent 决策分布熵曲线，
   标记 r<0.3 坍缩疑似窗口，并与生存状态/认知失败率交叉判读（见异常手册 §7）
7. §9.7 稳定性结论：认知成功率、429 频次、冷却自愈触发次数
8. §9.8 改进建议

### Token 表格式

```
| Agent | 模型 | calls | prompt | completion | failures | 增量说明 |
```

## 环境备忘

- 远程 server：`http://47.102.120.116:23333`（地址唯一来源 = `.test-agents/docker-compose.yml`）
- 远程日志/部署：`ssh admin@47.102.120.116`，容器名 `cyber-jianghu-server`，远端目录 `/home/admin/Cyber-Jianghu`
- 本机构建网络对 Docker Hub/BuildKit 不可靠：一律走 restart.sh --build 的离线通道
- 离线构建基镜像：`local-rust-trixie:builder` / `local-debian-slim:runtime`（脚本自动自举，勿手工删除）
- agent 数据目录：`.test-agents/agent-*/data/servers/<server_key>/`（server_key = host 点转横线 + -port）
- 推理型模型（MiniMax M2 系）agent 的 config/agent.yaml 必须含 `enable_streaming: true`（非流式会被网关 ~60s 切断）；该目录 gitignored，重建环境需重设
