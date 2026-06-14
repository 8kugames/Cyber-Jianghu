//! 意图验证器

use anyhow::Result;

use crate::actions::ParsedActionData;
use crate::actions::validate_action;
use crate::db::DbPool;
use crate::models::{AgentState, Intent};

/// 意图验证器
pub struct IntentResolver {
    db_pool: DbPool,
}

impl IntentResolver {
    pub fn new(db_pool: DbPool) -> Self {
        Self { db_pool }
    }

    /// 验证单个意图，返回类型安全的解析数据
    pub async fn validate_intent(
        &self,
        intent: &Intent,
        agent_state: &AgentState,
        all_states: &[AgentState],
    ) -> Result<ParsedActionData> {
        validate_action(intent, agent_state, all_states, &self.db_pool)
            .await
            .map_err(|e| anyhow::anyhow!("动作验证失败: {}", e))
    }
}
