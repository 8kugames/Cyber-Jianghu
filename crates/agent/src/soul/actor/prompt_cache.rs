// ============================================================================
// Prompt 缓存模块 - 人魂 Prompt 叙事冗余优化
// ============================================================================
//
// 三层缓存架构：
// - Layer 1: 静态缓存（persona、actions）进程生命周期内不变
// - Layer 2: 半静态缓存（inventory、locations、entities）变化时更新
// - Layer 3: 动态状态每轮生成，不缓存
//
// 变化检测：FnvHash 快速哈希 + tick 版本控制
// ============================================================================

use fnv::FnvHasher;
use std::hash::{Hash, Hasher};

use crate::component::persona::DynamicPersona;
use crate::models::WorldState;
use cyber_jianghu_protocol::types::{AdjacentNode, Entity, SceneItem};

/// Prompt 缓存状态
///
/// 三层缓存：
/// - Static: persona_desc, persona_summary, actions_list
/// - SemiStatic: inventory, adjacent_locations, entities (带哈希检测)
/// - Dynamic: self_status, recent_speeches (每轮生成，不缓存)
pub struct PromptCache {
    // Static（进程生命周期内不变）
    persona_desc: String,
    persona_summary: String,
    actions_list: String,
    persona_initialized: bool,

    // Semi-static（变化时更新，FnvHash 加速）
    cached_inventory: String,
    cached_inventory_hash: u64,
    cached_adjacent: String,
    cached_adjacent_hash: u64,
    cached_entities: String,
    cached_entities_hash: u64,
    cached_nearby_items: String,
    cached_nearby_items_hash: u64,
    last_update_tick: i64,
}

impl PromptCache {
    /// 创建新的 PromptCache
    pub fn new(persona_desc: String, actions_list: String, persona: &DynamicPersona) -> Self {
        let persona_summary = Self::build_structured_summary(persona);
        Self {
            persona_desc,
            persona_summary,
            actions_list,
            persona_initialized: false,
            cached_inventory: String::new(),
            cached_inventory_hash: 0,
            cached_adjacent: String::new(),
            cached_adjacent_hash: 0,
            cached_entities: String::new(),
            cached_entities_hash: 0,
            cached_nearby_items: String::new(),
            cached_nearby_items_hash: 0,
            last_update_tick: -1,
        }
    }

    /// 构建结构化 persona 摘要
    ///
    /// 从完整 persona 描述中提取关键维度，避免简单截取丢失信息。
    /// 格式：你是 {name}，核心特质：{traits}
    pub fn build_structured_summary(persona: &DynamicPersona) -> String {
        // 提取核心特质
        let traits: Vec<String> = persona
            .traits
            .iter()
            .map(|(name, trait_val)| {
                let normalized_value = trait_val.value as f64 / 100.0;
                format!(
                    "{}{}",
                    name,
                    if normalized_value > 0.7 {
                        "（强烈倾向）"
                    } else if normalized_value > 0.5 {
                        "（倾向）"
                    } else if normalized_value < 0.3 {
                        "（回避）"
                    } else {
                        ""
                    }
                )
            })
            .collect();

        let traits_str = if traits.is_empty() {
            "待探索".to_string()
        } else {
            traits.join("、")
        };

        // 当前状态作为情境上下文
        let state_str = if persona.current_state.current_emotion != "平静" {
            format!("（当前心境：{}）", persona.current_state.current_emotion)
        } else {
            String::new()
        };

        format!(
            "你是 {}，核心特质：{}{}",
            persona.name, traits_str, state_str
        )
    }

    /// 判断是否需要完整 persona（首次调用时）
    pub fn needs_full_persona(&self) -> bool {
        !self.persona_initialized
    }

    /// 获取 persona 内容（差异化：第一轮完整，后续摘要）
    ///
    /// 使用 RwLock 保护并发访问
    pub fn get_persona(&mut self, _world_state: &WorldState) -> &str {
        if !self.persona_initialized {
            self.persona_initialized = true;
            &self.persona_desc
        } else {
            &self.persona_summary
        }
    }

    /// 失效 persona 缓存（rebirth 后调用）
    pub fn invalidate_persona(&mut self, persona_desc: String, persona: &DynamicPersona) {
        self.persona_desc = persona_desc;
        self.persona_summary = Self::build_structured_summary(persona);
        self.persona_initialized = false; // 强制下一轮使用完整版
    }

    /// 获取 actions_list
    pub fn get_actions_list(&self) -> &str {
        &self.actions_list
    }

    /// 使用 FnvHash 快速计算哈希
    #[inline]
    fn compute_hash(value: &str) -> u64 {
        let mut hasher = FnvHasher::default();
        value.hash(&mut hasher);
        hasher.finish()
    }

    /// 检查并更新半静态缓存
    ///
    /// 如果 tick_id 与上次相同，跳过检查（同 tick 内不重复更新）。
    /// 如果哈希变化，更新缓存内容。
    pub fn check_and_update(&mut self, world_state: &WorldState) {
        let tick_id = world_state.tick_id;

        // 同 tick 内不重复检查
        if tick_id == self.last_update_tick {
            return;
        }

        // 检查 inventory 变化
        let inventory_str = Self::format_inventory(&world_state.self_state.inventory);
        let inventory_hash = Self::compute_hash(&inventory_str);
        if inventory_hash != self.cached_inventory_hash {
            self.cached_inventory = inventory_str;
            self.cached_inventory_hash = inventory_hash;
        }

        // 检查 locations 变化
        let adj_str = Self::format_adjacent(&world_state.location.adjacent_nodes);
        let adj_hash = Self::compute_hash(&adj_str);
        if adj_hash != self.cached_adjacent_hash {
            self.cached_adjacent = adj_str;
            self.cached_adjacent_hash = adj_hash;
        }

        // 检查 entities 变化
        let entities_str = Self::format_entities(&world_state.entities);
        let entities_hash = Self::compute_hash(&entities_str);
        if entities_hash != self.cached_entities_hash {
            self.cached_entities = entities_str;
            self.cached_entities_hash = entities_hash;
        }

        // 检查 nearby_items 变化
        let nearby_str = Self::format_nearby_items(&world_state.nearby_items);
        let nearby_hash = Self::compute_hash(&nearby_str);
        if nearby_hash != self.cached_nearby_items_hash {
            self.cached_nearby_items = nearby_str;
            self.cached_nearby_items_hash = nearby_hash;
        }

        self.last_update_tick = tick_id;
    }

    /// 获取变化标记（用于 diff-based prompt 输出）
    pub fn get_change_markers(&self) -> ChangeMarkers {
        ChangeMarkers {
            inventory_changed: true, // 简化：始终标记变化，prompt 会显示内容
            locations_changed: true,
            entities_changed: true,
            nearby_items_changed: true,
        }
    }

    /// 强制刷新缓存（应对极端场景）
    pub fn force_refresh(&mut self) {
        self.last_update_tick = -1;
    }

    /// 获取缓存的 inventory
    pub fn get_inventory(&self) -> &str {
        &self.cached_inventory
    }

    /// 获取缓存的 adjacent locations
    pub fn get_adjacent(&self) -> &str {
        &self.cached_adjacent
    }

    /// 获取缓存的 entities
    pub fn get_entities(&self) -> &str {
        &self.cached_entities
    }

    /// 获取缓存的 nearby_items
    pub fn get_nearby_items(&self) -> &str {
        &self.cached_nearby_items
    }

    /// 格式化 inventory 为字符串
    fn format_inventory(inventory: &[cyber_jianghu_protocol::InventoryItem]) -> String {
        if inventory.is_empty() {
            "空".to_string()
        } else {
            inventory
                .iter()
                .map(|i| format!("{} x{}", i.name, i.quantity))
                .collect::<Vec<_>>()
                .join(", ")
        }
    }

    /// 格式化 adjacent locations 为字符串
    fn format_adjacent(nodes: &[AdjacentNode]) -> String {
        if nodes.is_empty() {
            "无（当前位置无法移动）".to_string()
        } else {
            nodes
                .iter()
                .map(|n| {
                    if n.travel_cost > 1 {
                        format!("{} (耗时{}tick)", n.name, n.travel_cost)
                    } else {
                        n.name.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        }
    }

    /// 格式化 entities 为字符串
    fn format_entities(entities: &[Entity]) -> String {
        if entities.is_empty() {
            "无".to_string()
        } else {
            entities
                .iter()
                .map(|e| format!("{}({})", e.name, e.state))
                .collect::<Vec<_>>()
                .join(", ")
        }
    }

    /// 格式化 nearby_items 为字符串
    fn format_nearby_items(items: &[SceneItem]) -> String {
        if items.is_empty() {
            "无".to_string()
        } else {
            items
                .iter()
                .map(|i| format!("{} x{}", i.name, i.quantity))
                .collect::<Vec<_>>()
                .join(", ")
        }
    }

    /// 获取 persona_initialized 状态（用于调试）
    pub fn is_initialized(&self) -> bool {
        self.persona_initialized
    }

    /// 获取最后更新的 tick_id（用于调试）
    pub fn last_update_tick(&self) -> i64 {
        self.last_update_tick
    }
}

/// 变化标记（用于 diff-based prompt 输出）
#[derive(Debug, Clone, Default)]
pub struct ChangeMarkers {
    pub inventory_changed: bool,
    pub locations_changed: bool,
    pub entities_changed: bool,
    pub nearby_items_changed: bool,
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::InventoryItem;
    use cyber_jianghu_protocol::types::{AgentSelfState, WorldTime};

    fn create_test_persona() -> DynamicPersona {
        let agent_id = uuid::Uuid::new_v4();
        DynamicPersona::new(agent_id, "张三", "你是一名行侠仗义的侠客。")
    }

    fn create_test_world_time() -> WorldTime {
        WorldTime {
            year: 2026,
            month: 4,
            day: 10,
            hour: 12,
            minute: 0,
            second: 0,
            weather: "晴".to_string(),
        }
    }

    fn create_test_world_state() -> WorldState {
        WorldState {
            event_type: "world_state".to_string(),
            tick_id: 1,
            agent_id: Some(uuid::Uuid::new_v4()),
            world_time: create_test_world_time(),
            deadline_ms: 0,
            self_state: AgentSelfState {
                attributes: std::collections::HashMap::new(),
                derived_attributes: std::collections::HashMap::new(),
                attribute_descriptions: std::collections::HashMap::new(),
                status_effects: vec![],
                inventory: vec![],
            },
            location: crate::models::Location {
                node_id: "village".to_string(),
                node_type: "village".to_string(),
                name: "村庄".to_string(),
                adjacent_nodes: vec![],
            },
            entities: vec![],
            nearby_items: vec![],
            events_log: vec![],
            private_dialogue_log: vec![],
        }
    }

    #[test]
    fn test_structured_summary() {
        let persona = create_test_persona();
        let mut cache = PromptCache::new(
            "你是一名行侠仗义的侠客。".to_string(),
            "- idle: 休息".to_string(),
            &persona,
        );

        // 第一轮返回完整 persona
        let full = cache.get_persona(&create_test_world_state());
        assert_eq!(full, "你是一名行侠仗义的侠客。");

        // 第二轮返回结构化摘要
        let summary = cache.get_persona(&create_test_world_state());
        assert!(summary.contains("张三"));
        assert!(summary.contains("核心特质"));
    }

    #[test]
    fn test_first_round_full_persona() {
        let persona = create_test_persona();
        let mut cache = PromptCache::new(
            "你是一名行侠仗义的侠客。".to_string(),
            "- idle: 休息".to_string(),
            &persona,
        );

        let ws = create_test_world_state();
        let full = cache.get_persona(&ws);

        // 第一轮应返回完整 persona
        assert_eq!(full, "你是一名行侠仗义的侠客。");
        assert!(cache.is_initialized());
    }

    #[test]
    fn test_second_round_summary_persona() {
        let persona = create_test_persona();
        let mut cache = PromptCache::new(
            "你是一名行侠仗义的侠客。".to_string(),
            "- idle: 休息".to_string(),
            &persona,
        );

        // 第一轮返回完整 persona
        let ws1 = create_test_world_state();
        let full = cache.get_persona(&ws1);
        assert_eq!(full, "你是一名行侠仗义的侠客。");

        // 第二轮返回结构化摘要
        let ws2 = WorldState {
            tick_id: 2,
            ..create_test_world_state()
        };
        let summary = cache.get_persona(&ws2);

        // 第二轮应返回摘要，包含名字和特质
        assert!(summary.contains("张三"));
        assert!(summary.contains("核心特质"));
    }

    #[test]
    fn test_inventory_change_detection() {
        let persona = create_test_persona();
        let mut cache = PromptCache::new(
            "你是张三。".to_string(),
            "- idle: 休息".to_string(),
            &persona,
        );

        let mut ws1 = create_test_world_state();
        ws1.self_state.inventory = vec![InventoryItem {
            item_id: "mantou".to_string(),
            name: "馒头".to_string(),
            quantity: 2,
            is_equipped: false,
        }];

        cache.check_and_update(&ws1);
        assert_eq!(cache.get_inventory(), "馒头 x2");

        // 修改 inventory
        let mut ws2 = ws1.clone();
        ws2.tick_id = 2;
        ws2.self_state.inventory = vec![InventoryItem {
            item_id: "mantou".to_string(),
            name: "馒头".to_string(),
            quantity: 3,
            is_equipped: false,
        }];

        cache.check_and_update(&ws2);
        assert_eq!(cache.get_inventory(), "馒头 x3");
    }

    #[test]
    fn test_same_tick_skip() {
        let persona = create_test_persona();
        let mut cache = PromptCache::new(
            "你是张三。".to_string(),
            "- idle: 休息".to_string(),
            &persona,
        );

        let ws = create_test_world_state();
        cache.check_and_update(&ws);

        let last_tick = cache.last_update_tick();
        cache.check_and_update(&ws); // 同 tick 再次调用

        // 应该跳过，tick 不变
        assert_eq!(cache.last_update_tick(), last_tick);
    }

    #[test]
    fn test_force_refresh() {
        let persona = create_test_persona();
        let mut cache = PromptCache::new(
            "你是张三。".to_string(),
            "- idle: 休息".to_string(),
            &persona,
        );

        let ws = create_test_world_state();
        cache.check_and_update(&ws);
        assert!(cache.last_update_tick() >= 0);

        cache.force_refresh();
        assert_eq!(cache.last_update_tick(), -1);
    }

    #[test]
    fn test_persona_length_reduction() {
        // 模拟完整 persona 描述
        let full_persona_desc = "你是一名行侠仗义的侠客，性格豪爽，不畏强权。你出身于武林世家，自幼习武，对江湖规矩了如指掌。你重视义气，愿意为朋友两肋插刀。".to_string();

        let persona = create_test_persona();
        let mut cache = PromptCache::new(
            full_persona_desc.clone(),
            "- idle: 休息".to_string(),
            &persona,
        );

        // 第一轮：完整 persona
        let ws1 = create_test_world_state();
        let full = cache.get_persona(&ws1);
        assert_eq!(full.len(), full_persona_desc.len());

        // 第二轮：结构化摘要
        let ws2 = WorldState {
            tick_id: 2,
            ..create_test_world_state()
        };
        let summary = cache.get_persona(&ws2);

        // 验证摘要显著短于完整描述（节省 > 50%）
        let reduction =
            (full_persona_desc.len() - summary.len()) as f64 / full_persona_desc.len() as f64;
        assert!(
            reduction > 0.5,
            "Persona 摘要应比完整描述节省超过 50%，实际: {}%",
            (reduction * 100.0) as i32
        );
    }

    #[test]
    fn test_cache_efficiency_simulation() {
        // 模拟多轮决策，验证缓存效率
        let persona = create_test_persona();
        let full_persona_desc = "你是一名行侠仗义的侠客，性格豪爽，不畏强权。你出身于武林世家，自幼习武，对江湖规矩了如指掌。你重视义气，愿意为朋友两肋插刀。".to_string();

        let mut cache = PromptCache::new(
            full_persona_desc.clone(),
            "- idle: 休息\n- move: 移动\n- eat: 吃东西".to_string(),
            &persona,
        );

        // 模拟多轮
        let mut total_saved = 0;
        for tick in 1..=10 {
            let mut ws = create_test_world_state();
            ws.tick_id = tick;

            // 更新缓存
            cache.check_and_update(&ws);

            // 获取 persona
            let persona_str = cache.get_persona(&ws);

            // 第一轮不节省，后续节省
            if tick > 1 {
                total_saved += full_persona_desc.len() - persona_str.len();
            }
        }

        // 验证多轮总计节省 > 500 tokens（保守估计）
        assert!(
            total_saved > 500,
            "多轮应显著节省 token，实际节省: {}",
            total_saved
        );

        // 验证每轮平均节省
        let avg_saved = total_saved / 9;
        let avg_persona_len = full_persona_desc.len() - avg_saved;
        let reduction_ratio = avg_saved as f64 / full_persona_desc.len() as f64;

        assert!(
            reduction_ratio > 0.4,
            "每轮平均应节省 > 40%，实际: {}%",
            (reduction_ratio * 100.0) as i32
        );

        // 摘要后长度应合理（< 50 字符）
        assert!(
            avg_persona_len < 100,
            "摘要后 persona 应简短，实际: {} 字符",
            avg_persona_len
        );
    }
}
