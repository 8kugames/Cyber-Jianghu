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

/// 地点时代变体（吸收自传灯录拓扑地图的 time_variants 设计）
///
/// 按游戏日闭区间控制地点显隐与描述：`[from_game_day, to_game_day]` 两端含。
/// 缺省边界 = 无下界/无上界；列表按序首个命中生效；全部未命中回落基础定义。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LocationTimeVariant {
    /// 生效起始游戏日（含，1-based）；None = 无下界
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_game_day: Option<i64>,

    /// 生效结束游戏日（含，1-based）；None = 无上界
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_game_day: Option<i64>,

    /// 该时段的描述（覆盖节点基础 description；None = 沿用基础描述）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// false = 该时段隐藏（不出现在邻接、不可移动进入）；缺省 = 显示
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
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

    /// 节点描述（YAML 配置驱动，game_data 转换时需保留）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

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

    /// 时代变体列表（按游戏日闭区间，首个命中生效；空 = 恒显示基础定义）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub time_variants: Vec<LocationTimeVariant>,
}

impl LocationNode {
    /// 解析指定游戏日命中的时代变体（首个命中；None = 无变体或全部未命中）
    pub fn resolve_time_variant(&self, game_day: i64) -> Option<&LocationTimeVariant> {
        self.time_variants.iter().find(|v| {
            let lo_ok = v.from_game_day.is_none_or(|d| game_day >= d);
            let hi_ok = v.to_game_day.is_none_or(|d| game_day <= d);
            lo_ok && hi_ok
        })
    }

    /// 指定游戏日是否可见（命中的变体 visible=false 隐藏；未命中/无变体恒显示）
    pub fn is_visible_at(&self, game_day: i64) -> bool {
        self.resolve_time_variant(game_day)
            .and_then(|v| v.visible)
            .unwrap_or(true)
    }

    /// 指定游戏日的描述（命中的变体 description 优先，回落基础 description）
    pub fn resolved_description(&self, game_day: i64) -> Option<&str> {
        self.resolve_time_variant(game_day)
            .and_then(|v| v.description.as_deref())
            .or(self.description.as_deref())
    }
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

    /// 检查节点在指定游戏日是否可见（节点不存在 = 不可见）
    ///
    /// 时代显隐真源：移动校验与邻接广播均以此为准。
    /// 语义（与传灯录一致）：终点隐藏 → 不可达；起点不校验（所在地永远允许离开）。
    pub fn is_visible_at(&self, node_id: &str, game_day: i64) -> bool {
        self.nodes
            .get(node_id)
            .map(|n| n.is_visible_at(game_day))
            .unwrap_or(false)
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
}

/// 可采集资源信息（用于 WorldState）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatherableItem {
    /// 物品 uuid（v5 派生，同背包物品标识体系；采集意图照抄此 uuid，
    /// 人魂直连可见，天魂审查时校验有效性）
    pub item_id: String,
    /// 物品名称（人魂可见的显示名称）
    pub name: String,
    /// 物品类型（consumable/weapon/material 等）
    #[serde(default)]
    pub item_type: String,
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

    /// 祖先名称链（根区域 → 直接父级，不含当前节点；区域根为空链）
    /// 逐级显示契约：客户端舆图分层布局与位置层级表述的上下文来源。
    /// 注意：名称链不保证唯一性（极端同名场景仍歧义），唯一定位以 node_id 为准。
    /// 消费契约：客户端以链尾名称（或节点 ID）在 adjacent_nodes 中匹配直接父级。
    /// 约定同一父级的邻接集内节点名称唯一（装载配置约定保障）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parent_chain: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn variant(
        from: Option<i64>,
        to: Option<i64>,
        visible: Option<bool>,
        desc: Option<&str>,
    ) -> LocationTimeVariant {
        LocationTimeVariant {
            from_game_day: from,
            to_game_day: to,
            visible,
            description: desc.map(str::to_string),
        }
    }

    fn node_with(variants: Vec<LocationTimeVariant>) -> LocationNode {
        LocationNode {
            node_id: "n".to_string(),
            name: "测试".to_string(),
            node_type: LocationNodeType::Map,
            parent_id: None,
            description: Some("基础".to_string()),
            environmental_damage: None,
            gatherable_items: vec![],
            implicit_travel_cost: None,
            time_variants: variants,
        }
    }

    #[test]
    fn time_variant_interval_bounds_are_inclusive() {
        let n = node_with(vec![variant(Some(10), Some(20), None, None)]);
        assert!(n.resolve_time_variant(9).is_none(), "下界前不可命中");
        assert!(n.is_visible_at(9), "区间外回落恒显示");
        assert!(n.resolve_time_variant(10).is_some(), "下界当日命中（含）");
        assert!(n.resolve_time_variant(20).is_some(), "上界当日命中（含）");
        assert!(n.resolve_time_variant(21).is_none(), "上界后未命中");
        assert!(n.is_visible_at(21), "区间外回落恒显示");
    }

    #[test]
    fn time_variant_open_bounds_default_visible() {
        // 仅设 from：无上界
        let n = node_with(vec![variant(Some(5), None, Some(false), None)]);
        assert!(n.is_visible_at(4), "无变体命中回落恒显示");
        assert!(!n.is_visible_at(5), "from 起隐藏");
        assert!(!n.is_visible_at(i64::MAX), "无上界持续隐藏");

        // 仅设 to：无下界
        let n2 = node_with(vec![variant(None, Some(5), Some(false), None)]);
        assert!(!n2.is_visible_at(i64::MIN), "无下界从远古隐藏");
        assert!(!n2.is_visible_at(5));
        assert!(n2.is_visible_at(6));
    }

    #[test]
    fn time_variant_first_match_wins() {
        let n = node_with(vec![
            variant(Some(1), Some(10), Some(true), Some("早期")),
            variant(Some(1), Some(100), Some(false), Some("后期")),
        ]);
        let v = n.resolve_time_variant(5).expect("应命中首个变体");
        assert_eq!(v.description.as_deref(), Some("早期"));
        assert!(n.is_visible_at(5), "首个命中 visible=true 生效");
    }

    #[test]
    fn time_variant_description_fallback() {
        let n = node_with(vec![variant(Some(1), Some(10), None, Some("乱世废墟"))]);
        assert_eq!(n.resolved_description(5), Some("乱世废墟"));
        assert_eq!(
            n.resolved_description(11),
            Some("基础"),
            "未命中回落基础描述"
        );

        let no_desc = node_with(vec![variant(Some(1), Some(10), None, None)]);
        assert_eq!(
            no_desc.resolved_description(5),
            Some("基础"),
            "变体无描述沿用基础"
        );
    }

    #[test]
    fn time_variants_field_is_additive_on_wire() {
        // 旧客户端/旧配置无 time_variants 字段 → 反序列化为空表（向后兼容的 additive 变更）
        let legacy = r#"{"node_id":"x","name":"X","type":"map"}"#;
        let n: LocationNode = serde_json::from_str(legacy).expect("旧格式应可解析");
        assert!(n.time_variants.is_empty());
        assert!(n.is_visible_at(1));

        // 新字段序列化 round-trip
        let n2 = node_with(vec![variant(Some(3), None, Some(false), Some("d"))]);
        let json = serde_json::to_string(&n2).unwrap();
        assert!(json.contains("time_variants"));
        let back: LocationNode = serde_json::from_str(&json).unwrap();
        assert_eq!(back.time_variants, n2.time_variants);
    }

    #[test]
    fn graph_visibility_missing_node_is_false() {
        let g = LocationGraph::new();
        assert!(!g.is_visible_at("ghost", 1), "不存在即不可见");
    }
}
