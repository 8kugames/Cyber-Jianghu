// ============================================================================
// OpenClaw Cyber-Jianghu 配置缓存
// ============================================================================
//
// 本模块提供运行时配置缓存，使用 Arc<RwLock>> 实现线程安全
// ============================================================================

use super::types::GameData;
use cyber_jianghu_protocol::{DeathInfo, LocationEdge, LocationGraph, LocationNode};
use std::sync::Arc;

/// 配置缓存
///
/// 使用 RwLock 实现线程安全的读写访问
#[derive(Debug, Clone)]
pub struct GameDataCache {
    /// 使用 `Arc<RwLock<GameData>>` 实现线程安全的可变状态
    /// 注意：RwLock 是同步的，适用于异步环境
    data: Arc<std::sync::RwLock<GameData>>,

    /// 位置注册表
    pub location_registry: Arc<std::sync::RwLock<LocationRegistry>>,
}

impl GameDataCache {
    /// 创建新的配置缓存
    ///
    /// # 参数
    /// - `data`: 初始游戏数据
    pub fn new(data: GameData) -> Self {
        let location_registry = LocationRegistry::from_config(&data.locations);
        let data_arc = Arc::new(std::sync::RwLock::new(data));

        Self {
            data: data_arc,
            location_registry: Arc::new(std::sync::RwLock::new(location_registry)),
        }
    }

    /// 获取游戏数据（不可变引用）
    ///
    /// 注意：持有读锁期间，其他线程可以继续读取，但不能写入
    pub fn get(&self) -> std::sync::RwLockReadGuard<'_, GameData> {
        self.data.read().unwrap_or_else(|e| {
            // RwLock 被污染的情况理论上不应该发生
            // 这里直接 panic，因为这是严重错误
            panic!("配置缓存被污染: {}", e)
        })
    }

    /// 更新游戏数据
    ///
    /// 用于热加载配置时更新缓存内容
    ///
    /// # 参数
    /// - `data`: 新的游戏数据
    pub fn update(&self, data: GameData) {
        // 1. 更新位置注册表
        let new_registry = LocationRegistry::from_config(&data.locations);
        {
            let mut registry_guard = self
                .location_registry
                .write()
                .unwrap_or_else(|e| panic!("位置注册表缓存被污染: {}", e));
            *registry_guard = new_registry;
        }

        // 2. 更新游戏数据
        {
            let mut data_guard = self
                .data
                .write()
                .unwrap_or_else(|e| panic!("配置缓存被污染: {}", e));
            *data_guard = data;
        }
    }

    /// 获取 Arc 克隆
    ///
    /// 用于跨线程传递缓存引用
    #[allow(dead_code)]
    pub fn clone_arc(&self) -> Arc<std::sync::RwLock<GameData>> {
        Arc::clone(&self.data)
    }

    /// 获取属性的死亡信息
    ///
    /// 从统一属性配置中获取死亡原因和描述
    pub fn get_death_info(&self, attr_name: &str) -> Option<DeathInfo> {
        let config = self.get();
        let status_def = config.attributes.data.status.attributes.get(attr_name)?;

        let cause = status_def.death_cause.clone()?;
        let message = status_def.death_message.clone()?;

        Some(DeathInfo { cause, message })
    }
}

// ============================================================================
// 位置注册表
// ============================================================================

/// 位置注册表
#[derive(Debug)]
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
                node_type: match node.node_type.as_str() {
                    "map" => cyber_jianghu_protocol::LocationNodeType::Map,
                    "sub_scene" => cyber_jianghu_protocol::LocationNodeType::SubScene,
                    _ => cyber_jianghu_protocol::LocationNodeType::Map,
                },
                parent_id: if node.parent_id.is_empty() {
                    None
                } else {
                    Some(node.parent_id.clone())
                },
                environmental_damage: node.environmental_damage,
                gatherable_items: node.gatherable_items.clone().unwrap_or_default(),
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
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_data::types::*;

    fn create_test_game_data() -> GameData {
        GameData {
            game_rules: UnifiedGameRulesConfig {
                version: "2.0.0".to_string(),
                description: "".to_string(),
                meta: Default::default(),
                data: GameRulesData {
                    agent_state: AgentStateRulesData {
                        tick: TickRulesData {
                            real_seconds_per_tick: 60,
                        },
                        location: LocationRulesData {
                            spawn_location: "longmen_inn".to_string(),
                        },
                        game_time: GameTimeRulesData {
                            start_date: "2024-01-01".to_string(),
                            timezone_offset: 8, // UTC+8 北京时间
                        },
                    },
                    validation: ValidationRulesData {
                        action_validation: ActionValidationRulesData {
                            max_content_length: 500,
                        },
                        max_agent_name_length: 100,
                        max_system_prompt_length: 102400,
                        max_speak_content_length: 500,
                    },
                    ops: OpsRulesData {
                        death_threshold: 10,
                        offline_cleanup_days: 30,
                    },
                },
            },
            items: UnifiedItemsConfig {
                version: "2.0.0".to_string(),
                description: "".to_string(),
                meta: Default::default(),
                data: vec![],
            },
            actions: UnifiedActionsConfig {
                version: "2.0.0".to_string(),
                description: "".to_string(),
                meta: Default::default(),
                data: std::collections::HashMap::new(),
            },
            initial_inventory: UnifiedInitialInventoryConfig {
                version: "2.0.0".to_string(),
                description: "".to_string(),
                meta: Default::default(),
                data: InitialInventoryData { items: vec![] },
            },
            inventory: UnifiedInventoryConfig {
                version: "2.0.0".to_string(),
                description: "".to_string(),
                meta: Default::default(),
                data: InventoryLimitsData {
                    max_slots: 10,
                    max_stack_size: 10,
                },
            },
            network: UnifiedNetworkConfig {
                version: "2.0.0".to_string(),
                description: "".to_string(),
                meta: Default::default(),
                data: NetworkConfigData {
                    websocket: WebSocketConfigData {
                        rate_limit_ms: 500,
                        cleanup_interval_secs: 300,
                        cleanup_threshold: 100,
                    },
                    dialogue: Default::default(),
                },
            },
            locations: UnifiedLocationsConfig {
                version: "2.0.0".to_string(),
                description: "".to_string(),
                meta: Default::default(),
                data: LocationsData {
                    nodes: vec![],
                    edges: vec![],
                },
            },
            attributes: UnifiedAttributesConfig {
                version: "0.0.1".to_string(),
                description: "测试统一属性配置".to_string(),
                meta: Default::default(),
                data: AttributeCategories {
                    primary: PrimaryAttributesCategory {
                        description: "主属性".to_string(),
                        attributes: std::collections::HashMap::new(),
                    },
                    status: StatusAttributesCategory {
                        description: "状态值".to_string(),
                        attributes: std::collections::HashMap::new(),
                    },
                    derived: DerivedAttributesCategory {
                        description: "派生属性".to_string(),
                        attributes: std::collections::HashMap::new(),
                    },
                },
            },
            recipes: UnifiedRecipesConfig {
                version: "2.0.0".to_string(),
                description: "".to_string(),
                meta: Default::default(),
                data: std::collections::HashMap::new(),
            },
            time: UnifiedTimeConfig {
                version: "2.0.0".to_string(),
                description: "".to_string(),
                meta: Default::default(),
                data: TimeData {
                    ticks_per_hour: 60,
                    hours_per_day: 24,
                    days_per_season: 10,
                    seasons: vec![],
                },
            },
            narrative: Default::default(),
            display_messages: Default::default(),
        }
    }

    #[test]
    fn test_cache_creation() {
        let data = create_test_game_data();
        let cache = GameDataCache::new(data);

        let guard = cache.get();
        assert_eq!(guard.game_rules.version, "2.0.0");
    }

    #[test]
    fn test_cache_update() {
        let data = create_test_game_data();
        let cache = GameDataCache::new(data);

        // 创建新数据
        let mut new_data = create_test_game_data();
        new_data.game_rules.version = "3.0.0".to_string();

        // 更新缓存
        cache.update(new_data);

        // 验证更新
        let guard = cache.get();
        assert_eq!(guard.game_rules.version, "3.0.0");
    }

    #[test]
    fn test_multiple_reads() {
        let data = create_test_game_data();
        let cache = GameDataCache::new(data);

        // 多个读取操作应该都能成功
        let guard1 = cache.get();
        let guard2 = cache.get();
        let guard3 = cache.get();

        assert_eq!(guard1.game_rules.version, "2.0.0");
        assert_eq!(guard2.game_rules.version, "2.0.0");
        assert_eq!(guard3.game_rules.version, "2.0.0");
    }

    #[test]
    fn test_clone_arc() {
        let data = create_test_game_data();
        let cache = GameDataCache::new(data);

        let arc = cache.clone_arc();
        // 验证 Arc 可以正常使用
        let guard = arc.read().unwrap();
        assert_eq!(guard.game_rules.version, "2.0.0");
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
                        node_type: "sub_scene".to_string(),
                        parent_id: "inn".to_string(),
                        description: None,
                        environmental_damage: None,
                        gatherable_items: None,
                    },
                    LocationNodeData {
                        node_id: "kitchen".to_string(),
                        name: "厨房".to_string(),
                        node_type: "sub_scene".to_string(),
                        parent_id: "inn".to_string(),
                        description: None,
                        environmental_damage: None,
                        gatherable_items: None,
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

    // ========================================================================
    // get_death_info tests
    // ========================================================================

    #[test]
    fn test_get_death_info_returns_cause_and_message() {
        use crate::game_data::test_utils::init_test_registry;

        // Initialize test registry with death info configured
        init_test_registry();
        let cache = super::super::registry::registry().expect("test registry should be initialized");

        // Test hunger death info
        let info = cache.get_death_info("hunger");
        assert!(info.is_some(), "hunger should have death info");
        let info = info.unwrap();
        assert_eq!(info.cause, "hunger");
        assert!(info.message.contains("饥饿"), "death message should mention hunger");

        // Test thirst death info
        let info = cache.get_death_info("thirst");
        assert!(info.is_some(), "thirst should have death info");
        let info = info.unwrap();
        assert_eq!(info.cause, "thirst");
        assert!(info.message.contains("脱水"), "death message should mention dehydration");
    }

    #[test]
    fn test_get_death_info_returns_none_for_non_death_attribute() {
        use crate::game_data::test_utils::init_test_registry;

        init_test_registry();
        let cache = super::super::registry::registry().expect("test registry should be initialized");

        // Non-death attribute should return None
        let info = cache.get_death_info("hp");
        assert!(info.is_none(), "hp should not have death info");
    }

    #[test]
    fn test_get_death_info_returns_none_for_unknown_attribute() {
        use crate::game_data::test_utils::init_test_registry;

        init_test_registry();
        let cache = super::super::registry::registry().expect("test registry should be initialized");

        // Unknown attribute should return None
        let info = cache.get_death_info("nonexistent");
        assert!(info.is_none(), "unknown attribute should return None");
    }
}
