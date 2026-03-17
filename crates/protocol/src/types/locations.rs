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
        self.edges.entry(from).or_insert_with(Vec::new).push(edge);
    }

    /// 获取节点的所有相邻节点
    pub fn get_neighbors(&self, node_id: &str) -> Vec<&LocationEdge> {
        self.edges
            .get(node_id)
            .map(|v| v.iter().collect())
            .unwrap_or_default()
    }

    /// 检查两个节点是否直接相连
    pub fn is_connected(&self, from: &str, to: &str) -> bool {
        self.get_neighbors(from).iter().any(|e| e.to_node_id == to)
    }
}

impl Default for LocationGraph {
    fn default() -> Self {
        Self::new()
    }
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
}
