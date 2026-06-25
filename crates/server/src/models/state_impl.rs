// ============================================================================
// AgentState FromRow 实现
// ============================================================================

use crate::game_data::registry;
use crate::game_data::types::{
    Attribute, AttributeCollection, AttributeComponent, AttributeMetadata, AttributeType,
    AttributeValue, StatusComponent,
};
use sqlx::Row;

use super::AgentState;

// 为 FromRow 实现自定义反序列化（组件化架构）
// sqlx 0.7+ FromRow 需要生命周期参数和 Row 类型
impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for AgentState {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        // 从 JSONB attributes 列读取状态值
        let attributes_json: serde_json::Value = row.try_get("attributes")?;

        // 1. 尝试从全局注册表获取配置，初始化完整组件（带 Metadata）
        let game_data = registry().map(|cache| cache.get());

        let mut status = if let Some(ref data) = game_data {
            StatusComponent::from_unified_config(&data.attributes)
        } else {
            // 回退：如果注册表未初始化（如测试环境），创建空组件
            StatusComponent {
                collection: AttributeCollection::new_collection(),
                max_modifiers: Default::default(),
            }
        };

        let mut primary_attributes = if let Some(ref data) = game_data {
            AttributeComponent::from_unified_config(&data.attributes)
        } else {
            AttributeComponent {
                collection: AttributeCollection::new_collection(),
            }
        };

        // 2. 使用数据库中的值覆盖默认值
        if let Some(obj) = attributes_json.as_object() {
            for (key, value) in obj {
                if let Some(num) = value.as_i64() {
                    let val = num as i32;

                    // 优先尝试更新 Status
                    if status.collection.attributes.contains_key(key) {
                        // 使用 set 方法会应用范围限制，这里我们需要直接设置值（信任数据库）
                        if let Some(attr) = status.collection.attributes.get_mut(key) {
                            attr.value.set(val);
                        }
                    }
                    // 其次尝试更新 Primary Attributes
                    else if primary_attributes.collection.attributes.contains_key(key) {
                        if let Some(attr) = primary_attributes.collection.attributes.get_mut(key) {
                            attr.value.set(val);
                        }
                    }
                    // 如果组件是空的（回退模式），则手动创建 Status 属性
                    else if game_data.is_none() {
                        let attr_value = val.clamp(0, 255) as u8;
                        let metadata = AttributeMetadata {
                            name: key.clone(),
                            display_name: key.clone(),
                            description: format!("{} attribute", key),
                            attr_type: AttributeType::Status,
                            birth_range: None,
                            initial_value: None,
                            growth_rate: None,
                            affects: vec![],
                            decay_per_tick: None,
                            death_condition: None,
                            formula: None,
                            default_value: Some(val as f32),
                            min_value: Some(0.0),
                            max_value_formula: Some("255".to_string()),
                            recovery_formula: None,
                            primary_attribute_deps: vec![],
                        };

                        status.collection.add(Attribute {
                            value: AttributeValue::Static { value: attr_value },
                            metadata,
                        });
                    }
                }
            }
        }

        // 恢复 max_modifiers（从 JSONB _max_modifiers 键读取）
        if let Some(modifiers) = attributes_json.get("_max_modifiers").and_then(|v| {
            serde_json::from_value::<std::collections::HashMap<String, i32>>(v.clone()).ok()
        }) {
            status.max_modifiers = modifiers;
        }

        Ok(Self {
            id: row.try_get("id")?,
            agent_id: row.try_get("agent_id")?,
            name: row.try_get("name").unwrap_or_default(),
            tick_id: row.try_get("tick_id")?,
            state_version: row.try_get("state_version").unwrap_or(0),
            primary_attributes,
            status,
            node_id: row.try_get("node_id")?,
            is_alive: row.try_get("is_alive")?,
            inventory_cleared_this_tick: false,
            skills: attributes_json
                .get("_skills")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default(),
            action_counts: attributes_json
                .get("_action_counts")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default(),
            birth_tick: row.try_get("birth_tick").ok().flatten(),
            decay_accumulator: std::collections::HashMap::new(),
            created_at: row.try_get("created_at")?,
        })
    }
}
