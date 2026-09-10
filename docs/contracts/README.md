# 主仓对外契约文档(P1-P5)

本目录存放 client 仓(`8kugames/cyber-jianghu-client`)消费的最小契约 JSON Schema 片段。
对应的跨仓协调追踪器为 client 仓 `docs/coordination/main-repo-prereqs.md`。

## 真源规则

- **结构真源是 Rust 类型/端点实现**;本目录 schema 是"片段式"契约快照(顶层字段强类型,
  深层实体松类型),不追求与 Rust 结构逐字段等价。
- 修改 wire 结构时须同步更新对应 schema,并按规则 bump
  `crates/protocol/src/lib.rs::PROTOCOL_VERSION`:
  - 不兼容变更(删字段/改类型/改语义) → major
  - 新增可选字段/端点 → minor
  - 无契约影响的修复 → patch
- client 启动握手按 major 号判断兼容(major 不一致即拒绝连接)。

## 文件清单

| 文件 | 覆盖内容 | 实现真源 |
|---|---|---|
| `world_state.schema.json` | Server→Agent 原始 WorldState wire 结构 | `crates/protocol/src/types/world.rs::WorldState` |
| `intent.schema.json` | Agent→Server Intent wire 结构 | `crates/protocol/src/types/actions.rs::Intent` |
| `version.schema.json` | `GET /api/v1/version` 响应(agent 与 server 两侧) | `crates/agent/src/infra/api/handlers/basic.rs::version_handler` + `crates/server/src/handlers/system.rs::version` |
| `state_stream.schema.json` | `GET /api/v1/state/stream` SSE 三事件(connected/state/heartbeat) | `crates/agent/src/infra/api/handlers/state_stream.rs` |

## 鉴权

Agent HTTP API 接受两种 token(并集):

- 静态 token:env `CYBER_JIANGHU_AGENT_TOKEN`(外部 client 联调,设备注册前即可用)
- 设备 token:设备向 server 注册时下发的 `device_config.auth_token`(本地面板)

公开端点(无需认证):`/api/v1/health`、`/api/v1/version`、`/api/v1`、`/`、静态资源。
SSE 端点(`/api/v1/events`、`/api/v1/state/stream`)额外接受 `?token=<token>` query 参数。

## 校验方法

```bash
python3 - <<'EOF'
import json, jsonschema
for name in ["world_state", "intent", "version", "state_stream"]:
    schema = json.load(open(f"docs/contracts/{name}.schema.json"))
    jsonschema.Draft7Validator.check_schema(schema)
    print(f"{name}: OK")
EOF
```
