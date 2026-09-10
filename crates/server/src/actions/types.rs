use cyber_jianghu_protocol::AttributeValue;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::game_data::Operation;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionExecutionResult {
    pub intent_id: Option<Uuid>,
    pub success: bool,
    pub message: String,
    pub state_changes: Vec<StateChange>,
    pub action_type: String,
}

impl ActionExecutionResult {
    pub fn success(
        message: impl Into<String>,
        action_type: impl Into<String>,
        intent_id: Option<Uuid>,
    ) -> Self {
        Self {
            intent_id,
            success: true,
            message: message.into(),
            state_changes: Vec::new(),
            action_type: action_type.into(),
        }
    }

    pub fn failure(
        message: impl Into<String>,
        action_type: impl Into<String>,
        intent_id: Option<Uuid>,
    ) -> Self {
        Self {
            intent_id,
            success: false,
            message: message.into(),
            state_changes: Vec::new(),
            action_type: action_type.into(),
        }
    }

    pub fn add_change(&mut self, change: StateChange) {
        self.state_changes.push(change);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemEffect {
    pub attribute: String,
    #[serde(default)]
    pub operation: Operation,
    pub value: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StateChange {
    AttributeChanged {
        agent_id: Uuid,
        attribute: String,
        delta: AttributeValue,
    },
    HpChanged {
        agent_id: Uuid,
        delta: i32,
    },
    StaminaChanged {
        agent_id: Uuid,
        delta: i32,
    },
    AttributeMaxChanged {
        agent_id: Uuid,
        attribute: String,
        delta: i32,
    },
    ItemTransferred {
        from: Uuid,
        to: Uuid,
        item_id: String,
        quantity: i32,
    },
    ItemAcquired {
        agent_id: Uuid,
        item_id: String,
        quantity: i32,
        source: String,
    },
    ItemDisposed {
        agent_id: Uuid,
        item_id: String,
        quantity: i32,
        location: String,
    },
    ItemUsed {
        agent_id: Uuid,
        item_id: String,
        effects: Vec<ItemEffect>,
    },
    ItemEquipped {
        agent_id: Uuid,
        item_id: String,
    },
    ItemCrafted {
        agent_id: Uuid,
        item_id: String,
        quantity: i32,
    },
    MessageSpoken {
        agent_id: Uuid,
        content: String,
        channel: String,
        target_agent_id: Option<Uuid>,
        already_broadcast: bool,
    },
    AgentDied {
        agent_id: Uuid,
        cause: String,
    },
    LocationChanged {
        agent_id: Uuid,
        old_location: String,
        new_location: String,
    },
    SkillLearned {
        agent_id: Uuid,
        skill_id: String,
    },
    RecipeLearned {
        agent_id: Uuid,
        recipe_id: String,
        source: String,
    },
    Observation {
        observer_id: Uuid,
        target_id: Option<Uuid>,
        description: String,
        detected: bool,
    },
}

impl StateChange {
    /// 属性族变更的目标 Agent（AttributeMutator 按 agent_id 在状态切片中定位）。
    /// 物品/位置/技能等变更不经此路径（DB 直写或行动者自身），返回 None。
    /// 跨 Agent 效果（如攻击目标）由 processor 据此把目标状态纳入切片。
    pub fn affected_agent(&self) -> Option<Uuid> {
        match self {
            Self::AttributeChanged { agent_id, .. }
            | Self::HpChanged { agent_id, .. }
            | Self::StaminaChanged { agent_id, .. }
            | Self::AttributeMaxChanged { agent_id, .. }
            | Self::AgentDied { agent_id, .. } => Some(*agent_id),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 路由正确性直接决定跨 Agent 效果能否命中目标（历史 bug：HpChanged
    /// 找不到目标导致攻击整体回滚），属性族四个变体 + AgentDied 必须路由
    #[test]
    fn affected_agent_routes_attribute_family() {
        let id = Uuid::new_v4();
        assert_eq!(
            StateChange::HpChanged {
                agent_id: id,
                delta: -5
            }
            .affected_agent(),
            Some(id)
        );
        assert_eq!(
            StateChange::AttributeChanged {
                agent_id: id,
                attribute: "sanity".to_string(),
                delta: AttributeValue::Delta { value: -1 },
            }
            .affected_agent(),
            Some(id)
        );
        assert_eq!(
            StateChange::StaminaChanged {
                agent_id: id,
                delta: -3
            }
            .affected_agent(),
            Some(id)
        );
        assert_eq!(
            StateChange::AttributeMaxChanged {
                agent_id: id,
                attribute: "hp".to_string(),
                delta: 10
            }
            .affected_agent(),
            Some(id)
        );
        assert_eq!(
            StateChange::AgentDied {
                agent_id: id,
                cause: "combat".to_string()
            }
            .affected_agent(),
            Some(id)
        );
        // 物品/位置/技能族不经此路径
        assert_eq!(
            StateChange::LocationChanged {
                agent_id: id,
                old_location: "a".to_string(),
                new_location: "b".to_string(),
            }
            .affected_agent(),
            None
        );
    }
}
