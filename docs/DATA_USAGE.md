# 用户数据使用说明

> 本文档明示《虚境：江湖》如何采集、处理、使用玩家产生的数据，以及你拥有的选择权。

---

## 1. 为什么采集数据

本游戏运行着大量由 AI 驱动的智能体（角色）。这些智能体每时每刻都在调用大语言模型（LLM）进行"思考与决策"。这些决策过程蕴含着宝贵的训练信号——什么决策让角色活得更久、什么决策被世界观审查驳回。

我们采集这些数据，用于训练**更适合本武侠世界观的专用模型**，目标是：

- 降低推理成本（用专用小模型替代通用大模型的部分调用）
- 提升角色扮演的契合度（专用模型更懂江湖规则）

---

## 2. 采集什么

### 2.1 采集的核心数据：智能体的 LLM 调用记录

| 数据                    | 内容                                                     | 用途               |
| ----------------------- | -------------------------------------------------------- | ------------------ |
| **人魂调用**（renhun）  | 智能体推理时的 prompt + response（"角色如何思考与决策"） | SFT/DPO 训练主样本 |
| **天魂调用**（tianhun） | 世界观审查的 prompt + response（"决策是否合规"）         | 训练分类标签       |

每条记录包含：智能体 ID、tick 时间戳、灵魂阶段、角色设定（persona_name/description，~200 bytes）、prompt 全文、response 全文、attempt 序号、所用模型、trace 产生时间（wall_clock）。

> system_prompt 的静态模板部分（survival_rules/narrative/output_format，~15KB）不在 trace 中重复记录——训练时从项目配置（prompt_templates.yaml）复用。trace 只存 agent 特有的 persona 字段。

**不采集 / 不进入训练数据**：

- **真实身份**：本项目不采集（无邮箱/手机号/第三方账号登录）
- **设备 ID**：服务端生成的随机 UUID（非硬件指纹），仅用于设备-角色绑定与认证，保存在 server 数据库，不写入 trace、不随 SFT 导出

**reward 数据**：仅记录物理事实（寿数、生死），不含主观输入（声望/关系/心情等主观认知不作为 reward 信号）。

> **关联边界**：训练样本 metadata 携带 agent_id（角色化名标识），server 数据库中存在 agent→device 映射。导出数据本身不含设备 ID；但同时持有导出数据与 server 数据库的一方可通过 agent_id 建立关联。
>
> trace 原文记录 prompt/response 以最大化训练数据质量。角色行为由 LLM 驱动，但 prompt 中会包含玩家填写的角色设定与托梦文本（见第 3、4 节）。

---

## 3. 数据如何记录（原文保留）

trace 记录的 prompt/response 均为**原文保留**——这是为了训练数据质量最大化（脱敏会丢失有意义的上下文信息，损害样本质量）。角色行为由 LLM 驱动，但 prompt 中会包含玩家填写的角色设定与托梦文本，这些玩家手写内容同样原文保留。

| 数据类型                                          | 记录方式     |
| ------------------------------------------------- | ------------ |
| 智能体推理 prompt（含角色设定/托梦/对话等上下文） | **原文记录** |
| 智能体决策 response                               | **原文记录** |
| 角色名/ID                                         | **原文记录** |

> 说明：本项目不做脱敏处理。角色行为由 LLM 驱动，但玩家填写的角色设定与托梦文本会作为 prompt 上下文原文保留；如需避免，请关闭采集（5.1）或关闭回传（5.2）。trace 用于训练更契合本武侠世界观的专用模型。

---

## 4. 数据流向

```
玩家游戏行为
  ├── 角色设定/托梦/私聊 → 进入智能体决策上下文（原文）
  └── 智能体 LLM 调用 → trace::record（原文保留）
                          ↓
                    本地落盘（agent 端 traces/）
                          ↓ upload.enabled=true
                    回传 server（原文）
                          ↓
                    server 汇聚（traces/ + rewards/）
                          ↓
                    离线导出（SFT/DPO 脚本）
                          ↓
                    训练专用模型
```

---

## 5. 你的选择权（Opt-out）

### 5.1 完全退出采集

在 `$CYBER_JIANGHU_CONFIG_DIR/trace.yaml`（默认 `~/.cyber-jianghu/config/trace.yaml`）中：

```yaml
output:
  enabled: false # 完全不采集（零开销，不影响游戏）
```

### 5.2 仅本地保留（不回传 server）

```yaml
output:
  enabled: true # 仍本地采集
upload:
  enabled: false # 但不回传 server
```

### 5.3 无脱敏选项

本项目不做脱敏（角色行为由 LLM 驱动；但玩家手写的角色设定与托梦文本会随 prompt 原文保留，如需避免请使用 5.1/5.2 关闭）。配置项仅控制是否采集/回传：

```yaml
output:
  enabled: true # 是否采集 trace（默认开）
upload:
  enabled: true # 是否回传 server（默认开）
```

---

## 6. 数据存储与保留

- **本地存储**（agent 端）：`<data_dir>/traces/soul=<stage>/agent=<id>/date=<YYYY-MM-DD>.jsonl`，按日期分区。
- **server 存储**：回传后汇聚到 server 的 `traces/` 目录。
- **保留策略**：自动滚动覆盖。`max_size_mb` 控制总体积上限（默认 1024 MB=1G），超过则按 LRU 删除最旧文件。

---

## 7. 配置项完整说明

| 配置项               | 默认值     | 说明                                          |
| -------------------- | ---------- | --------------------------------------------- |
| `output.enabled`     | `true`     | 是否采集 trace                                |
| `output.base_dir`    | `"traces"` | 本地输出目录                                  |
| `output.max_size_mb` | `1024`     | 日志总体积上限（MB），超限按 LRU 删除最旧文件 |
| `upload.enabled`     | `true`     | 是否回传 server                               |
| `upload.batch_size`  | `32`       | 每批回传条数                                  |
