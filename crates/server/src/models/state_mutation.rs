// ============================================================================
// AgentState 状态变更方法
// ============================================================================

use evalexpr::ContextWithMutableVariables;
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

    /// 获取季节对指定属性的修饰系数（数据驱动）
    ///
    /// 从 time.json 的季节配置中获取 attribute_modifiers
    /// 返回 1.0 表示无修饰，>1.0 表示增加，<1.0 表示减少
    fn get_season_modifier(&self, attr_name: &str, tick_id: i64) -> f32 {
        if let Some(season) = crate::game_data::registry::TimeRegistry::get_current_season(tick_id)
        {
            // 从季节配置中获取该属性的修饰系数
            if let Some(&modifier) = season.attribute_modifiers.get(attr_name) {
                return modifier;
            }
        }
        1.0 // 默认无修饰
    }

    /// 应用生理值衰减（委托给 StatusComponent）
    ///
    /// 处理两类属性变化：
    /// 1. decay_per_tick: 衰减值（正值表示扣减，如 hunger 每tick扣减5）
    /// 2. recovery_formula: 恢复公式（如 stamina 每tick恢复 5 + constitution * 0.1）
    ///
    /// 季节修饰系数从 time.json 的季节配置中读取（数据驱动）
    ///
    /// 返回值：如果Agent死亡，返回 Some(attr_name) 表示触发死亡的属性名；否则返回 None
    pub fn apply_decay(&mut self, tick_id: i64) -> Option<String> {
        if !self.is_alive {
            return None;
        }

        let context = self.get_formula_context();

        // 1. 处理衰减属性
        // decay_per_tick 表示扣减量（正值=扣减量，如 hunger 每tick扣减5）
        let attributes_to_decay = self.status.get_decaying_attributes();

        for (attr_name, decay_amount) in attributes_to_decay {
            // decay_per_tick 是扣减量，需要取负值作为 delta
            let base_delta = -decay_amount;

            // 获取季节修饰系数（数据驱动）
            let season_modifier = self.get_season_modifier(&attr_name, tick_id);
            let delta = (base_delta * season_modifier).floor() as i32;

            // 记录衰减前的值
            let before_value = self.status.get(&attr_name).unwrap_or(-1);

            debug!(
                "Applying decay to {}: decay_amount={}, season_modifier={}, delta={}, before_value={}",
                attr_name, decay_amount, season_modifier, delta, before_value
            );

            if let Ok(new_val) = self.status.apply_change(&attr_name, delta, &context) {
                debug!(
                    "Applied decay to {}: before={}, delta={}, after={}",
                    attr_name, before_value, delta, new_val
                );
                // 检查是否触发死亡条件
                if self.status.check_death_condition(&attr_name) {
                    self.is_alive = false;
                    let _ = self.status.set("hp", 0);
                    tracing::warn!(
                        "Agent {} 因 {} 归零而死亡 (Tick: {})",
                        self.agent_id,
                        attr_name,
                        tick_id
                    );
                    // 返回触发死亡的属性名
                    return Some(attr_name);
                }
            }
        }

        // 如果已死亡，不再处理恢复
        if !self.is_alive {
            return None;
        }

        // 2. 处理恢复属性 (recovery_formula)
        let recovering_attributes = self.status.get_recovering_attributes();

        for (attr_name, formula) in recovering_attributes {
            // 使用 evalexpr 计算恢复值
            let mut eval_context = evalexpr::HashMapContext::<evalexpr::DefaultNumericTypes>::new();
            for (k, v) in &context {
                let _ = eval_context.set_value(k.clone(), evalexpr::Value::Int(*v as i64));
            }

            // 计算基础恢复值
            let base_recovery = if let Ok(evalexpr::Value::Float(recovery)) =
                evalexpr::eval_with_context(&formula, &eval_context)
            {
                recovery.floor() as i32
            } else if let Ok(evalexpr::Value::Int(recovery)) =
                evalexpr::eval_with_context(&formula, &eval_context)
            {
                recovery as i32
            } else {
                continue;
            };

            if base_recovery > 0 {
                // 获取季节修饰系数（数据驱动）
                let season_modifier = self.get_season_modifier(&attr_name, tick_id);
                let delta = (base_recovery as f32 * season_modifier).floor() as i32;

                if delta > 0 {
                    let before_value = self.status.get(&attr_name).unwrap_or(-1);
                    debug!(
                        "Applying recovery to {}: formula={}, base_recovery={}, season_modifier={}, delta={}, before_value={}",
                        attr_name, formula, base_recovery, season_modifier, delta, before_value
                    );

                    if let Ok(new_val) = self.status.apply_change(&attr_name, delta, &context) {
                        debug!(
                            "Applied recovery to {}: before={}, delta={}, after={}",
                            attr_name, before_value, delta, new_val
                        );
                    }
                }
            }
        }

        None
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
        if self.status.check_death_conditions().is_some() {
            self.is_alive = false;
        }
    }

    /// 获取所有属性用于协议序列化（从组件转换为 HashMap）
    ///
    /// 将组件化的属性转换为 HashMap 格式，用于 WebSocket 传输
    pub fn get_attributes_for_protocol(&self) -> HashMap<String, i32> {
        let mut attributes = HashMap::new();
        let context = self.get_formula_context();

        // 从 StatusComponent 收集所有状态属性
        for (name, attr) in &self.status.collection.attributes {
            attributes.insert(name.clone(), attr.value.get());

            // 顺便提供上限值
            let max_value = crate::game_data::types::StatusComponent::evaluate_max_value(
                &attr.metadata.max_value_formula,
                255.0,
                &context,
            );
            attributes.insert(format!("{}_max", name), max_value as i32);
        }

        // 从 AttributeComponent 收集所有先天属性
        for (name, attr) in &self.primary_attributes.collection.attributes {
            attributes.insert(name.clone(), attr.value.get());

            // 提供先天属性的极限值（如果是可成长属性）
            if let crate::game_data::types::attributes::AttributeValue::Growable { base, .. } =
                &attr.value
            {
                attributes.insert(format!("{}_max", name), *base as i32);
            }
        }

        attributes
    }

    /// 获取派生属性用于协议序列化（浮点数）
    ///
    /// 计算派生属性（如闪避率、暴击率等）并返回 f32 HashMap
    pub fn get_derived_attributes_for_protocol(&self) -> HashMap<String, f32> {
        let mut derived_attributes = HashMap::new();
        let context = self.get_formula_context();

        if let Some(config) = crate::game_data::registry::StateRegistry::get_attributes_config() {
            for (name, attr_def) in &config.data.derived.attributes {
                if let Some(formula) = &attr_def.formula {
                    let value = crate::game_data::types::StatusComponent::evaluate_max_value(
                        &Some(formula.clone()),
                        attr_def.default_value.unwrap_or(0.0) as f32,
                        &context,
                    );
                    derived_attributes.insert(name.clone(), value);
                }
            }
        }

        derived_attributes
    }
}
