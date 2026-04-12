// ============================================================================
// OpenClaw Cyber-Jianghu 派生属性组件
// ============================================================================
//
// 基于先天属性实时计算的派生属性（如物理伤害、闪避率等）
// ============================================================================

use crate::game_data::formula_engine::FormulaEngine;
use crate::game_data::types::attributes::AttributeMetadata;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 派生属性组件（基于先天属性实时计算）
#[derive(Debug, Clone, Deserialize, Serialize)]
#[allow(dead_code)]
pub struct DerivedAttributeComponent {
    /// 派生属性定义
    #[serde(skip)]
    pub definitions: HashMap<String, AttributeMetadata>,

    /// 缓存（用于避免重复计算）
    #[serde(skip)]
    pub cache: HashMap<String, f32>,
}

#[allow(dead_code)]
impl DerivedAttributeComponent {
    /// 从配置创建派生属性组件
    pub fn from_config(config: &HashMap<String, AttributeMetadata>) -> Self {
        let definitions = config.clone();
        Self {
            definitions,
            cache: HashMap::new(),
        }
    }

    /// 计算单个派生属性
    pub fn calculate(
        &self,
        name: &str,
        formula_engine: &FormulaEngine,
        context: &HashMap<String, f64>,
    ) -> Result<f32, String> {
        if let Some(&cached) = self.cache.get(name) {
            return Ok(cached);
        }

        let def = self
            .definitions
            .get(name)
            .ok_or_else(|| format!("Derived attribute '{}' not found", name))?;

        if let Some(formula) = &def.formula {
            let result = formula_engine
                .evaluate(formula, context)
                .map_err(|e| format!("Formula evaluation error: {}", e))?;
            Ok(result as f32)
        } else {
            Err(format!(
                "No formula defined for derived attribute '{}'",
                name
            ))
        }
    }

    /// 计算所有派生属性
    pub fn calculate_all(
        &mut self,
        formula_engine: &FormulaEngine,
        context: &HashMap<String, f64>,
    ) -> HashMap<String, f32> {
        let mut results = HashMap::new();

        for name in self.definitions.keys() {
            if let Ok(value) = self.calculate(name, formula_engine, context) {
                results.insert(name.clone(), value);
                self.cache.insert(name.clone(), value);
            }
        }

        results
    }

    /// 清空缓存（当主属性变化时调用）
    pub fn invalidate_cache(&mut self) {
        self.cache.clear();
    }
}
