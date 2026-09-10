// ============================================================================
// 位置注册表（自 cache.rs 拆分，原文件超 800 行上限）
// ============================================================================
//
// LocationGraph 的服务端包装：邻接查询（显式边 + 隐式 parent-child）、
// 时代显隐可见性（time_variants）、时代感知描述解析。
// 图语义唯一事实文档：docs/architecture/p0_core/locations_graph.md
// ============================================================================

use cyber_jianghu_protocol::{AdjacentNode, LocationEdge, LocationGraph, LocationNode};

// 位置注册表
// ============================================================================

/// 位置注册表
#[derive(Debug, Clone)]
pub struct LocationRegistry {
    graph: LocationGraph,
}

impl LocationRegistry {
    /// 从配置创建位置注册表
    pub fn from_config(config: &super::types::UnifiedLocationsConfig) -> Self {
        let mut graph = LocationGraph::new();

        // 添加节点
        for node in &config.data.nodes {
            let location_node = LocationNode {
                node_id: node.node_id.clone(),
                name: node.name.clone(),
                node_type: node.node_type,
                parent_id: node.parent_id.clone(),
                description: node.description.clone(),
                environmental_damage: node.environmental_damage,
                gatherable_items: node.gatherable_items.clone(),
                implicit_travel_cost: node.implicit_travel_cost,
                time_variants: node.time_variants.clone(),
            };
            graph.add_node(location_node);
        }

        // 添加边
        for edge in &config.data.edges {
            let location_edge = LocationEdge {
                from_node_id: edge.from.clone(),
                to_node_id: edge.to.clone(),
                travel_cost: edge.travel_cost as u32,
            };
            graph.add_edge(location_edge);
        }

        Self { graph }
    }

    /// 检查节点是否存在
    pub fn node_exists(&self, node_id: &str) -> bool {
        self.graph.nodes.contains_key(node_id)
    }

    /// 获取节点信息
    pub fn get_node(&self, node_id: &str) -> Option<&LocationNode> {
        self.graph.nodes.get(node_id)
    }

    /// 检查两个节点是否直接相连
    pub fn is_connected(&self, from: &str, to: &str) -> bool {
        self.graph.is_connected(from, to)
    }

    /// 获取移动消耗
    #[allow(dead_code)]
    pub fn get_travel_cost(&self, from: &str, to: &str) -> Option<u32> {
        self.graph
            .get_neighbors(from)
            .iter()
            .find(|e| e.to_node_id == to)
            .map(|e| e.travel_cost)
    }

    /// 获取节点的所有相邻边
    pub fn get_neighbors(&self, node_id: &str) -> Vec<&LocationEdge> {
        self.graph.get_neighbors(node_id)
    }

    /// 获取所有邻居（显式边 + 隐式 parent-child），自动去重
    pub fn get_all_neighbors(
        &self,
        node_id: &str,
        default_implicit_travel_cost: u32,
    ) -> Vec<AdjacentNode> {
        self.graph
            .get_all_neighbors(node_id, default_implicit_travel_cost)
    }

    /// 节点在指定游戏日是否可见（存在且未被时代变体隐藏）
    ///
    /// execute_move 的终点校验用：隐藏地点等价于"当前不存在"。
    pub fn is_node_visible(&self, node_id: &str, game_day: i64) -> bool {
        self.graph.is_visible_at(node_id, game_day)
    }

    /// 获取指定游戏日可见的邻居（显式 + 隐式，过滤时代隐藏地点）
    ///
    /// WorldState 邻接广播用：Agent 只能看到当前时代存在的可达地点。
    pub fn get_visible_neighbors(
        &self,
        node_id: &str,
        default_implicit_travel_cost: u32,
        game_day: i64,
    ) -> Vec<AdjacentNode> {
        self.get_all_neighbors(node_id, default_implicit_travel_cost)
            .into_iter()
            .filter(|adj| self.graph.is_visible_at(&adj.node_id, game_day))
            .collect()
    }

    /// 各节点在指定游戏日的解析描述（时代变体命中优先，回落基础；无描述节点不入表）
    ///
    /// dashboard locations 端点消费（天道全知视角下的时代感知描述）。
    pub fn resolved_descriptions_at(
        &self,
        game_day: i64,
    ) -> std::collections::HashMap<String, String> {
        self.graph
            .nodes
            .iter()
            .filter_map(|(id, node)| {
                node.resolved_description(game_day)
                    .map(|d| (id.clone(), d.to_string()))
            })
            .collect()
    }

    /// 导出整个图为 owned 可序列化结构（C4：地点端点用）
    ///
    /// 返回 `LocationGraph` 的 owned 克隆（节点+边），前端可以一次性拿到完整
    /// 地图拓扑，无需逐节点遍历。`LocationGraph` 已实现 Serialize。
    pub fn export_graph(&self) -> LocationGraph {
        self.graph.clone()
    }

    /// 所有节点 id（owned，便于遍历）
    pub fn all_node_ids(&self) -> Vec<String> {
        self.graph.nodes.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_data::types::{
        LocationEdgeData, LocationNodeData, LocationsData, UnifiedLocationsConfig,
    };

    /// 回归测试：locations.yaml 的 region 节点不能被降级为 Map。
    /// 之前 cache.rs 手写 match 只认 map/sub_scene，region 落默认 Map。
    #[test]
    fn test_location_node_type_region_not_swallowed() {
        let yaml = r#"
version: "1.0"
data:
  nodes:
    - node_id: test_region
      name: 测试区域
      type: "region"
      parent_id: None
  edges: []
"#;
        let config: UnifiedLocationsConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = LocationRegistry::from_config(&config);
        let node = registry.graph.nodes.get("test_region");
        assert!(node.is_some(), "region 节点应存在");
        assert_eq!(
            node.unwrap().node_type,
            cyber_jianghu_protocol::LocationNodeType::Region,
            "region 不应被降级为 Map"
        );
    }

    #[test]
    fn test_location_registry() {
        let config = UnifiedLocationsConfig {
            version: "2.0.0".to_string(),
            description: "".to_string(),
            meta: Default::default(),
            data: LocationsData {
                nodes: vec![
                    LocationNodeData {
                        node_id: "lobby".to_string(),
                        name: "大堂".to_string(),
                        node_type: cyber_jianghu_protocol::LocationNodeType::SubScene,
                        parent_id: Some("inn".to_string()),
                        description: None,
                        environmental_damage: None,
                        gatherable_items: vec![],
                        implicit_travel_cost: None,
                        time_variants: vec![],
                    },
                    LocationNodeData {
                        node_id: "kitchen".to_string(),
                        name: "厨房".to_string(),
                        node_type: cyber_jianghu_protocol::LocationNodeType::SubScene,
                        parent_id: Some("inn".to_string()),
                        description: None,
                        environmental_damage: None,
                        gatherable_items: vec![],
                        implicit_travel_cost: None,
                        time_variants: vec![],
                    },
                ],
                edges: vec![LocationEdgeData {
                    from: "lobby".to_string(),
                    to: "kitchen".to_string(),
                    travel_cost: 1,
                }],
            },
        };

        let registry = LocationRegistry::from_config(&config);

        assert!(registry.node_exists("lobby"));
        assert!(registry.is_connected("lobby", "kitchen"));
        assert_eq!(registry.get_travel_cost("lobby", "kitchen"), Some(1));
    }

    #[test]
    fn test_time_variants_visibility_filtering() {
        let hidden_later = cyber_jianghu_protocol::LocationTimeVariant {
            from_game_day: Some(10),
            to_game_day: None,
            visible: Some(false),
            description: Some("已在战乱中湮灭".to_string()),
        };
        let config = UnifiedLocationsConfig {
            version: "1.0".to_string(),
            description: "".to_string(),
            meta: Default::default(),
            data: LocationsData {
                nodes: vec![
                    LocationNodeData {
                        node_id: "inn".to_string(),
                        name: "客栈".to_string(),
                        node_type: cyber_jianghu_protocol::LocationNodeType::Map,
                        parent_id: None,
                        description: None,
                        environmental_damage: None,
                        gatherable_items: vec![],
                        implicit_travel_cost: None,
                        time_variants: vec![],
                    },
                    LocationNodeData {
                        node_id: "ruins".to_string(),
                        name: "古城废墟".to_string(),
                        node_type: cyber_jianghu_protocol::LocationNodeType::Map,
                        parent_id: None,
                        description: Some("昔日繁华的古城".to_string()),
                        environmental_damage: None,
                        gatherable_items: vec![],
                        implicit_travel_cost: None,
                        time_variants: vec![hidden_later],
                    },
                ],
                edges: vec![
                    LocationEdgeData {
                        from: "inn".to_string(),
                        to: "ruins".to_string(),
                        travel_cost: 2,
                    },
                    LocationEdgeData {
                        from: "ruins".to_string(),
                        to: "inn".to_string(),
                        travel_cost: 2,
                    },
                ],
            },
        };

        let registry = LocationRegistry::from_config(&config);

        // 第 9 日：两处均可见
        assert!(registry.is_node_visible("ruins", 9));
        let adj = registry.get_visible_neighbors("inn", 1, 9);
        assert_eq!(adj.len(), 1, "第 9 日应看到 ruins 邻居");

        // 第 10 日起：ruins 隐藏——不可见且从邻接中消失
        assert!(!registry.is_node_visible("ruins", 10));
        let adj = registry.get_visible_neighbors("inn", 1, 10);
        assert!(adj.is_empty(), "隐藏地点不应出现在邻接广播中");

        // 隐藏地点作为移动终点应被拒（execute_move 消费同一判定）
        assert!(registry.is_connected("inn", "ruins"), "图上仍有边");
        assert!(!registry.is_node_visible("ruins", 10), "但时代不可见");

        // 时代描述解析（dashboard 消费）：第 9 日回落基础描述，第 10 日起命中变体描述；
        // 天道全知视角：隐藏节点的描述同样解析
        let d9 = registry.resolved_descriptions_at(9);
        assert_eq!(d9.get("ruins").map(String::as_str), Some("昔日繁华的古城"));
        let d10 = registry.resolved_descriptions_at(10);
        assert_eq!(d10.get("ruins").map(String::as_str), Some("已在战乱中湮灭"));
        assert!(!d10.contains_key("inn"), "无描述节点不入表");
    }

    #[test]
    fn test_implicit_travel_cost_passthrough() {
        // 回归：from_config 曾把节点级 implicit_travel_cost 硬置 None（配置被静默丢弃）
        let config = UnifiedLocationsConfig {
            version: "1.0".to_string(),
            description: "".to_string(),
            meta: Default::default(),
            data: LocationsData {
                nodes: vec![
                    LocationNodeData {
                        node_id: "region".to_string(),
                        name: "区域".to_string(),
                        node_type: cyber_jianghu_protocol::LocationNodeType::Region,
                        parent_id: None,
                        description: None,
                        environmental_damage: None,
                        gatherable_items: vec![],
                        implicit_travel_cost: None,
                        time_variants: vec![],
                    },
                    LocationNodeData {
                        node_id: "village".to_string(),
                        name: "村庄".to_string(),
                        node_type: cyber_jianghu_protocol::LocationNodeType::Map,
                        parent_id: Some("region".to_string()),
                        description: None,
                        environmental_damage: None,
                        gatherable_items: vec![],
                        implicit_travel_cost: Some(7),
                        time_variants: vec![],
                    },
                ],
                edges: vec![],
            },
        };

        let registry = LocationRegistry::from_config(&config);
        // 全局默认 1，节点覆盖 7：隐式边应取节点覆盖值
        let neighbors = registry.get_all_neighbors("village", 1);
        let region_edge = neighbors
            .iter()
            .find(|n| n.node_id == "region")
            .expect("隐式 parent-child 边应存在");
        assert_eq!(
            region_edge.travel_cost, 7,
            "节点级 implicit_travel_cost 应生效而非被丢弃"
        );
    }
}
