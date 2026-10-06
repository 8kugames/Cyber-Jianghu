# 决策模型（玩家侧 2B 意图决策）运行手册

两段式架构：人魂 LLM 只产认知（不写 actions JSON），2B 决策模型把"认知摘要 + 世界状态"
转成结构化意图（act1 动作选择 + 实体绑定），之后照常走天魂四层审查。置信度门控
（act1 conf >= threshold）不过或任何环节失败，整体回退既有"人魂 LLM 写完整 JSON + 重试"
路径——回退路径即历史生产行为，因此本特性零风险灰度。

## 一、数据流

```
WorldState + 记忆上下文
   │
   ├─ [1] 人魂认知-only LLM（think_cognition_only；output_format 段去掉 actions 要求）
   │       产出 self_status / environment / key_observations / primary_drive /
   │       drive_intensity / thought_process / should_remember / constructed_emotion
   │
   ├─ [2] 决策状态文本 = system 消息 + tick 消息 + 认知摘要块
   │       （与训练数据 prepare_decision_sft.py 的 "system persona + tick user + 认知块"
   │         拼装逐字一致——这是模型训练时的输入分布，勿改格式）
   │
   ├─ [3] 决策模型逐问题读出（一次一问，letter logits 直读，零生成）：
   │       act1（12 动作选 1）→ act2（12+「无」）→ 实体绑定（按 act1 语义需要）：
   │         取/用/吃/喝/予 → item1（物品候选：背包/附近/可采集，上限 25 + 「无」）
   │         攻击/教导/说话/观察 → agent1（附近人 8 位短 ID）
   │         移动 → loc1（相邻地点名 + node_id 双形态）
   │
   ├─ [4] 门控与绑定：
   │         act1 ∈ {休整,移动,吃,喝,用,取,观察,攻击} 且 conf >= threshold；
   │         act2 仅采纳「无/休整/观察」（其余动作缺实体绑定，直接丢弃更稳）；
   │         绑定失败（攻击答「无」、取 绑背包物品等）→ 整体回退。
   │
   └─ [5] 构造 Intents（thought_log 标注 来源=decision_model conf=x.xxx）
           → 认知链 4 阶段补全 → CognitiveValidator 校验
           → 天魂 validate_with_reflector 四层（零改动）
```

说话/予/教导/制造 不走决策路径（content/recipient_id/recipe_id 无法由决策问题绑定，
act1 命中即回退 LLM 路径补写完整 JSON）。

## 二、模型资产与分发

官方发布仓（双源同构，解析器对两种 manifest schema 均兼容）：

- ModelScope 主源：https://www.modelscope.cn/models/8kugames/Cyber-Jianghu-Decision-2B
- GitHub 备源：https://github.com/8kugames/Cyber-Jianghu-Decision-2B（Release tag `model-v1`）

版本目录结构（v1 发布 q5_k_m / q4_k_s 两档位；q8_0/q6_k 未发布，
配置指定会得到"清单中无该档位"的干净报错并回退 LLM 路径）：

```
manifest.json                                        # {"version": "model-v1", "files": [...]}
decision_config.json                                 # letter_token_ids=[32..57] + 校准温度 1.3742
Cyber-Jianghu-Decision-2B-Q5_K_M.gguf                # 1.41GB
Cyber-Jianghu-Decision-2B-Q4_K_S.gguf                # 1.21GB
llama-server-b11408/                                 # llama.cpp b11408 CPU 构建运行时（按构建号分目录）
  llama-server-macos-arm64.tar.gz                    #   平铺 launcher + 共享库，各 11-20MB
  llama-server-macos-x86_64.tar.gz                   #   Linux 为 glibc 构建，musl/Alpine 请用
  llama-server-linux-x86_64.tar.gz                   #   系统包管理器安装并在 llama_server_path 指定
  llama-server-linux-arm64.tar.gz
  llama-server-windows-x86_64.tar.gz
```

**推理运行时自动装配**：agent 安装模型时按当前平台（`macos-arm64`/`linux-x86_64`/
`windows-x86_64` 等）自动下载 llama-server 归档，sha256 校验后解压到版本目录并补
可执行位，启动时注入 `DYLD_LIBRARY_PATH`/`LD_LIBRARY_PATH` 兜底共享库解析——
玩家零手动步骤。清单缺本平台条目（旧 manifest / 未覆盖平台）时告警降级：
模型照常安装，运行时改用 `llama_server_path` / 安装目录 / 可执行文件同级 /
PATH 中的 llama-server，全部缺失才回退 LLM 决策路径。

manifest.json schema（`size`/`bytes` 两种键名均可；`kind`/`quant` 可省略，
按文件名推断；`model`/`display_name`/`license`/`base_model` 等额外字段忽略）：

```json
{
  "version": "model-v1",
  "files": [
    { "name": "decision_config.json", "bytes": 476, "sha256": "<64hex>" },
    {
      "name": "Cyber-Jianghu-Decision-2B-Q5_K_M.gguf",
      "bytes": 1411120576,
      "sha256": "<64hex>"
    }
  ]
}
```

- 下载顺序：ModelScope `https://www.modelscope.cn/models/{repo}/resolve/master/{file}` 主源，
  GitHub `{base}/latest/download/{file}` 备源；HTTP Range 断点续传；sha256 终验，
  不符删档换源。
- 实测（2026-10-05 发布 v1）：ModelScope 对 Range 返回 200（不支持续传，下载器静默
  全量重下，路径有测试覆盖）；GitHub Release 资产为 S3 后端，支持 206 真续传。
  GitHub 侧四资产已通过 Release API 的 sha256 digest 逐一核对。
- 安装目录：`{install_dir}/{version}/`（默认 `~/.cyber-jianghu/decision-model/`），
  `current.json` 记录当前版本；升级 = 下载新版本目录后原子改写指针，旧目录保留供回滚
  （把 current.json 的 version 改回旧值即可）。
- 低配自动降档：系统可用内存（Linux 读 MemAvailable 并叠加 cgroup v2 限额）
  低于 `low_memory_threshold_mb` 时自动改用 q4_k_s。
- llama-server 二进制不由本管线分发；配置 `llama_server_path` 或放入安装目录 / agent
  可执行文件同级 / PATH。
- letter token 映射运行时校验：启动 llama-server 后用 /tokenize 对探针 prompt 逐字母
  验证「追加字母恰为单 token 且 id 与 decision_config.json 一致」，不符即判运行时不可用
  （回退 LLM 路径）。

## 二点五、部署模式（本地自部署 vs 远程 URL）

- `mode: local`（本地 agent 缺省）：下载模型 + 自启动 llama-server，全流程零手工。
- `mode: remote`（docker 部署唯一允许）：通过 `remote_url` 访问已部署的决策模型端点
  （自架 llama-server 服务项目专用 GGUF，或第三方决策 API；协议为 llama-server 兼容，
  启动时逐字母校验 letter token 映射，不兼容端点自动回退 LLM 路径）。
  `remote_api_key` 可选（端点设置了 --api-key 时使用）。
- 容器内（检测 /.dockerenv 等）缺省 remote 且未配置 URL 时，自动使用 docker 网络
  发现地址 `http://decision-model:8081`（compose 服务名约定）。**容器内禁止 local
  模式**——不在每个 agent 容器重复下载与自部署，集中一个端点服务全集群。
- 端点部署示例（compose 片段，宿主机需 >=4GB 内存）：

```yaml
services:
  decision-model:
    image: ghcr.io/ggml-org/llama.cpp:server
    command:
      [
        "-m",
        "/models/Cyber-Jianghu-Decision-2B-Q5_K_M.gguf",
        "--host",
        "0.0.0.0",
        "--port",
        "8081",
        "-c",
        "16384",
        "--parallel",
        "8",
      ]
    volumes:
      - ./models:/models
    # agent 侧默认经 docker 网络以 http://decision-model:8081 访问
```

- POST /api/v1/decision-model/config 新字段三态语义：
  `mode` 缺省/null = 回到环境自动（容器 remote / 本地 local）；
  `remote_url` 缺省/空串 = 清除已存 URL；`remote_api_key` 缺省 = 保持已存值（留空即可）；
  `remote_model` 缺省/空串 = 清除路由名。

### 多模型网关（llama.app / llama-swap）

端点为多模型代理时（如 macOS 的 Llama.app `serve`、llama-swap），必须在请求中携带
`model` 字段路由到决策模型，否则会打到当前活跃模型。配置：

```yaml
decision_model:
  mode: remote
  remote_url: http://localhost:9931 # Llama.app serve 监听地址
  remote_model: cyber-jianghu/decision-2b # 网关 preset id，需与 models.user.ini 段名一致
```

Llama.app 自定义模型登记在 `~/.config/llama/models.user.ini`（重启 app 生效）：

```ini
[cyber-jianghu/decision-2b]
model = /path/to/Cyber-Jianghu-Decision-2B-Q5_K_M.gguf
ctx-size = 4096
```

行为要点：`/completion`、`/tokenize` 携带 model 字段路由；`/health` 不带（网关全局存活
信号）。首次请求触发模型加载（冷启动秒级）；`--sleep-idle-seconds` 空闲卸载后下次
决策自动重载；`--models-max 1` 下其他模型被拉起会挤掉决策模型，但 model 路由保证
不会把决策 prompt 发错模型。letter token 映射由启动时 /tokenize 逐字母校验兜底，
网关侧 llama.cpp 版本与训练不一致时自动回退 LLM 路径。

## 三、配置（agent.yaml）

```yaml
decision_model:
  enabled: true # 总开关，默认开启；用户可主动关闭（false = 完全走既有路径）
  modelscope_repo: "8kugames/Cyber-Jianghu-Decision-2B" # 主源，默认官方仓
  github_release_url: "https://github.com/8kugames/Cyber-Jianghu-Decision-2B/releases" # 备源，置空串禁用
  quant: q5_k_m # q8_0/q6_k/q5_k_m/q4_k_s
  threshold: 0.70 # 置信度门控
  timeout_ms: 30000 # 单问题调用超时
  install_dir: null # 默认数据目录下 decision-model/
  low_memory_threshold_mb: 6144
  llama_server_path: null
  port: 0 # 0 = 自动空闲端口
  llama_server_args: [] # 如 ["-ngl", "99"]
  startup_timeout_ms: 180000
  # remote 模式追加（mode: remote 时）：
  # remote_url: http://decision-model:8081
  # remote_api_key: sk-xxx
  # remote_model: my-gateway/decision-2b  # 多模型网关 preset id（单模型端点留空）
```

配置之外的正确性前提：`server` 侧 `actions.yaml` 动作词表需与训练口径一致（12 动作，
选项顺序以代码内 ACTION_ORDER 为准）；新增动作会自动追加到选项表尾（分布外，谨慎）。

面板/HTTP 写入口（agent 内置 Web 面板「设置 → 决策模型」卡片走同一通道）：

- `POST /api/v1/decision-model/config`：全量保存 `{enabled, quant, threshold, timeout_ms}`，
  持久化 decision_model 段 + 热换装 manager（下一 tick 生效，无需重启）；`enabled=true`
  与启动装配同口径要求至少一个下载源；timeout 限 1000-120000ms；停用时置空槽位
  （旧 manager drop 时 llama-server 由 kill_on_drop 回收）。
- `POST /api/v1/decision-model/install`：手动触发下载/修复（幂等；已就绪不重下），
  进度经 events SSE 推送。
- 连续更换 quant 档位请等上一轮安装完成：新旧 manager 的安装任务共享安装目录，
  并发下载可能损坏半写文件（失败自动 Failed，可重试自愈）。

## 四、集成验证 runbook（发版前执行）

```bash
# 1. 本地起 llama-server（Q8_0 精度，与离线评测同口径）
llama-server -m decision-2b-Q8_0.gguf -c 16384 --port 8091

# 2. 352 条 held-out 离线复测（对照 tmp/eval_ours2b_B.json 基线：
#    act1_top1 94.0% / top3 98.9% / act2 81.3%）
.venv/bin/python tmp/eval_startlux_decision.py \
  --backend http --http-url http://127.0.0.1:8090/v1/systemone \
  --sequential --with-cognition --out tmp/eval_ours2b_B_recheck.json

# 3. agent 侧真实 tick 冒烟（decision_model.enabled=true + llama_server_path 指向本地）
#    观察：
#    - 日志 [decision_model] 采用决策输出 / 门控未通过 / 回退
#    - GET /api/v1/metrics        -> decision_model 小节（take_rate / 置信度直方图 / 耗时）
#    - GET /api/v1/decision-model/status
#    - GET /api/v1/decision-model/events （SSE 下载进度）
#    - 面板意图 thought_log 出现（来源=decision_model conf=…）标注，
#      天魂四层照常出裁决（layer0..layer3）

# 4. 回退演练（必须项）：
#    - 停掉 llama-server → 决策调用失败 → 日志回退 + metrics.fallback_error 增长，
#      agent 行为恢复既有 LLM 路径，不阻塞主循环
#    - decision_model.enabled=false → 完全走既有路径，零差异
#
# 5. 面板写入口冒烟（POST，需 Bearer token）：
#    POST /api/v1/decision-model/config {enabled,quant,threshold,timeout_ms}
#      → 观察：yaml 落盘、日志 [decision_model] 面板热换装、下一决策 tick 生效；
#        enabled=false → 槽位置空、llama-server 退出、决策走既有路径
#    POST /api/v1/decision-model/install → Failed/NotInstalled 后重试，SSE 跟随进度
```

## 五、兜底与可观测

| 故障                                | 行为                                           |
| ----------------------------------- | ---------------------------------------------- |
| 模型未安装 / 下载源未配置           | 后台下载不阻塞启动；决策路径直接回退 LLM       |
| 下载 sha256 不符                    | 删档换源重试，全部失败记 Failed 状态           |
| llama-server 缺失 / 启动超时 / 崩溃 | 决策路径回退 LLM，下次决策重新尝试拉起         |
| 单问题超时（timeout_ms）            | 回退 LLM                                       |
| letter token 校验失败               | 判运行时不可用（tokenizer/模板漂移），回退 LLM |
| act1 conf < threshold               | 回退 LLM（metrics.fallback_low_confidence）    |
| act1 不可绑定动作 / 绑定失败        | 回退 LLM（metrics.fallback_ineligible）        |

计量（`GET /api/v1/metrics` 的 `decision_model` 小节）：决策 tick 数、采纳数、
take_rate、回退分类计数、单问题调用数/失败数/平均耗时、认知阶段平均耗时、
act1 置信度 10 桶直方图。决策模型不经过 LLM 场景路由（非 LLM API 调用），
token 记账不含其开销（本地推理）。

## 六、代码位置

| 模块                              | 路径                                                            |
| --------------------------------- | --------------------------------------------------------------- |
| 管理器/计量/状态                  | `crates/agent/src/component/decision_model/mod.rs`              |
| 资产清单 + sha256                 | `crates/agent/src/component/decision_model/manifest.rs`         |
| 双源下载器（断点续传）            | `crates/agent/src/component/decision_model/downloader.rs`       |
| 提示词渲染（jevfmt 移植）         | `crates/agent/src/component/decision_model/prompt.rs`           |
| llama-server 运行时 + letter 读出 | `crates/agent/src/component/decision_model/server.rs`           |
| 问题构造/候选/门控                | `crates/agent/src/component/decision_model/questions.rs`        |
| 认知-only 阶段                    | `crates/agent/src/soul/actor/engine/cognition.rs`               |
| 门控接线（回退即既有路径）        | `crates/agent/src/runtime/decision.rs`                          |
| 状态/进度端点                     | `crates/agent/src/infra/api/handlers/decision_model.rs`         |
| 配置段                            | `crates/agent/src/config/sub_configs.rs`（DecisionModelConfig） |
