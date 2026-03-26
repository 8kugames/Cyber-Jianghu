//! 意图解析器
//!
//! 负责验证和解析 Agent 意图。

use anyhow::Result;
use tracing::debug;

use crate::actions::validate_action;
use crate::db::DbPool;
use crate::models::{AgentState, Intent};

/// 意图解析器
pub struct IntentResolver {
    db_pool: DbPool,
}

impl IntentResolver {
    /// 创建新的解析器
    pub fn new(db_pool: DbPool) -> Self {
        Self { db_pool }
    }

    /// 验证意图
    ///
    /// # 参数
    /// - `intent`: 待验证的意图
    /// - `agent_state`: Agent 当前状态
    /// - `all_states`: 所有 Agent 状态
    ///
    /// # 返回
    /// - `Ok(())`: 验证通过
    /// - `Err(...)`: 验证失败
    pub async fn validate_intent(
        &self,
        intent: &Intent,
        agent_state: &AgentState,
        all_states: &[AgentState],
    ) -> Result<()> {
        validate_action(intent, agent_state, all_states, &self.db_pool)
            .await
            .map_err(|e| anyhow::anyhow!("动作验证失败: {}", e))
    }

    /// 解析意图列表
    ///
    /// 返回有效意图的索引列表
    #[allow(dead_code)]
    pub async fn resolve_intents(
        &self,
        intents: &[Intent],
        agent_states: &[AgentState],
    ) -> Vec<usize> {
        let mut valid_indices = Vec::new();

        for (idx, intent) in intents.iter().enumerate() {
            // 查找对应的 Agent
            if let Some(agent_state) = agent_states.iter().find(|s| s.agent_id == intent.agent_id) {
                // 验证意图
                if self
                    .validate_intent(intent, agent_state, agent_states)
                    .await
                    .is_ok()
                {
                    valid_indices.push(idx);
                } else {
                    debug!(
                        "意图验证失败: agent={}, action={}",
                        intent.agent_id, intent.action_type
                    );
                }
            }
        }

        valid_indices
    }
}

/// 验证错误
#[allow(dead_code)]
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("Agent {0} 不存在")]
    AgentNotFound(uuid::Uuid),

    #[error("动作验证失败: {0}")]
    ActionValidationFailed(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_resolver_creation() {
        let db_pool = DbPool::connect_lazy("postgres://postgres@localhost/postgres").unwrap();
        let _resolver = IntentResolver::new(db_pool);
        // 测试创建成功
    }
}
