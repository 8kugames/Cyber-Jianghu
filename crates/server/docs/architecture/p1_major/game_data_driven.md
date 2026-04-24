# 游戏数据驱动系统

**级别**: P1 重要特性
**模块**: `crates/server`

## 1. 设计目标
将所有业务逻辑抽离为外部配置文件，实现修改即生效，做到“零硬编码”。极大降低游戏平衡性调整和版本迭代的开发成本。

## 2. 核心机制
### 2.1 YAML/JSON 载入
通过 `serde_yaml` 和 `serde_json` 解析位于 `config/` 目录下的各种数据字典：
- `actions.yaml`：定义所有可执行动作、验证等级、参数格式。
- `attributes.yaml`：定义基础属性名和派生属性的计算公式。
- `items.yaml` / `locations.yaml` / `recipes.yaml` 等。

### 2.2 热重载 (Hot Reload)
- 后台线程或特定管理接口监听配置文件的 `mtime` 变化。
- 一旦发生修改，触发内存中配置注册表（`ConfigRegistry`）的无缝替换，不中断当前的 Server 进程。
- 随后通过 WebSocket 广播 `GameRulesUpdate` 通知所有 Agent 更新认知。

### 2.3 公式引擎 (evalexpr)
- 将诸如“伤害计算”、“派生属性计算”转化为配置中的字符串公式（如 `(strength * 2) + agility`）。
- 运行时通过 `evalexpr` 库安全求值，支持动态解析当前的属性上下文。

## 3. 架构约束
- Server 代码库中不得硬编码任何阈值、伤害基数、物品 ID 或节点 ID。
- 热重载过程中必须使用 `Arc<RwLock>` 保护读取一致性。

## 4. 代码入口
- 数据加载与解析: `crates/server/src/game_data/loaders/`
- 公式引擎集成: `crates/server/src/game_data/formula_engine/engine.rs`
