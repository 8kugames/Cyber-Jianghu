# 层级位置图系统

**级别**: P1 重要特性
**模块**: `crates/protocol`

## 1. 设计目标
通过层级化的地理位置结构，提供高维度的空间认知，让 Agent 能够规划长距离跨区移动和场景探索，同时控制网络广播的数据量。

## 2. 核心机制
### 2.1 树状地理拓扑
地图数据通过 `locations.yaml` 定义为三级树状结构：
- **大区 (Region)**：如“江南”、“西域”。
- **地图 (Map)**：如“临安城”、“沙漠客栈”。
- **子场景/节点 (SubScene/NodeID)**：如“客栈大堂”、“临安城东门”。这是 Agent 实际所处的物理坐标。

### 2.2 拓扑推导与连通性
- Server 在启动时解析配置，自动推导节点之间的连通关系（Neighbors）。
- 生成全局导航图，支持路径寻找（Pathfinding）。

### 2.3 视野可见性与协议传输
- 在 `WorldState` 广播时，**视距隔离**生效：仅下发当前所在 `NodeID` 的同场景实体，以及相邻节点的摘要信息。
- 避免全图数据造成的网络风暴，极大降低了 WebSocket 广播带宽和 LLM 的上下文窗口占用。

## 3. 架构约束
- Agent 只能感知同层级或相邻层级的位置变化。
- 跨地图移动必须经过特定的出入口节点（Portal Nodes）。

## 4. 代码入口
- 拓扑定义: `crates/protocol/src/location.rs`
- Server 视距过滤: `crates/server/src/tick/scheduler.rs` (WorldState 构建逻辑)
