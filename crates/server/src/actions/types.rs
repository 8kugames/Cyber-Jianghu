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
