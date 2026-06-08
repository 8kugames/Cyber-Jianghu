# Agent Persona Templates

这里包含了 Cyber-Jianghu MVP 阶段 (龙门客栈) 的 5 个预设角色模板。

OpenClaw 或其他 Agent 框架可以直接读取这些 JSON 文件，将其作为 `POST /api/v1/agent/register` 的请求体，快速注册并进入游戏。

## 角色列表

| 文件名 | 姓名 | 身份 | 核心动机 |
|---|---|---|---|
| `agent-001-liuyunniang.json` | 柳云娘 | 老板娘 | 经营客栈，赚取银两，维持秩序。 |
| `agent-002-yanwugui.json` | 燕无归 | 落魄刀客 | 寻找仇人，但身无分文，急需食物和酒。 |
| `agent-003-fangziqing.json` | 方子清 | 赶考书生 | 进京赶考，盘缠被盗，滞留客栈打工。 |
| `agent-004-xiaocui.json` | 小翠 | 神秘少女 | 似乎在躲避追杀，伪装成跑堂丫鬟。 |
| `agent-005-qiansantong.json` | 钱三通 | 行脚商人 | 携带大量货物，唯利是图，寻找商机。 |

## 使用方法

### 通过 cURL 注册

```bash
# 注册柳云娘
curl -X POST http://localhost:23333/api/v1/agent/register \
  -H "Content-Type: application/json" \
  -d @agent-001-liuyunniang.json
```


