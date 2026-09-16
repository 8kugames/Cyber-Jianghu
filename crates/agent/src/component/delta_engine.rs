// ============================================================================
// Delta Engine — WorldState 变化检测
// 纯规则引擎，零 LLM token 消耗
// ============================================================================

use cyber_jianghu_protocol::WorldState;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// 变化类别
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChangeCategory {
    Survival,
    Social,
    Environment,
    Inventory,
    Location,
}

/// 紧急程度
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Urgency {
    Critical,
    Important,
    Info,
}

/// 检测到的状态变化
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateChange {
    pub category: ChangeCategory,
    pub urgency: Urgency,
    pub field: String,
    pub description: String,
    pub data: serde_json::Value,
    pub tool_hint: Option<String>,
}

/// 变化检测结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateDelta {
    pub changes: Vec<StateChange>,
    pub is_first_tick: bool,
}

/// Delta 检测配置
#[derive(Debug, Clone)]
pub struct DeltaConfig {
    /// 变化百分比阈值（|diff| / 100 >= threshold => Important）
    pub change_percentage_threshold: f32,
    /// 生存驱动 Critical 阈值：只有 survival_drive.urgency >= 此值时才标 Critical
    /// 默认值 5 对应 narratives.yaml 中 satiation/hydration urgency=3(轻微), 7(重度), 10(致命)
    /// 低于此值的生存属性变化标 Important 而非 Critical，减少信号噪声
    pub survival_critical_urgency_threshold: u8,
}

impl Default for DeltaConfig {
    fn default() -> Self {
        Self {
            change_percentage_threshold: 0.1,
            survival_critical_urgency_threshold: 5,
        }
    }
}

/// Delta Engine: 比较 prev vs curr WorldState
pub struct DeltaEngine {
    config: DeltaConfig,
}

impl DeltaEngine {
    pub fn new(config: DeltaConfig) -> Self {
        Self { config }
    }

    /// 计算 prev → curr 的变化量
    pub fn compute(&self, prev: Option<&WorldState>, curr: &WorldState) -> StateDelta {
        let is_first_tick = prev.is_none();
        let mut changes = Vec::new();

        match prev {
            None => {
                self.detect_full_state(curr, &mut changes);
            }
            Some(prev) => {
                self.detect_survival_changes(
                    &curr.self_state.attributes,
                    &prev.self_state.attributes,
                    &curr.self_state.survival_drives,
                    &mut changes,
                );
                self.detect_social_changes(&curr.entities, &prev.entities, &mut changes);
                self.detect_environment_changes(&curr.events_log, &prev.events_log, &mut changes);
                self.detect_inventory_changes(
                    &curr.self_state.inventory,
                    &prev.self_state.inventory,
                    &mut changes,
                );
                self.detect_location_changes(&curr.location, &prev.location, &mut changes);
            }
        }

        StateDelta {
            changes,
            is_first_tick,
        }
    }

    /// 首次 tick：生成全量状态快照
    fn detect_full_state(&self, curr: &WorldState, changes: &mut Vec<StateChange>) {
        let survival_urgencies: HashMap<&str, u8> = curr
            .self_state
            .survival_drives
            .iter()
            .map(|sd| (sd.attribute.as_str(), sd.urgency))
            .collect();

        // 属性
        for (key, &val) in &curr.self_state.attributes {
            let urgency = match survival_urgencies.get(key.as_str()) {
                Some(&drive_urgency)
                    if drive_urgency >= self.config.survival_critical_urgency_threshold =>
                {
                    Urgency::Critical
                }
                Some(_) => Urgency::Important,
                None => Urgency::Important,
            };
            changes.push(StateChange {
                category: ChangeCategory::Survival,
                urgency,
                field: format!("attributes.{}", key),
                description: format!("初始状态 {}: {}", key, val),
                data: serde_json::json!({ key: val }),
                tool_hint: None,
            });
        }

        // 实体
        for entity in &curr.entities {
            changes.push(StateChange {
                category: ChangeCategory::Social,
                urgency: Urgency::Important,
                field: "entities".to_string(),
                description: format!("附近存在: {}", entity.name),
                data: serde_json::json!({ "id": entity.id, "name": entity.name }),
                tool_hint: Some(format!(
                    "query_world(section=entities, filter={})",
                    entity.name
                )),
            });
        }

        // 位置
        changes.push(StateChange {
            category: ChangeCategory::Location,
            urgency: Urgency::Important,
            field: "location".to_string(),
            description: format!(
                "当前位置: {} ({})",
                curr.location.name, curr.location.node_id
            ),
            data: serde_json::json!({
                "node_id": curr.location.node_id,
                "name": curr.location.name,
            }),
            tool_hint: Some("query_world(section=environment)".to_string()),
        });

        // 事件
        for event in &curr.events_log {
            changes.push(StateChange {
                category: ChangeCategory::Environment,
                urgency: Urgency::Important,
                field: "events_log".to_string(),
                description: format!("事件: {}", event.description),
                data: serde_json::to_value(event).unwrap_or_default(),
                tool_hint: Some("query_world(section=events)".to_string()),
            });
        }

        // 背包
        if !curr.self_state.inventory.is_empty() {
            changes.push(StateChange {
                category: ChangeCategory::Inventory,
                urgency: Urgency::Important,
                field: "inventory".to_string(),
                description: format!("背包有 {} 件物品", curr.self_state.inventory.len()),
                data: serde_json::json!(curr.self_state.inventory.len()),
                tool_hint: Some("query_world(section=inventory)".to_string()),
            });
        }
    }

    /// 检测属性变化（数据驱动：从 server 下发的 survival_drives 判定 Critical）
    ///
    /// Critical 判定规则：survival_drive.urgency >= config.survival_critical_urgency_threshold
    /// 而不是简单地检查属性是否在 survival_drives 中，以避免低紧迫度的生存属性变化
    /// （如 hydration=59→58, urgency=3）产生 Critical 信号噪声。
    fn detect_survival_changes(
        &self,
        curr_attrs: &HashMap<String, i32>,
        prev_attrs: &HashMap<String, i32>,
        survival_drives: &[cyber_jianghu_protocol::SurvivalDrive],
        changes: &mut Vec<StateChange>,
    ) {
        // 构建 attribute -> urgency 映射（仅 urgency > 0 的驱动）
        let survival_urgencies: HashMap<&str, u8> = survival_drives
            .iter()
            .map(|sd| (sd.attribute.as_str(), sd.urgency))
            .collect();

        for (key, &curr_val) in curr_attrs {
            let prev_val = prev_attrs.get(key).copied().unwrap_or(0);
            if curr_val == prev_val {
                continue;
            }
            let diff = (curr_val - prev_val).unsigned_abs();
            let urgency = match survival_urgencies.get(key.as_str()) {
                Some(&drive_urgency)
                    if drive_urgency >= self.config.survival_critical_urgency_threshold =>
                {
                    Urgency::Critical
                }
                Some(_) => Urgency::Important, // 生存属性但紧迫不足 → Important
                None if diff as f32 / 100.0 >= self.config.change_percentage_threshold => {
                    Urgency::Important
                }
                _ => Urgency::Info,
            };

            changes.push(StateChange {
                category: ChangeCategory::Survival,
                urgency,
                field: format!("attributes.{}", key),
                description: format!("{}: {} -> {}", key, prev_val, curr_val),
                data: serde_json::json!({ "key": key, "prev": prev_val, "curr": curr_val }),
                tool_hint: Some("query_world(section=state)".to_string()),
            });
        }
    }

    /// 检测实体变化（出现/消失）
    fn detect_social_changes(
        &self,
        curr_entities: &[cyber_jianghu_protocol::Entity],
        prev_entities: &[cyber_jianghu_protocol::Entity],
        changes: &mut Vec<StateChange>,
    ) {
        let curr_ids: HashSet<uuid::Uuid> = curr_entities.iter().map(|e| e.id).collect();
        let prev_ids: HashSet<uuid::Uuid> = prev_entities.iter().map(|e| e.id).collect();

        // 新出现
        for entity in curr_entities {
            if !prev_ids.contains(&entity.id) {
                changes.push(StateChange {
                    category: ChangeCategory::Social,
                    urgency: Urgency::Important,
                    field: "entities".to_string(),
                    description: format!("{} 出现", entity.name),
                    data: serde_json::json!({ "id": entity.id, "name": entity.name }),
                    tool_hint: Some(format!(
                        "query_world(section=entities, filter={})",
                        entity.name
                    )),
                });
            }
        }

        // 离开
        for entity in prev_entities {
            if !curr_ids.contains(&entity.id) {
                changes.push(StateChange {
                    category: ChangeCategory::Social,
                    urgency: Urgency::Info,
                    field: "entities".to_string(),
                    description: format!("{} 离开", entity.name),
                    data: serde_json::json!({ "id": entity.id, "name": entity.name }),
                    tool_hint: None,
                });
            }
        }
    }

    /// 检测新事件（events_log 末尾追加）
    fn detect_environment_changes(
        &self,
        curr_events: &[cyber_jianghu_protocol::WorldEvent],
        prev_events: &[cyber_jianghu_protocol::WorldEvent],
        changes: &mut Vec<StateChange>,
    ) {
        if curr_events.len() > prev_events.len() {
            for event in &curr_events[prev_events.len()..] {
                changes.push(StateChange {
                    category: ChangeCategory::Environment,
                    urgency: Urgency::Important,
                    field: "events_log".to_string(),
                    description: event.description.clone(),
                    data: serde_json::to_value(event).unwrap_or_default(),
                    tool_hint: Some("query_world(section=events)".to_string()),
                });
            }
        }
    }

    /// 检测背包变化
    fn detect_inventory_changes(
        &self,
        curr_inv: &[cyber_jianghu_protocol::InventoryItem],
        prev_inv: &[cyber_jianghu_protocol::InventoryItem],
        changes: &mut Vec<StateChange>,
    ) {
        let curr_map: HashMap<&str, &cyber_jianghu_protocol::InventoryItem> =
            curr_inv.iter().map(|i| (i.item_id.as_str(), i)).collect();
        let prev_map: HashMap<&str, &cyber_jianghu_protocol::InventoryItem> =
            prev_inv.iter().map(|i| (i.item_id.as_str(), i)).collect();

        for (id, item) in &curr_map {
            match prev_map.get(id) {
                None => {
                    changes.push(StateChange {
                        category: ChangeCategory::Inventory,
                        urgency: Urgency::Info,
                        field: format!("inventory.{}", id),
                        description: format!("获得 {} x{}", item.name, item.quantity),
                        data: serde_json::json!({ "item_id": id, "quantity": item.quantity }),
                        tool_hint: Some("query_world(section=inventory)".to_string()),
                    });
                }
                Some(prev_item) if prev_item.quantity != item.quantity => {
                    let urgency = if item.quantity < prev_item.quantity {
                        Urgency::Important
                    } else {
                        Urgency::Info
                    };
                    changes.push(StateChange {
                        category: ChangeCategory::Inventory,
                        urgency,
                        field: format!("inventory.{}", id),
                        description: format!(
                            "{}: {} -> {}",
                            item.name, prev_item.quantity, item.quantity
                        ),
                        data: serde_json::json!({
                            "item_id": id,
                            "prev": prev_item.quantity,
                            "curr": item.quantity,
                        }),
                        tool_hint: Some("query_world(section=inventory)".to_string()),
                    });
                }
                _ => {}
            }
        }

        // 移除
        for (id, item) in &prev_map {
            if !curr_map.contains_key(id) {
                changes.push(StateChange {
                    category: ChangeCategory::Inventory,
                    urgency: Urgency::Important,
                    field: format!("inventory.{}", id),
                    description: format!("失去 {}", item.name),
                    data: serde_json::json!({ "item_id": id, "lost": true }),
                    tool_hint: None,
                });
            }
        }
    }

    /// 检测位置变化
    fn detect_location_changes(
        &self,
        curr_loc: &cyber_jianghu_protocol::Location,
        prev_loc: &cyber_jianghu_protocol::Location,
        changes: &mut Vec<StateChange>,
    ) {
        if curr_loc.node_id != prev_loc.node_id {
            changes.push(StateChange {
                category: ChangeCategory::Location,
                urgency: Urgency::Important,
                field: "location.node_id".to_string(),
                description: format!("移动: {} -> {}", prev_loc.name, curr_loc.name),
                data: serde_json::json!({
                    "prev": { "node_id": prev_loc.node_id, "name": prev_loc.name },
                    "curr": { "node_id": curr_loc.node_id, "name": curr_loc.name },
                }),
                tool_hint: Some("query_world(section=environment)".to_string()),
            });
        }
    }
}

#[cfg(test)]
#[path = "delta_engine_tests.rs"]
mod tests;
