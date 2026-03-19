//! 属性系统相关类型
//!
//! 完全数据驱动的属性系统（Data-Driven + COI）

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 属性集合（完全数据驱动）
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct AttributeCollection {
    /// 所有属性存储在 HashMap 中（零硬编码）
    pub attributes: HashMap<String, Attribute>,
}

/// 统一的属性类型
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Attribute {
    /// 属性值（支持多种类型）
    pub value: AttributeValue,

    /// 属性元数据（从配置加载，不序列化）
    #[serde(skip)]
    pub metadata: AttributeMetadata,
}

/// 属性值类型（枚举组合，而非继承）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AttributeValue {
    /// 静态值（如魅力、银两）
    Static { value: u8 },

    /// 每日随机值（如福缘）
    DailyRandom { value: u8, range: (u8, u8) },

    /// 可成长值（如力量、敏捷、根骨、悟性）
    Growable {
        base: u8,    // 极限值
        current: u8, // 当前值
    },
}

/// 属性元数据（从配置加载）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AttributeMetadata {
    /// 属性名称（英文key）
    pub name: String,

    /// 显示名称
    pub display_name: String,

    /// 属性描述
    pub description: String,

    /// 属性类型
    pub attr_type: AttributeType,

    /// 出生随机范围 [min, max]
    pub birth_range: Option<(u8, u8)>,

    /// 初始值
    pub initial_value: Option<u8>,

    /// 成长速率修正
    pub growth_rate: Option<f32>,

    /// 影响的派生属性列表
    #[serde(default)]
    pub affects: Vec<String>,

    /// 每tick衰减值
    pub decay_per_tick: Option<i32>,

    /// 死亡条件
    pub death_condition: Option<DeathCondition>,

    /// 计算公式
    pub formula: Option<String>,

    /// 默认值
    pub default_value: Option<i32>,

    /// 最小值
    pub min_value: Option<i32>,

    /// 最大值公式
    pub max_value_formula: Option<String>,

    /// 恢复公式
    pub recovery_formula: Option<String>,

    /// 依赖的主属性列表
    #[serde(default)]
    pub primary_attribute_deps: Vec<String>,
}

/// 属性类型
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AttributeType {
    /// 可成长属性（先天属性）
    Growable,

    /// 静态属性（先天属性）
    Static,

    /// 每日随机属性（先天属性）
    DailyRandom,

    /// 状态值（生理/精神状态）
    Status,

    /// 派生属性（计算得出）
    Derived,
}

/// 死亡条件
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeathCondition {
    /// 比较操作符
    pub operator: ComparisonOperator,

    /// 比较值
    pub value: i32,
}

/// 比较操作符
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ComparisonOperator {
    Equals,
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
}

impl ComparisonOperator {
    /// 检查值是否满足条件
    pub fn check(&self, current: i32, threshold: i32) -> bool {
        match self {
            ComparisonOperator::Equals => current == threshold,
            ComparisonOperator::LessThan => current < threshold,
            ComparisonOperator::LessThanOrEqual => current <= threshold,
            ComparisonOperator::GreaterThan => current > threshold,
            ComparisonOperator::GreaterThanOrEqual => current >= threshold,
        }
    }
}

impl Attribute {
    /// 从配置创建属性
    pub fn from_config(config: &AttributeMetadata) -> Self {
        let value = match config.attr_type {
            AttributeType::Growable => {
                let base = config
                    .birth_range
                    .map(|(min, max)| {
                        use rand::RngExt;
                        let mut rng = rand::rng();
                        rng.random_range(min..=max)
                    })
                    .unwrap_or(10);
                let current = config.initial_value.unwrap_or(10);
                AttributeValue::Growable { base, current }
            }
            AttributeType::Static => {
                let value = config
                    .birth_range
                    .map(|(min, max)| {
                        use rand::RngExt;
                        let mut rng = rand::rng();
                        rng.random_range(min..=max)
                    })
                    .unwrap_or(10);
                AttributeValue::Static { value }
            }
            AttributeType::DailyRandom => {
                let range = config.birth_range.unwrap_or((10, 50));
                use rand::RngExt;
                let mut rng = rand::rng();
                let value = rng.random_range(range.0..=range.1);
                AttributeValue::DailyRandom { value, range }
            }
            AttributeType::Status => {
                let value = config.default_value.unwrap_or(0) as u8;
                AttributeValue::Static { value }
            }
            AttributeType::Derived => {
                // 派生属性不存储值，实时计算
                AttributeValue::Static { value: 0 }
            }
        };

        Self {
            value,
            metadata: config.clone(),
        }
    }

    /// 获取属性值
    pub fn get_value(&self) -> i32 {
        self.value.get()
    }

    /// 设置属性值
    pub fn set_value(&mut self, value: i32) {
        self.value.set(value)
    }

    /// 成长（仅对可成长属性有效）
    pub fn train(&mut self, amount: i32) -> bool {
        if let AttributeValue::Growable { base, current } = &mut self.value
            && *current < *base {
                *current = (*current as i32 + amount).min(*base as i32) as u8;
                return true;
            }
        false
    }

    /// 突破极限（仅对可成长属性有效）
    pub fn breakthrough(&mut self, amount: i32) -> bool {
        if let AttributeValue::Growable { base, .. } = &mut self.value {
            *base = (*base as i32 + amount).min(100) as u8;
            return true;
        }
        false
    }

    /// 刷新每日随机值
    pub fn refresh_daily(&mut self) {
        if let AttributeValue::DailyRandom { value, range } = &mut self.value {
            use rand::RngExt;
            let mut rng = rand::rng();
            *value = rng.random_range(range.0..=range.1);
        }
    }
}

impl AttributeValue {
    /// 获取当前值
    pub fn get(&self) -> i32 {
        match self {
            AttributeValue::Static { value } => *value as i32,
            AttributeValue::DailyRandom { value, .. } => *value as i32,
            AttributeValue::Growable { current, .. } => *current as i32,
        }
    }

    /// 设置值
    pub fn set(&mut self, value: i32) {
        let v_u8 = value.clamp(0, 255) as u8;
        match self {
            AttributeValue::Static { value: v } => *v = v_u8,
            AttributeValue::DailyRandom { value: v, .. } => *v = v_u8,
            AttributeValue::Growable { current: v, .. } => *v = v_u8,
        }
    }
}

impl AttributeCollection {
    /// 创建空的属性集合
    pub fn new_collection() -> Self {
        Self {
            attributes: HashMap::new(),
        }
    }

    /// 获取属性
    pub fn get(&self, name: &str) -> Option<&Attribute> {
        self.attributes.get(name)
    }

    /// 获取可变引用
    pub fn get_mut(&mut self, name: &str) -> Option<&mut Attribute> {
        self.attributes.get_mut(name)
    }

    /// 获取属性值
    pub fn get_value(&self, name: &str) -> Option<i32> {
        self.get(name).map(|attr| attr.value.get())
    }

    /// 设置属性值
    pub fn set_value(&mut self, name: &str, value: i32) -> Result<(), String> {
        if let Some(attr) = self.get_mut(name) {
            attr.value.set(value);
            Ok(())
        } else {
            Err(format!("Attribute {} not found", name))
        }
    }

    /// 添加属性
    pub fn add(&mut self, attribute: Attribute) {
        self.attributes
            .insert(attribute.metadata.name.clone(), attribute);
    }
}

impl DeathCondition {
    /// 检查整数值是否满足死亡条件
    pub fn check_int(&self, current: i32) -> bool {
        self.operator.check(current, self.value)
    }
}

impl Default for AttributeMetadata {
    fn default() -> Self {
        Self {
            name: String::new(),
            display_name: String::new(),
            description: String::new(),
            attr_type: AttributeType::Static,
            birth_range: None,
            initial_value: None,
            growth_rate: None,
            affects: Vec::new(),
            decay_per_tick: None,
            death_condition: None,
            formula: None,
            default_value: None,
            min_value: None,
            max_value_formula: None,
            recovery_formula: None,
            primary_attribute_deps: Vec::new(),
        }
    }
}

// Duplicate impl Attribute removed - methods are defined in the first impl block

// ============================================================================
// 组件化设计（COI - Composition Over Inheritance）
// ============================================================================

/// 属性组件（先天属性）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AttributeComponent {
    pub collection: AttributeCollection,
}

impl AttributeComponent {
    /// 创建新组件
    pub fn new() -> Self {
        Self::default()
    }

    /// 从配置创建组件
    pub fn from_config(configs: &HashMap<String, AttributeMetadata>) -> Self {
        let mut collection = AttributeCollection::default();

        for (name, config) in configs {
            let attr = Attribute::from_config(config);
            collection.attributes.insert(name.clone(), attr);
        }

        Self { collection }
    }

    /// 创建随机属性（角色出生）
    pub fn random_from_config(configs: &HashMap<String, AttributeMetadata>) -> Self {
        let mut collection = AttributeCollection::default();

        for (name, config) in configs {
            let attr = Attribute::from_config(config);
            collection.attributes.insert(name.clone(), attr);
        }

        Self { collection }
    }

    /// 通用访问器（无硬编码）
    pub fn get(&self, name: &str) -> Option<&Attribute> {
        self.collection.attributes.get(name)
    }

    /// 获取属性值
    pub fn get_value(&self, name: &str) -> Option<i32> {
        self.collection
            .attributes
            .get(name)
            .map(|attr| attr.get_value())
    }

    /// 获取属性（可变）
    pub fn get_mut(&mut self, name: &str) -> Option<&mut Attribute> {
        self.collection.attributes.get_mut(name)
    }

    /// 属性成长
    pub fn train(&mut self, name: &str, amount: i32) -> bool {
        if let Some(attr) = self.collection.attributes.get_mut(name) {
            attr.train(amount)
        } else {
            false
        }
    }

    /// 突破极限
    pub fn breakthrough(&mut self, name: &str, amount: i32) -> bool {
        if let Some(attr) = self.collection.attributes.get_mut(name) {
            attr.breakthrough(amount)
        } else {
            false
        }
    }

    /// 刷新每日随机属性
    pub fn refresh_daily(&mut self) {
        for attr in self.collection.attributes.values_mut() {
            attr.refresh_daily();
        }
    }

    /// 检查属性是否存在
    pub fn has(&self, name: &str) -> bool {
        self.collection.attributes.contains_key(name)
    }

    /// 获取所有属性名称
    pub fn get_all_names(&self) -> Vec<String> {
        self.collection.attributes.keys().cloned().collect()
    }
}

/// 状态组件（状态值）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StatusComponent {
    pub collection: AttributeCollection,
}

impl StatusComponent {
    /// 创建新组件
    pub fn new() -> Self {
        Self::default()
    }

    /// 从配置创建组件
    pub fn from_config(configs: &HashMap<String, AttributeMetadata>) -> Self {
        let mut collection = AttributeCollection::default();

        for (name, config) in configs {
            let attr = Attribute::from_config(config);
            collection.attributes.insert(name.clone(), attr);
        }

        Self { collection }
    }

    /// 通用访问器
    pub fn get(&self, name: &str) -> Option<&Attribute> {
        self.collection.attributes.get(name)
    }

    /// 获取状态值
    pub fn get_value(&self, name: &str) -> Option<i32> {
        self.collection
            .attributes
            .get(name)
            .map(|attr| attr.get_value())
    }

    /// 设置状态值
    pub fn set_value(&mut self, name: &str, value: i32) {
        if let Some(attr) = self.collection.attributes.get_mut(name) {
            attr.set_value(value);
        }
    }

    /// 应用衰减
    pub fn apply_decay(&mut self) {
        for attr in self.collection.attributes.values_mut() {
            if let Some(decay) = attr.metadata.decay_per_tick {
                let current = attr.get_value();
                let new_value = (current + decay).max(0);
                attr.set_value(new_value);
            }
        }
    }

    /// 检查死亡条件
    pub fn check_death_conditions(&self) -> Vec<String> {
        let mut dead_attrs = Vec::new();

        for (name, attr) in &self.collection.attributes {
            if let Some(condition) = &attr.metadata.death_condition {
                let value = attr.get_value();
                if condition.operator.check(value, condition.value) {
                    dead_attrs.push(name.clone());
                }
            }
        }

        dead_attrs
    }
}

/// 派生属性组件（实时计算）
#[derive(Debug, Clone, Default)]
pub struct DerivedAttributeComponent {
    /// 配置缓存（不序列化，实时计算）
    pub configs: HashMap<String, AttributeMetadata>,
}

impl DerivedAttributeComponent {
    /// 创建新组件
    pub fn new() -> Self {
        Self::default()
    }

    /// 从配置创建组件
    pub fn from_config(configs: &HashMap<String, AttributeMetadata>) -> Self {
        Self {
            configs: configs.clone(),
        }
    }

    /// 计算派生属性（需要外部公式引擎）
    pub fn get_formula(&self, name: &str) -> Option<&String> {
        self.configs
            .get(name)
            .and_then(|config| config.formula.as_ref())
    }

    /// 获取所有派生属性名称
    pub fn get_all_names(&self) -> Vec<String> {
        self.configs.keys().cloned().collect()
    }
}
