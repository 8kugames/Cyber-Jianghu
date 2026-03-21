// ============================================================================
// OpenClaw Cyber-Jianghu 先天属性组件
// ============================================================================
//
// 可成长属性：力量、敏捷、根骨、悟性
// 静态属性：魅力
// 每日随机属性：福缘
// ============================================================================

use crate::game_data::types::attributes::{
    Attribute, AttributeCollection, AttributeMetadata, AttributeType, AttributeValue,
};
use crate::game_data::types::primary_attributes::{
    PRIMARY_ATTR_DAILY_RANDOM, PRIMARY_ATTR_GROWABLE, PRIMARY_ATTR_STATIC, PrimaryAttributesConfig,
};
use crate::game_data::types::unified_attributes::UnifiedAttributesConfig;
use serde::{Deserialize, Serialize};

/// 先天属性组件
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AttributeComponent {
    /// 属性集合
    pub collection: AttributeCollection,
}

impl AttributeComponent {
    /// 从配置创建先天属性组件（数据驱动）
    #[allow(dead_code)]
    pub fn from_config(config: &PrimaryAttributesConfig) -> Self {
        let mut collection = AttributeCollection::new_collection();

        for attr_def in config.attributes.values() {
            // 使用字符串比较而非枚举模式匹配
            let attr_type = match attr_def.type_name.as_str() {
                PRIMARY_ATTR_GROWABLE => AttributeType::Growable,
                PRIMARY_ATTR_STATIC => AttributeType::Static,
                PRIMARY_ATTR_DAILY_RANDOM => AttributeType::DailyRandom,
                _ => AttributeType::Static, // 未知类型默认为静态
            };

            let metadata = AttributeMetadata {
                name: attr_def.name.clone(),
                display_name: attr_def.display_name.clone(),
                description: attr_def.description.clone(),
                attr_type,
                birth_range: attr_def
                    .birth_range
                    .map(|(min, max)| (min as u8, max as u8)),
                initial_value: attr_def.initial_value.map(|v| v as u8),
                growth_rate: attr_def.growth_rate.map(|v| v as f32),
                affects: attr_def.affects.clone(),
                decay_per_tick: None,
                death_condition: None,
                formula: None,
                default_value: None,
                min_value: None,
                max_value_formula: None,
                recovery_formula: None,
                primary_attribute_deps: vec![],
            };

            let attribute = Attribute::from_config(&metadata);
            collection.add(attribute);
        }

        Self { collection }
    }

    /// 获取属性
    #[allow(dead_code)]
    pub fn get(&self, name: &str) -> Option<&Attribute> {
        self.collection.get(name)
    }

    /// 获取属性值
    #[allow(dead_code)]
    pub fn get_value(&self, name: &str) -> Option<i32> {
        self.collection.get_value(name)
    }

    /// 训练属性（对于可成长属性）
    #[allow(dead_code)]
    pub fn train(&mut self, name: &str, amount: i32) -> Result<(), String> {
        let attr = self
            .collection
            .get_mut(name)
            .ok_or_else(|| format!("Attribute '{}' not found", name))?;

        match &mut attr.value {
            AttributeValue::Growable { current, base } => {
                let max_limit = attr
                    .metadata
                    .birth_range
                    .map(|(_, max)| max as i32)
                    .unwrap_or(255);
                // 确保不超过 base 或配置的最大范围
                let actual_max = (*base as i32).max(max_limit);
                *current = (*current as i32 + amount).min(actual_max) as u8;
                Ok(())
            }
            _ => Err(format!("Attribute '{}' is not growable", name)),
        }
    }

    /// 属性突破（突破当前上限）
    #[allow(dead_code)]
    pub fn breakthrough(&mut self, name: &str) -> Result<i32, String> {
        let attr = self
            .collection
            .get_mut(name)
            .ok_or_else(|| format!("Attribute '{}' not found", name))?;

        match &mut attr.value {
            AttributeValue::Growable { current, base, .. } => {
                // 突破机制：当前基础值 + 固定比例或数值，这里使用 10% 提升或固定 10
                let increase = (*base as i32 / 10).max(1);
                let new_max = (*base as i32 + increase).min(255);
                *base = new_max as u8;
                *current = new_max as u8;
                Ok(new_max)
            }
            _ => Err(format!("Attribute '{}' is not growable", name)),
        }
    }

    /// 刷新每日随机属性（如福缘）
    #[allow(dead_code)]
    pub fn refresh_daily(&mut self) {
        use rand::RngExt;
        let mut rng = rand::rng();

        for attr in self.collection.attributes.values_mut() {
            if matches!(attr.value, AttributeValue::DailyRandom { .. })
                && let Some((min, max)) = attr.metadata.birth_range {
                    attr.value = AttributeValue::DailyRandom {
                        value: rng.random_range(min..=max),
                        range: (min, max),
                    };
                }
        }
    }

    /// 从统一配置创建先天属性组件（数据驱动）
    pub fn from_unified_config(config: &UnifiedAttributesConfig) -> Self {
        let mut collection = AttributeCollection::new_collection();

        for (attr_name, attr_def) in &config.data.primary.attributes {
            // 使用字符串比较而非枚举模式匹配
            let attr_type = match attr_def.type_name.as_str() {
                PRIMARY_ATTR_GROWABLE => AttributeType::Growable,
                PRIMARY_ATTR_STATIC => AttributeType::Static,
                PRIMARY_ATTR_DAILY_RANDOM => AttributeType::DailyRandom,
                _ => AttributeType::Static, // 未知类型默认为静态
            };

            let metadata = AttributeMetadata {
                name: attr_name.clone(),
                display_name: attr_def.display_name.clone(),
                description: attr_def.description.clone(),
                attr_type,
                birth_range: attr_def
                    .birth_range
                    .map(|(min, max)| (min as u8, max as u8)),
                initial_value: attr_def.initial_value.map(|v| v as u8),
                growth_rate: attr_def.growth_rate.map(|v| v as f32),
                affects: attr_def.affects.clone(),
                decay_per_tick: None,
                death_condition: None,
                formula: None,
                default_value: None,
                min_value: None,
                max_value_formula: None,
                recovery_formula: None,
                primary_attribute_deps: vec![],
            };

            let value = match metadata.attr_type {
                AttributeType::Growable => {
                    // 可成长属性（力量、敏捷、根骨、悟性）：
                    // - birth_range: 随机产出成长极限（上限）
                    // - initial_value: 进入游戏时的初始属性值
                    let growth_limit = metadata
                        .birth_range
                        .map(|(min, max)| {
                            use rand::RngExt;
                            let mut rng = rand::rng();
                            rng.random_range(min..=max) as i32
                        })
                        .unwrap_or(50);
                    let initial = metadata.initial_value.unwrap_or(10) as i32;
                    AttributeValue::Growable {
                        base: growth_limit as u8,  // 随机生成的成长极限
                        current: initial as u8,    // 固定的初始值
                    }
                }
                AttributeType::Static => {
                    // 静态属性（魅力）：出生随机，之后固定
                    let val = metadata
                        .birth_range
                        .map(|(min, max)| {
                            use rand::RngExt;
                            let mut rng = rand::rng();
                            rng.random_range(min..=max)
                        })
                        .unwrap_or(10);
                    AttributeValue::Static { value: val }
                }
                AttributeType::DailyRandom => {
                    // 每日随机属性（福缘）：每游戏日随机刷新
                    let val = metadata
                        .birth_range
                        .map(|(min, max)| {
                            use rand::RngExt;
                            let mut rng = rand::rng();
                            rng.random_range(min..=max)
                        })
                        .unwrap_or(10);
                    AttributeValue::DailyRandom {
                        value: val,
                        range: metadata.birth_range.unwrap_or((10, 10)),
                    }
                }
                _ => AttributeValue::Growable {
                    base: 50,
                    current: 10,
                },
            };

            collection.add(Attribute { value, metadata });
        }

        Self { collection }
    }
}
