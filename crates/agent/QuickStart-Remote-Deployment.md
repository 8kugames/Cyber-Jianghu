# Agent 远程部署快速开始（多实例，每用户一实例）

> 场景：agent 不跑在用户设备上，而是部署在服务器上，客户端（Godot 桌面/手机端）通过远程
> HTTP API + SSE 接入。适用于手机端（设备无法运行 Rust agent）与评审组远程体验。

## 部署原则

- **每用户一个 agent 实例**。一个实例 = 一个设备身份 = 独立 token = 独立记忆库，
  实例内可通过 `characters/switch` 管理多个角色（同一时刻一个活跃）。
- 禁止多用户共享同一实例：共享即共享大脑（决策互相污染）与凭据。
- 多实例不是 docker 专属能力：agent 二进制原生支持同机多进程（独立
  `CYBER_JIANGHU_CONFIG_DIR` + 不同 `--port`），docker 只是让隔离省事。
- 同机共存注意：本 compose 的 embedding 服务容器名/卷名与主开发栈全同
  （`cyber-jianghu-embedding` / `cyber-jianghu-embedding-data`），与主栈同机部署
  会冲突；需改名或停用其一（独立远程服务器通常无主栈，不受影响）。

## 部署步骤

前置：服务器已安装 Docker，游戏服务器（23333）已运行且可达。

```bash
# 1. 准备实例配置。agent 启动强依赖 agent.yaml（缺失会直接退出，不会自动生成
#    设备身份），必须从模板预置：
mkdir -p instances/agent-1/config instances/agent-2/config
cp ../config/agent.yaml.example instances/agent-1/config/agent.yaml
cp ../config/agent.yaml.example instances/agent-2/config/agent.yaml
#    逐实例编辑 agent.yaml：
#    - server.ws_url / server.http_url 指向游戏服务器
#    - llm 段填 provider / api_key / model（cognitive 模式必需）
#    - runtime.mode 无需修改：启动命令未带 --mode 时 CLI 默认 cognitive

# 2. 导出每实例访问 token（openssl rand -hex 32 生成，逐实例不同）
export CJ_AGENT1_TOKEN=<token-1>
export CJ_AGENT2_TOKEN=<token-2>

# 3. 全新服务器需先创建外部网络（与游戏服务器 compose 共用）
docker network create cyber-jianghu-network

# 4. 启动（在仓库的 crates/agent/ 目录下执行）
docker compose -f docker-compose.remote.yml up -d

# 5. 验证（服务器本机）
curl http://127.0.0.1:23340/api/v1/health
curl http://127.0.0.1:23341/api/v1/health
```

在仓库根目录的 `crates/agent/` 下执行（compose 的 build context 为 `../..`）。
新增实例：复制一个 agent 服务块，改容器名、端口映射、config 目录、token 环境变量名
与数据卷名。

注意：`CYBER_JIANGHU_SERVER_WS_URL` / `CYBER_JIANGHU_SERVER_HTTP_URL` 环境变量不被
agent 代码消费（存量遗留），游戏服务器地址只认 `agent.yaml` 的 `server.ws_url` /
`server.http_url` 或 CLI `--server` 参数。

## 访问链接与凭据交付

给每个使用者的接入信息四元组：

| 项          | 值                             | 说明                                              |
| ----------- | ------------------------------ | ------------------------------------------------- |
| base_url    | `http://<服务器IP>:23340`      | 实例的 HTTP API 与面板根地址                      |
| token       | 该实例的 `CJ_AGENTx_TOKEN`     | REST 用 `Authorization: Bearer`，SSE 用 `?token=` |
| ws (server) | `ws://<游戏服务器IP>:23333/ws` | agent 已代连，客户端通常无需直连 server           |

交付方式：带外渠道（二维码 / 深链 / 私信）。客户端（Godot）侧经
`CYBER_JIANGHU_AGENT_TOKEN` 环境变量注入 token 接入（当前 v1 行为；连接预设中的
per-preset 独立 token 为占位能力，尚未生效）。

## 安全模型（重要）

- **`/api/v1/setup/status` 仅对 loopback 对端返回 device auth_token**
  （`setup_status_handler` 按 socket 对端地址判定，对端信息缺失一律 fail-closed）。
  远程访问者无法经此端点获取 token，token 必须带外交付。
- **agent-web 面板的远程与容器化访问限制**：面板首次加载依赖 setup/status 自动取
  token，以下两种形态都拿不到 token，且当前面板无手动输入 token 的入口（后续版本
  补充）：
  1. 远程浏览器访问（对端是远程主机，非 loopback）；
  2. Docker 端口发布下的宿主浏览器访问（端口发布经 docker-proxy/DNAT，容器内看到的
     对端是网桥网关 IP 而非 loopback；可用
     `curl -s http://127.0.0.1:23340/api/v1/setup/status | grep -c auth_token`
     在你的部署形态上实测，输出 0 即受此限制）。
     因此容器化实例的 LLM 配置一律走 `instances/agent-N/config/agent.yaml` 编辑 +
     重启实例；面板操作仅限原生（非容器）部署的本机浏览器。
- **反向代理注意**：代理与 agent 同机时，agent 看到的对端是代理（loopback），
  loopback 判定会失真。同机反代必须在代理层封禁引导端点（nginx 示例）：

  ```nginx
  location /api/v1/setup/ { return 403; }
  ```

- **TLS**：内测期可用明文 HTTP（Android 可用，iOS 需 ATS 例外配置）；
  正式上架前必须上 TLS 反代（iOS ATS 默认拒绝明文 HTTP，应用商店审核同样要求）。
- **凭据强度**：token 为 64 位 hex 随机串；泄露后更换环境变量并重建实例容器即可。

## 运维

- 日志：`docker compose -f docker-compose.remote.yml logs -f agent-1`
- 重启实例：`docker compose -f docker-compose.remote.yml restart agent-1`
- 彻底重置某实例（清除设备身份与记忆）：停止实例后删除
  `instances/agent-N/config/` 与对应数据卷，再重新预置 agent.yaml 并启动（视为新设备）。
