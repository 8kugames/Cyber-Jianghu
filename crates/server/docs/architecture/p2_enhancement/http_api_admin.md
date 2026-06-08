# HTTP API 与管理后台

**级别**: P2 体验增强
**模块**: `crates/server`

## 1. 设计目标
提供给开发者和 GM（Game Master）的 RESTful API 入口与可视化管理面板，方便监控集群运行状态、干预游戏生态以及进行数据配置。

## 2. 核心机制
### 2.1 HTTP API 端点 (Handlers)
通过 `axum` 提供丰富的 API：
- `/admin/*` — 静态资源代理与管理面板入口页面。
- `/api/v1/agent/*` — 负责 Agent 注册、归隐、传记回传及管理员的物资注入。
- `/api/dashboard/*` — Dashboard 专属 API，包括全盘 Agent 监控、事件流、配置下发状态以及 Vendor 补货规则。
- `/api/config/*` — YAML/JSON 配置文件的热重载接口及内容编辑器。
- `/api/admin/*` — 后台鉴权和会话管理（基于 Token / Cookie）。
- `/health` — 提供给 Docker/K8s 的探针。

### 2.2 Admin Web Dashboard (前端 UI)
- 前端文件作为静态资源内嵌于 `crates/server/static/admin/` 目录。
- **细粒度权限控制**：通过 Server 配置的 `admin_read_token` 与 `admin_write_token` 进行读写分离拦截，防止越权操作。
- **大盘监控**：实时展示服务器性能指标、内存中的存活 Agent 数量与事件日志。
- **生态补货 (Vendor Management)**：可视化设置 NPC 商人自动补货的 `threshold`、`budget_ratio` 等规则，维持虚拟经济平衡。
- **YAML 编辑器**：在页面上直接修改 `actions.yaml` 或 `game_rules.yaml` 并执行热重载。

### 2.3 管理员干预机制
管理员的发放物品等操作（例如 `/api/v1/agent/grant-items`），不会直接绕过系统改写数据库的背包字段，而是向 `VendorPendingEvents`（共享的 AppState 结构）写入待处理事件。
随后的 `TickScheduler` 会在下一个 Tick 处理前，将这些事件注入并由正常的广播流程分发，保证状态的一致性与合法性。

## 3. 架构约束
- 管理后台的操作必须通过正规的鉴权中间件拦截验证。
- 前端页面应保持纯静态实现，所有的状态和数据由 API 接口提供。

## 4. 代码入口
- 路由注册与启动: `crates/server/src/main.rs`
- 接口处理逻辑: `crates/server/src/handlers/` (包含 `dashboard`, `agent.rs`, `config_editor.rs` 等)
- 静态页面存放: `crates/server/static/admin/`
