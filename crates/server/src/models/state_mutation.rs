// ============================================================================
// AgentState 状态变更方法
// ============================================================================

use std::collections::HashMap;
use tracing::debug;

use super::AgentState;

impl AgentState {
    /// 获取公式计算上下文
    pub fn get_formula_context(&self) -> std::collections::HashMap<String, i32> {
        let mut context = std::collections::HashMap::new();
        // 添加状态属性
        for (name, attr) in &self.status.collection.attributes {
            context.insert(name.clone(), attr.value.get());
        }
        // 添加先天属性
        for (name, attr) in &self.primary_attributes.collection.attributes {
            context.insert(name.clone(), attr.value.get());
        }
        context
    }

    /// 应用生理值衰减（委托给 StatusComponent）
    ///
    /// 对所有配置了 decay_per_tick 的属性应用衰减
    pub fn apply_decay(&mut self, tick_id: i64) {
        if !self.is_alive {
            return;
        }

        // 获取季节系数
        let mut modifier = 1.0;
        if let Some(season) = crate::game_data::registry::TimeRegistry::get_current_season(tick_id) {
            if season.temperature_modifier < 0 {
                modifier = 1.5; // 冬季增加消耗
            } else if season.temperature_modifier > 10 {
                modifier = 1.2; // 夏季也增加一些消耗
            }
        }

        let context = self.get_formula_context();
        let attributes_to_decay = self.status.get_decaying_attributes();

        for (attr_name, decay_amount) in attributes_to_decay {
            // let mut base_delta = -decay_amount;
            // The tests expect the attribute to DECREASE by decay_amount.
            // If decay_amount is 5, it means we want to subtract 5.
            // In apply_change, a negative delta means decrease.
            // Wait, what if decay_amount in attributes.json is -5?
            // "decay_per_tick": 5 means it DECAYS by 5. So delta should be -5.
            let mut base_delta = -decay_amount;
            
            if attr_name == "hunger" || attr_name == "thirst" {
                if base_delta < 0 {
                    base_delta = (base_delta as f32 * modifier).floor() as i32;
                } else {
                    base_delta = (base_delta as f32 * modifier).ceil() as i32;
                }
            }
            
            debug!("Applying decay to {}: decay_amount={}, base_delta={}", attr_name, decay_amount, base_delta);
            
            if let Ok(_new_val) = self.status.apply_change(&attr_name, base_delta, &context) {
                // 检查是否触发死亡条件
                if self.status.check_death_condition(&attr_name) {
                    self.is_alive = false;
                    let _ = self.status.set("hp", 0);
                    tracing::warn!("Agent {} 因 {} 归零而死亡 (Tick: {})", self.agent_id, attr_name, tick_id);
                    break;
                }
            }
        }
    }

    /// 恢复属性值（通用方法，委托给 StatusComponent）
    ///
    /// 使用物品恢复某个属性
    ///
    /// 如果属性不存在，变更会被拒绝，保持原始值不变
    pub fn restore_attribute(&mut self, attr_name: &str, amount: i32) {
        if !self.is_alive {
            return;
        }
        let context = self.get_formula_context();
        // 使用 StatusComponent 的 apply_change 方法（带范围限制）
        let _ = self.status.apply_change(attr_name, amount, &context);
    }

    /// 受到伤害
    ///
    /// HP减少，归零后死亡
    pub fn take_damage(&mut self, damage: i32) {
        if !self.is_alive {
            return;
        }
        self.restore_attribute("hp", -damage);

        // 检查死亡条件（通过组件）
        if let Some(_) = self.status.check_death_conditions() {
            self.is_alive = false;
        }
    }

    /// 获取所有属性用于协议序列化（从组件转换为 HashMap）
    ///
    /// 将组件化的属性转换为 HashMap 格式，用于 WebSocket 传输
    pub fn get_attributes_for_protocol(&self) -> HashMap<String, i32> {
        let mut attributes = HashMap::new();

        // 从 StatusComponent 收集所有状态属性
        for (name, attr) in &self.status.collection.attributes {
            attributes.insert(name.clone(), attr.value.get());
        }

        // 从 AttributeComponent 收集所有先天属性
        for (name, attr) in &self.primary_attributes.collection.attributes {
            attributes.insert(name.clone(), attr.value.get());
        }

        attributes
    }
}
