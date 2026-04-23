//! 位置图相关类型
//!
//! 地图分层系统类型（Region → Map → SubScene）

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

/// 节点类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocationNodeType {
    /// 区域（Region）- 最高层级
    Region,

    /// 地图（Map）- 中间层级
    Map,

    /// 子场景（SubScene）- 最低层级，MVP 实现重点
    SubScene,
}

impl fmt::Display for LocationNodeType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Region => write!(f, "region"),
            Self::Map => write!(f, "map"),
            Self::SubScene => write!(f, "sub_scene"),
        }
    }
}

/// 位置节点（三层统一接口）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocationNode {
    /// 节点 ID（全局唯一）
    pub node_id: String,

    /// 节点名称
    pub name: String,

    /// 节点类型
    #[serde(rename = "type")]
    pub node_type: LocationNodeType,

    /// 父节点 ID（子场景 → 地图 → 区域）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,

    /// 环境伤害（每Tick扣除的HP值）
    /// 如果为 0 或 None，则表示无环境伤害
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environmental_damage: Option<i32>,

    /// 可采集物品列表
    /// 格式：["item_id1", "item_id2"]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gatherable_items: Vec<String>,

    /// 隐式 parent-child 连接的 travel_cost 覆盖
    /// None 时使用全局 default_implicit_travel_cost
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implicit_travel_cost: Option<u32>,

    /// 别名列表（供 LLM 别名映射使用）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
}

/// 节点连接（边）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocationEdge {
    /// 起点
    pub from_node_id: String,

    /// 终点
    pub to_node_id: String,

    /// 移动消耗（Tick 数）
    pub travel_cost: u32,
}

/// 地图图结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocationGraph {
    /// 所有节点
    pub nodes: HashMap<String, LocationNode>,

    /// 所有边（邻接表）
    pub edges: HashMap<String, Vec<LocationEdge>>,
}

impl LocationGraph {
    /// 创建空图
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            edges: HashMap::new(),
        }
    }

    /// 添加节点
    pub fn add_node(&mut self, node: LocationNode) {
        let node_id = node.node_id.clone();
        self.nodes.insert(node_id, node);
    }

    /// 添加边
    pub fn add_edge(&mut self, edge: LocationEdge) {
        let from = edge.from_node_id.clone();
        self.edges.entry(from).or_default().push(edge);
    }

    /// 获取节点的所有相邻节点
    pub fn get_neighbors(&self, node_id: &str) -> Vec<&LocationEdge> {
        self.edges
            .get(node_id)
            .map(|v| v.iter().collect())
            .unwrap_or_default()
    }

    /// 检查两个节点是否直接相连
    ///
    /// 自环（from == to）视为有效：原地不动是合法的"移动"。
    /// 支持隐式 parent-child 连接：sub_scene 与 parent 之间自动可达。
    pub fn is_connected(&self, from: &str, to: &str) -> bool {
        if from == to {
            return true;
        }
        // 显式边
        if self.get_neighbors(from).iter().any(|e| e.to_node_id == to) {
            return true;
        }
        // 隐式 parent-child：from 是 to 的 parent，或 to 是 from 的 parent
        if let Some(from_node) = self.nodes.get(from)
            && from_node.parent_id.as_deref() == Some(to)
        {
            return true;
        }
        if let Some(to_node) = self.nodes.get(to)
            && to_node.parent_id.as_deref() == Some(from)
        {
            return true;
        }
        false
    }

    /// 获取隐式 parent-child 邻居
    ///
    /// 返回通过 parent_id 关系隐式连接的相邻节点。
    /// `default_travel_cost` 在节点未配置 `implicit_travel_cost` 时使用。
    pub fn get_implicit_neighbors(
        &self,
        node_id: &str,
        default_travel_cost: u32,
    ) -> Vec<AdjacentNode> {
        let mut implicit = Vec::new();

        // 当前节点的 parent_id → parent 是邻居
        if let Some(node) = self.nodes.get(node_id)
            && let Some(parent_id) = &node.parent_id
            && let Some(parent) = self.nodes.get(parent_id)
        {
            let cost = node.implicit_travel_cost.unwrap_or(default_travel_cost);
            implicit.push(AdjacentNode {
                node_id: parent_id.clone(),
                name: parent.name.clone(),
                travel_cost: cost,
                aliases: parent.aliases.clone(),
            });
        }

        // 其他节点的 parent_id == node_id → children 是邻居
        for (child_id, child_node) in &self.nodes {
            if child_node.parent_id.as_deref() == Some(node_id) {
                let cost = child_node
                    .implicit_travel_cost
                    .unwrap_or(default_travel_cost);
                implicit.push(AdjacentNode {
                    node_id: child_id.clone(),
                    name: child_node.name.clone(),
                    travel_cost: cost,
                    aliases: child_node.aliases.clone(),
                });
            }
        }

        implicit
    }

    /// 获取所有邻居（显式 + 隐式 parent-child），自动去重
    ///
    /// 隐式连接中，显式边已存在时优先使用显式边的 travel_cost。
    pub fn get_all_neighbors(
        &self,
        node_id: &str,
        default_implicit_travel_cost: u32,
    ) -> Vec<AdjacentNode> {
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // 显式邻居
        for edge in self.get_neighbors(node_id) {
            if seen.insert(edge.to_node_id.clone())
                && let Some(node) = self.nodes.get(&edge.to_node_id)
            {
                result.push(AdjacentNode {
                    node_id: edge.to_node_id.clone(),
                    name: node.name.clone(),
                    travel_cost: edge.travel_cost,
                    aliases: node.aliases.clone(),
                });
            }
        }

        // 隐式邻居（去重：已通过显式边添加的跳过）
        for adj in self.get_implicit_neighbors(node_id, default_implicit_travel_cost) {
            if seen.insert(adj.node_id.clone()) {
                result.push(adj);
            }
        }

        result
    }
}

impl Default for LocationGraph {
    fn default() -> Self {
        Self::new()
    }
}

/// 相邻节点信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdjacentNode {
    /// 节点 ID
    pub node_id: String,
    /// 节点名称
    pub name: String,
    /// 移动消耗（tick 数）
    pub travel_cost: u32,

    /// 别名列表（供 LLM 别名映射使用）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
}

/// 可采集资源信息（用于 WorldState）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatherableItem {
    /// 物品 ID（人魂直连可见，天魂审查时校验有效性）
    pub item_id: String,
    /// 物品名称（人魂可见的显示名称）
    pub name: String,
    /// 物品类型（consumable/weapon/material 等）
    #[serde(default)]
    pub item_type: String,
    /// 别名列表（供 LLM 别名映射使用）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
}

/// 节点信息（简化版，用于 WorldState）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Location {
    /// 节点 ID
    pub node_id: String,

    /// 节点名称
    pub name: String,

    /// 节点类型（客栈、街道等）
    #[serde(rename = "type")]
    pub node_type: String,

    /// 可到达的相邻节点
    #[serde(default)]
    pub adjacent_nodes: Vec<AdjacentNode>,

    /// 当前位置可采集的资源（含名称，数据驱动）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gatherable_items: Vec<GatherableItem>,
}
