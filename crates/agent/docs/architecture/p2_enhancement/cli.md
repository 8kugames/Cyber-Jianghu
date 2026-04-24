# 命令行工具 (CLI)

**级别**: P2 体验增强
**模块**: `crates/agent`

## 1. 设计目标
提供便捷、标准的 Unix 风格运维手段，方便在终端快速启动、调试和管理成百上千个 Agent 实例。

## 2. 核心机制
### 2.1 子命令支持 (Subcommands)
基于 `clap` 框架构建，支持以下核心指令：
- `run`：启动 Agent 主进程，建立 WebSocket 连接并开始思考循环。
- `config`：检查并打印当前加载的环境变量和 YAML 配置路径。
- `create-character`：引导式创建一个新的角色配置文件。
- `show` / `reset`：查看或清空 Agent 本地的 SQLite 记忆库。

### 2.2 自动端口探测
- 每个 Agent 自身也暴露一组 HTTP API（用于控制台或集群调度）。
- 在大规模启动时，支持传入 `--port 0` 参数，由操作系统自动寻找可用端口，避免端口冲突，并在启动日志中输出绑定的端口号。

### 2.3 环境隔离
- 支持通过 `--env` 参数加载不同的 `.env` 配置文件（如 `.env.test`, `.env.prod`）。
- 确保不同环境下的数据库存储目录和 Server 地址互不干扰。

## 3. 架构约束
- 必须构建清晰的帮助文档 (`--help`) 和严格的参数类型校验。
- 所有的错误（如配置文件找不到）必须在启动的最初阶段被捕捉并输出友好的错误信息，禁止在运行时隐式 panic。

## 4. 代码入口
- CLI 定义: `crates/agent/src/bin/cyber-jianghu-agent.rs`
- 配置解析: `crates/agent/src/config.rs`
