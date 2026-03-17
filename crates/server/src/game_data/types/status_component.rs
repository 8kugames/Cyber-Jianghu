// ============================================================================
// OpenClaw Cyber-Jianghu 状态值组件
// ============================================================================
//
// HP、体力、饥饿、口渴、内力、理智、声望、银两
// ============================================================================

use crate::game_data::types::attributes::{
    Attribute, AttributeCollection, AttributeMetadata, AttributeType,
    AttributeValue,
};
use crate::game_data::types::attributes_config::AttributesConfig;
use crate::game_data::types::unified_attributes::UnifiedAttributesConfig;
use serde::{Deserialize, Serialize};

/// 状态值组件
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StatusComponent {
    /// 属性集合
    pub collection: AttributeCollection,
}

use evalexpr::ContextWithMutableVariables;

impl StatusComponent {
    /// 从配置创建状态值组件（数据驱动）
    pub fn from_config(config: &AttributesConfig) -> Self {
        let mut collection = AttributeCollection::new_collection();

        for (attr_name, attr_def) in &config.attributes {
            let metadata = AttributeMetadata {
                name: attr_name.clone(),
                display_name: attr_def.display_name.clone(),
                description: attr_def.description.clone(),
                attr_type: AttributeType::Status,
                birth_range: None,
                initial_value: None,
                growth_rate: None,
                affects: vec![],
                decay_per_tick: attr_def.decay_per_tick_as_i32(),
                death_condition: attr_def.death_condition.clone(),
                formula: attr_def.formula.clone(),
                default_value: attr_def.default_value_as_i32(),
                min_value: attr_def.min_value_as_i32(),
                max_value_formula: attr_def.max_value_as_i32().map(|v| v.to_string()),
                recovery_formula: attr_def.recovery_formula.clone(),
                primary_attribute_deps: attr_def.primary_attribute_deps.clone().unwrap_or_default(),
            };

            let attribute = Attribute::from_config(&metadata);
            collection.add(attribute);
        }

        Self { collection }
    }

    /// 获取属性值
    pub fn get(&self, name: &str) -> Option<i32> {
        self.collection.get_value(name)
    }

    /// 设置属性值
    pub fn set(&mut self, name: &str, value: i32) -> Result<(), String> {
        self.collection.set_value(name, value)
    }

    /// 应用生理值衰减
    ///
    /// 返回触发死亡条件的属性名称列表
    pub fn apply_decay(
        &mut self,
        _tick_count: i64,
        _context: &std::collections::HashMap<String, i32>,
    ) -> Vec<String> {
        // 这个方法不再被 state_mutation.rs 直接调用，而是通过 get_decaying_attributes 手动迭代
        // 主要是为了在外面可以插入季节系数逻辑
        // 保留该方法以防其他地方调用
        vec![]
    }

    /// 获取需要衰减的属性列表及其衰减量
    pub fn get_decaying_attributes(&self) -> Vec<(String, i32)> {
        let mut result = Vec::new();
        for (name, attr) in &self.collection.attributes {
            if let Some(decay) = attr.metadata.decay_per_tick {
                if decay != 0 {
                    result.push((name.clone(), decay));
                }
            }
        }
        
        // Ensure consistent ordering for tests (e.g. hunger, thirst, stamina)
        // Tests rely on specific execution order if checking death conditions.
        // Sort by decay amount, then name to ensure predictable order.
        result.sort_by(|a, b| {
            if a.1 == b.1 {
                a.0.cmp(&b.0)
            } else {
                b.1.cmp(&a.1) // Higher decay first
            }
        });
        
        result
    }

    /// 检查死亡条件
    pub fn check_death_conditions(&self) -> Option<String> {
        for attr in self.collection.attributes.values() {
            if let Some(death_condition) = &attr.metadata.death_condition {
                if death_condition.check_int(attr.value.get()) {
                    return Some(format!(
                        "Death condition met for attribute '{}'",
                        attr.metadata.name
                    ));
                }
            }
        }
        None
    }

    /// 从统一配置创建状态值组件
    pub fn from_unified_config(config: &UnifiedAttributesConfig) -> Self {
        let mut collection = AttributeCollection::new_collection();

        for (attr_name, attr_def) in &config.data.status.attributes {
            let metadata = AttributeMetadata {
                name: attr_name.clone(),
                display_name: attr_def.display_name.clone(),
                description: attr_def.description.clone(),
                attr_type: AttributeType::Status,
                birth_range: None,
                initial_value: None,
                growth_rate: None,
                affects: vec![],
                decay_per_tick: attr_def.decay_per_tick.map(|v| v as i32),
                death_condition: attr_def.death_condition.clone(),
                formula: attr_def.formula.clone(),
                default_value: attr_def.default_value.map(|v| v as i32),
                min_value: attr_def.min_value.map(|v| v as i32),
                max_value_formula: attr_def.max_value_formula.clone(),
                recovery_formula: attr_def.recovery_formula.clone(),
                primary_attribute_deps: attr_def.primary_attribute_deps.clone().unwrap_or_default(),
            };

            let initial_val = attr_def.default_value.map(|v| v as i32).unwrap_or(0);
            collection.add(Attribute {
                value: AttributeValue::Static {
                    value: initial_val as u8,
                },
                metadata,
            });
        }

        Self { collection }
    }

    /// 辅助方法：解析最大值公式
    fn evaluate_max_value(
        formula: &Option<String>,
        default_max: i32,
        context: &std::collections::HashMap<String, i32>,
    ) -> i32 {
        if let Some(f) = formula {
            let mut eval_context = evalexpr::HashMapContext::<evalexpr::DefaultNumericTypes>::new();
            for (k, v) in context {
                let _ = eval_context.set_value(k.clone(), evalexpr::Value::Int(*v as i64));
            }
            let res = evalexpr::eval_with_context(f, &eval_context);
            if let Ok(evalexpr::Value::Int(result)) = res {
                return result as i32;
            } else if let Ok(evalexpr::Value::Float(result)) = res {
                return result as i32;
            } else if let Ok(parsed) = f.parse::<i32>() {
                return parsed;
            }
        }
        default_max
    }

    /// 应用属性值变化（带范围限制）
    ///
    /// 返回 Ok(new_value) 表示成功，Err(error_value) 表示属性不存在
    pub fn apply_change(
        &mut self,
        name: &str,
        delta: i32,
        context: &std::collections::HashMap<String, i32>,
    ) -> Result<i32, i32> {
        // 先检查属性是否存在
        if !self.collection.attributes.contains_key(name) {
            return Err(0);
        }

        let attr = self.collection.attributes.get_mut(name).unwrap();

        let current = attr.value.get();
        let min_value = attr.metadata.min_value.unwrap_or(0);
        let max_value = Self::evaluate_max_value(&attr.metadata.max_value_formula, 255, context);

        let new_value = (current + delta).clamp(min_value, max_value);
        attr.value.set(new_value);
        Ok(new_value)
    }

    /// 获取属性的每tick衰减值
    pub fn decay_per_tick(&self, name: &str) -> Option<i32> {
        self.collection
            .attributes
            .get(name)
            .and_then(|attr| attr.metadata.decay_per_tick)
    }

    /// 检查指定属性是否满足死亡条件
    pub fn check_death_condition(&self, name: &str) -> bool {
        self.collection
            .attributes
            .get(name)
            .and_then(|attr| {
                attr.metadata
                    .death_condition
                    .as_ref()
                    .map(|cond| cond.check_int(attr.value.get()))
            })
            .unwrap_or(false)
    }
}
