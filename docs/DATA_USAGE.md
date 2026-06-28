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

| 数据 | 内容 | 用途 |
|---|---|---|
| **人魂调用**（renhun） | 智能体推理时的 prompt + response（"角色如何思考与决策"） | SFT/DPO 训练主样本 |
| **天魂调用**（tianhun） | 世界观审查的 prompt + response（"决策是否合规"） | 训练分类标签 |

每条记录包含：智能体 ID、tick 时间戳、灵魂阶段、prompt 全文、response 全文、所用模型。

### 2.2 不直接采集的数据

- **设备 ID / 真实身份**：仅用于账号绑定，**不进入训练数据**
- **reward 数据**：是物理事实（寿数、生死），不含玩家主观输入，非隐私敏感

---

## 3. 玩家输入如何处理（脱敏机制）

玩家通过游戏输入的两类内容会进入智能体的决策上下文，进而可能出现在采集的 prompt 中。我们对这些内容做**脱敏处理**，确保原文不进入训练集：

| 玩家输入类型 | 出现位置 | 脱敏方式 | 脱敏后形态 |
|---|---|---|---|
| **角色设定**（姓名） | trace 的 character_name 字段 | 角色名 SHA256 哈希化 | `角色_a1b2c3d4` |
| **角色设定**（背景描述） | system prompt（当前留空） | 当前不脱敏（system_prompt 留空，描述不在 trace 中）；未来填充时补 | — |
| **托梦**（玩家意图注入） | user prompt（memory_context） | 原文占位化（保留哈希标识） | `[托梦内容已脱敏_a1b2c3d4]` |
| **玩家间私聊** | user prompt（dialogue_section） | 发言占位化、对方标识哈希化 | `[对话内容已脱敏]`、`玩家_a1b2c3d4` |

**关键设计**：
- 脱敏在**数据采集环节（agent 端）**完成，回传到 server 的已是脱敏后数据。
- 脱敏**不影响智能体的实际推理**——智能体看到的仍是原文，只有训练数据是脱敏版本。
- 哈希使用 SHA256（跨平台稳定），不可逆。

### 3.1 智能体自身产出（LLM response）

智能体的 response（决策结果）**不脱敏**——因为它不是玩家直接输入，是 AI 产出，是训练的核心目标数据。

---

## 4. 数据流向

```
玩家游戏行为
  ├── 角色设定/托梦/私聊 → 进入智能体决策上下文（原文）
  └── 智能体 LLM 调用 → trace::record（此时脱敏玩家输入）
                          ↓
                    本地落盘（agent 端 traces/）
                          ↓ upload.enabled=true
                    回传 server（已脱敏）
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
  enabled: false   # 完全不采集（零开销，不影响游戏）
```

### 5.2 仅本地保留（不回传 server）

```yaml
output:
  enabled: true    # 仍本地采集
upload:
  enabled: false   # 但不回传 server
```

### 5.3 关闭部分脱敏（不推荐）

```yaml
sanitize:
  enabled: true
  persona_name_hash: false       # 保留原始角色名（不脱敏）
  dream_content_mask: false      # 保留托梦原文
  dialogue_content_mask: false   # 保留私聊原文
```

> 默认配置：采集开启 + 回传开启 + 全部脱敏开启。这是为了训练数据规模化积累。

---

## 6. 数据存储与保留

- **本地存储**（agent 端）：`<data_dir>/traces/soul=<stage>/agent=<id>/date=<YYYY-MM-DD>.jsonl`，按日期分区。
- **server 存储**：回传后汇聚到 server 的 `traces/` 目录。
- **保留策略**：当前无自动清理机制（文件按日期分区，便于手动归档/删除）。长期运行需运维定期归档。

---

## 7. 配置项完整说明

| 配置项 | 默认值 | 说明 |
|---|---|---|
| `output.enabled` | `true` | 是否采集 trace |
| `output.base_dir` | `"traces"` | 本地输出目录 |
| `upload.enabled` | `true` | 是否回传 server |
| `upload.batch_size` | `32` | 每批回传条数 |
| `sanitize.enabled` | `true` | 脱敏总开关 |
| `sanitize.persona_name_hash` | `true` | 角色名哈希化 |
| `sanitize.persona_description_mask` | `true` | 角色描述占位化 |
| `sanitize.dream_content_mask` | `true` | 托梦占位化 |
| `sanitize.dialogue_content_mask` | `true` | 私聊占位化 |
