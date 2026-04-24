# HTTP API 与管理后台

**级别**: P2 体验增强
**模块**: `crates/server`

## 1. 设计目标
提供给开发者和 GM（Game Master）的可视化管理和人工干预入口，方便监控集群状态、干预 Agent 行为以及管理生态经济。

## 2. 核心机制
### 2.1 HTTP API 端点
- `/admin/*` — 静态资源与管理面板入口。
- `/api/v1/agent/*` — 管理特定 Agent 的状态，以及配置 Vendor 补货规则。
- `/api/config/*` — 触发配置文件热重载。
- `/api/dashboard/chronicles` — 查询世界传记。
- `/health` — 提供给 K8s/Docker 的节点存活与 Tick 周期探针。

### 2.2 Admin Web Dashboard (前端 UI)
- 基于静态多页应用构建（存放在 `crates/server/static/admin`）。
- **细粒度权限控制**：通过 Read/Write Token 进行拦截，防止越权操作。
- **大盘监控**：实时展示 Tick 流转帧率、Agent 在线分布热力图。
- **生态自动补货管理 (Eco-Physics)**：可视化设置 NPC 商人自动补货的 `threshold` 和 `budget_ratio` 预算，管理经济闭环。
- **LLM 面板**：支持在线测试 Ollama/OpenAI 兼容接口并进行全局切换。

## 3. 架构约束
- 后台界面的所有的管理干预（如发放物品）最终必须转化为标准的 `Intent` 或 `State` 变更调用底层的 Processor，禁止直接改写数据库字段破坏一致性。

## 4. 代码入口
- HTTP 路由与鉴权: `crates/server/src/handlers.rs`
- 静态页面: `crates/server/static/admin/`
