//! 属性系统相关类型
//!
//! 完全数据驱动的属性系统（Data-Driven + COI）
//!
//! 本模块从 protocol 重新导出核心类型，并添加 server 特有的实现。

use std::collections::HashMap;

use super::status_component::DEFAULT_STATUS_MAX_VALUE;

// 从 protocol 重新导出核心类型
// 注意：AttributeComponent, StatusComponent, DerivedAttributeComponent 由 server crate 自定义实现
pub use cyber_jianghu_protocol::{
    Attribute, AttributeCollection, AttributeMetadata, AttributeType, AttributeValue,
};

// 导入组件类型（用于扩展trait实现）
use super::{AttributeComponent, DerivedAttributeComponent, StatusComponent};

// ============================================================================
// Server 特有的扩展 trait
// ============================================================================

/// Attribute 的 server 特有方法
// pub trait AttributeExt { ... } // Removed and moved to protocol
/// AttributeComponent 的 server 特有方法（预留：属性系统扩展）
#[allow(dead_code)]
pub trait AttributeComponentExt {
    /// 从配置创建组件
    fn from_config_map(configs: &HashMap<String, AttributeMetadata>) -> Self;

    /// 创建随机属性（角色出生）
    fn random_from_config(configs: &HashMap<String, AttributeMetadata>) -> Self;

    /// 通用访问器（无硬编码）
    fn get_attr(&self, name: &str) -> Option<&Attribute>;

    /// 获取属性值
    fn get_attr_value(&self, name: &str) -> Option<i32>;

    /// 获取属性（可变）
    fn get_attr_mut(&mut self, name: &str) -> Option<&mut Attribute>;

    /// 属性成长
    fn train_attr(&mut self, name: &str, amount: i32) -> bool;

    /// 突破极限
    fn breakthrough_attr(&mut self, name: &str, amount: i32) -> bool;

    /// 刷新每日随机属性
    fn refresh_daily_attrs(&mut self);

    /// 检查属性是否存在
    fn has_attr(&self, name: &str) -> bool;

    /// 获取所有属性名称
    fn get_all_attr_names(&self) -> Vec<String>;
}

impl AttributeComponentExt for AttributeComponent {
    fn from_config_map(configs: &HashMap<String, AttributeMetadata>) -> Self {
        let mut collection = AttributeCollection::new_collection();

        for (name, config) in configs {
            let attr = Attribute::from_config(config);
            collection.attributes.insert(name.clone(), attr);
        }

        Self { collection }
    }

    fn random_from_config(configs: &HashMap<String, AttributeMetadata>) -> Self {
        let mut collection = AttributeCollection::new_collection();

        for (name, config) in configs {
            let attr = Attribute::from_config(config);
            collection.attributes.insert(name.clone(), attr);
        }

        Self { collection }
    }

    fn get_attr(&self, name: &str) -> Option<&Attribute> {
        self.collection.attributes.get(name)
    }

    fn get_attr_value(&self, name: &str) -> Option<i32> {
        self.collection
            .attributes
            .get(name)
            .map(|attr| attr.get_value())
    }

    fn get_attr_mut(&mut self, name: &str) -> Option<&mut Attribute> {
        self.collection.attributes.get_mut(name)
    }

    fn train_attr(&mut self, name: &str, amount: i32) -> bool {
        if let Some(attr) = self.collection.attributes.get_mut(name) {
            attr.train(amount)
        } else {
            false
        }
    }

    fn breakthrough_attr(&mut self, name: &str, amount: i32) -> bool {
        if let Some(attr) = self.collection.attributes.get_mut(name) {
            attr.breakthrough(amount)
        } else {
            false
        }
    }

    fn refresh_daily_attrs(&mut self) {
        for attr in self.collection.attributes.values_mut() {
            attr.refresh_daily();
        }
    }

    fn has_attr(&self, name: &str) -> bool {
        self.collection.attributes.contains_key(name)
    }

    fn get_all_attr_names(&self) -> Vec<String> {
        self.collection.attributes.keys().cloned().collect()
    }
}

/// StatusComponent 的 server 特有方法（预留：状态系统扩展）
#[allow(dead_code)]
pub trait StatusComponentExt {
    /// 从配置创建组件
    fn from_config_map(configs: &HashMap<String, AttributeMetadata>) -> Self;

    /// 通用访问器
    fn get_attr(&self, name: &str) -> Option<&Attribute>;

    /// 获取状态值
    fn get_attr_value(&self, name: &str) -> Option<i32>;

    /// 设置状态值
    fn set_attr_value(&mut self, name: &str, value: i32);

    /// 应用衰减
    fn apply_attr_decay(&mut self);

    /// 检查死亡条件
    fn check_attr_death_conditions(&self) -> Vec<String>;
}

impl StatusComponentExt for StatusComponent {
    fn from_config_map(configs: &HashMap<String, AttributeMetadata>) -> Self {
        let mut collection = AttributeCollection::new_collection();

        for (name, config) in configs {
            let attr = Attribute::from_config(config);
            collection.attributes.insert(name.clone(), attr);
        }

        Self {
            collection,
            max_modifiers: Default::default(),
        }
    }

    fn get_attr(&self, name: &str) -> Option<&Attribute> {
        self.collection.attributes.get(name)
    }

    fn get_attr_value(&self, name: &str) -> Option<i32> {
        self.collection
            .attributes
            .get(name)
            .map(|attr| attr.get_value())
    }

    fn set_attr_value(&mut self, name: &str, value: i32) {
        if let Some(attr) = self.collection.attributes.get_mut(name) {
            attr.set_value(value.clamp(0, 255));
        }
    }

    fn apply_attr_decay(&mut self) {
        for attr in self.collection.attributes.values_mut() {
            if let Some(decay) = attr.metadata.decay_per_tick {
                let current = attr.get_value();
                let new_value = (current as f32 + decay).floor().clamp(0.0, DEFAULT_STATUS_MAX_VALUE) as i32;
                attr.set_value(new_value);
            }
        }
    }

    fn check_attr_death_conditions(&self) -> Vec<String> {
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

/// DerivedAttributeComponent 的 server 特有方法（预留：派生属性系统扩展）
#[allow(dead_code)]
pub trait DerivedAttributeComponentExt {
    /// 从配置创建组件
    fn from_config_map(configs: &HashMap<String, AttributeMetadata>) -> Self;

    /// 计算派生属性（需要外部公式引擎）
    fn get_attr_formula(&self, name: &str) -> Option<&String>;

    /// 获取所有派生属性名称
    fn get_all_attr_names(&self) -> Vec<String>;
}

impl DerivedAttributeComponentExt for DerivedAttributeComponent {
    fn from_config_map(configs: &HashMap<String, AttributeMetadata>) -> Self {
        Self {
            definitions: configs.clone(),
            cache: HashMap::new(),
        }
    }

    fn get_attr_formula(&self, name: &str) -> Option<&String> {
        self.definitions
            .get(name)
            .and_then(|config| config.formula.as_ref())
    }

    fn get_all_attr_names(&self) -> Vec<String> {
        self.definitions.keys().cloned().collect()
    }
}
