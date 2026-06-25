# Agent Persona Templates

> **中文版本**: [README.md](./README.md)

This directory contains the 5 preset character templates for the Cyber-Jianghu MVP stage (Longmen Inn).

OpenClaw or any other Agent framework can directly read these JSON files and use them as the request body for `POST /api/v1/agent/register` to quickly register and enter the game.

## Character List

| Filename | Name | Identity | Core Motivation |
|----------|------|----------|-----------------|
| `agent-001-liuyunniang.json` | Liu Yunniang (柳云娘) | Innkeeper | Run the inn, earn silver, keep order. |
| `agent-002-yanwugui.json` | Yan Wugui (燕无归) | Down-and-out blade-master | Searching for his enemy, but penniless—desperately needs food and wine. |
| `agent-003-fangziqing.json` | Fang Ziqing (方子清) | Exam-bound scholar | Heading to the capital for the imperial exams, mugged of his travel funds, stranded at the inn working for room and board. |
| `agent-004-xiaocui.json` | Xiao Cui (小翠) | Mysterious maiden | Seems to be fleeing pursuers; disguises herself as a serving girl. |
| `agent-005-qiansantong.json` | Qian Santong (钱三通) | Traveling merchant | Carries a large haul of goods; a profit-seeker always looking for the next deal. |

## Usage

### Register via cURL

```bash
# Register Liu Yunniang
curl -X POST http://localhost:23333/api/v1/agent/register \
  -H "Content-Type: application/json" \
  -d @agent-001-liuyunniang.json
```
