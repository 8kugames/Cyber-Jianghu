// ============================================================================
// OpenClaw Cyber-Jianghu 状态值组件
// ============================================================================
//
// HP、体力、饥饿、口渴、内力、理智、声望、银两
// ============================================================================

use crate::game_data::types::attributes::{
    Attribute, AttributeCollection, AttributeMetadata, AttributeType, AttributeValue,
};

pub const DEFAULT_STATUS_MAX_VALUE: f32 = 255.0;
use crate::game_data::types::attributes_config::AttributesConfig;
use crate::game_data::types::unified_attributes::UnifiedAttributesConfig;
use serde::{Deserialize, Serialize};

/// 状态值组件
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StatusComponent {
    /// 属性集合
    pub collection: AttributeCollection,

    /// 属性上限永久修正值（attribute_name → bonus）
    /// 叠加在 max_value_formula 计算结果之上，持久化到 DB
    #[serde(default)]
    pub max_modifiers: std::collections::HashMap<String, i32>,
}

impl StatusComponent {
    /// 从配置创建状态值组件（数据驱动）
    #[allow(dead_code)]
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
                decay_per_tick: attr_def.decay_per_tick_as_f32(),
                death_condition: attr_def.death_condition.clone(),
                formula: attr_def.formula.clone(),
                default_value: attr_def.default_value_as_f32(),
                min_value: attr_def.min_value_as_f32(),
                max_value_formula: attr_def.max_value_as_i32().map(|v| v.to_string()),
                recovery_formula: attr_def.recovery_formula.clone(),
                primary_attribute_deps: attr_def.primary_attribute_deps.clone().unwrap_or_default(),
            };

            let attribute = Attribute::from_config(&metadata);
            collection.add(attribute);
        }

        Self {
            collection,
            max_modifiers: std::collections::HashMap::new(),
        }
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
    #[allow(dead_code)]
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
    ///
    /// 注意：decay_per_tick 表示衰减（扣减），正值表示减少量
    pub fn get_decaying_attributes(&self) -> Vec<(String, f32)> {
        let mut result = Vec::new();
        for (name, attr) in &self.collection.attributes {
            if let Some(decay) = attr.metadata.decay_per_tick
                && decay != 0.0
            {
                result.push((name.clone(), decay));
            }
        }

        // Ensure consistent ordering for tests (e.g. hunger, thirst, stamina)
        // Tests rely on specific execution order if checking death conditions.
        // Sort by decay amount, then name to ensure predictable order.
        result.sort_by(|a, b| {
            if a.1 == b.1 {
                a.0.cmp(&b.0)
            } else {
                b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal) // Higher decay first
            }
        });

        result
    }

    /// 获取需要恢复的属性列表及其恢复公式
    ///
    /// 这些属性有 recovery_formula 但没有 decay_per_tick（或 decay_per_tick 为 0）
    pub fn get_recovering_attributes(&self) -> Vec<(String, String)> {
        let mut result = Vec::new();
        for (name, attr) in &self.collection.attributes {
            if let Some(formula) = &attr.metadata.recovery_formula {
                // 只有当 decay_per_tick 为 0 或不存在时，才使用 recovery_formula
                let decay = attr.metadata.decay_per_tick.unwrap_or(0.0);
                if decay == 0.0 {
                    result.push((name.clone(), formula.clone()));
                }
            }
        }
        result
    }

    /// 检查死亡条件
    pub fn check_death_conditions(&self) -> Option<String> {
        for attr in self.collection.attributes.values() {
            if let Some(death_condition) = &attr.metadata.death_condition
                && death_condition.check_int(attr.value.get())
            {
                return Some(format!(
                    "Death condition met for attribute '{}'",
                    attr.metadata.name
                ));
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
                decay_per_tick: attr_def.decay_per_tick.map(|v| v as f32),
                death_condition: attr_def.death_condition.clone(),
                formula: attr_def.formula.clone(),
                default_value: attr_def.default_value.map(|v| v as f32),
                min_value: attr_def.min_value.map(|v| v as f32),
                max_value_formula: attr_def.max_value_formula.clone(),
                recovery_formula: attr_def.recovery_formula.clone(),
                primary_attribute_deps: attr_def.primary_attribute_deps.clone().unwrap_or_default(),
            };

            let initial_val = attr_def.default_value.map(|v| v as f32).unwrap_or(0.0) as i32;
            collection.add(Attribute {
                value: AttributeValue::Static {
                    value: initial_val as u8,
                },
                metadata,
            });
        }

        Self {
            collection,
            max_modifiers: std::collections::HashMap::new(),
        }
    }

    /// 辅助方法：解析最大值公式
    pub fn evaluate_max_value(
        formula: &Option<String>,
        default_max: f32,
        context: &std::collections::HashMap<String, i32>,
    ) -> f32 {
        let i64_context: std::collections::HashMap<String, i64> = context
            .iter()
            .map(|(k, v)| (k.clone(), *v as i64))
            .collect();
        let engine = crate::game_data::formula_engine::FormulaEngine::new();
        engine.evaluate_max(formula, default_max, &i64_context)
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
        let min_value = attr.metadata.min_value.unwrap_or(0.0) as i32;
        let max_value = Self::evaluate_max_value(&attr.metadata.max_value_formula, DEFAULT_STATUS_MAX_VALUE, context)
            as i32
            + self.max_modifiers.get(name).copied().unwrap_or(0);

        let new_value = (current + delta).clamp(min_value, max_value);
        attr.value.set(new_value);
        Ok(new_value)
    }

    /// 获取属性的每tick衰减值
    #[allow(dead_code)]
    pub fn decay_per_tick(&self, name: &str) -> Option<f32> {
        self.collection
            .attributes
            .get(name)
            .and_then(|attr| attr.metadata.decay_per_tick)
    }

    /// 永久提升属性上限
    ///
    /// 将 delta 叠加到 max_modifiers 中，并立即将 current 提升至新上限
    pub fn apply_max_change(&mut self, name: &str, delta: i32) -> Result<i32, i32> {
        if !self.collection.attributes.contains_key(name) {
            return Err(0);
        }

        *self.max_modifiers.entry(name.to_string()).or_insert(0) += delta;

        // 同时将当前值提升 delta（修炼获得的上限提升应立即生效）
        let attr = self.collection.attributes.get_mut(name).unwrap();
        let current = attr.value.get();
        let new_current = current + delta;
        attr.value.set(new_current);
        Ok(new_current)
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
