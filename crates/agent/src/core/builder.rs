// ============================================================================
// Agent Builder
// ============================================================================
//
// 提供流式接口构建 Agent
// ============================================================================

use std::sync::Arc;
use tracing::info;
use uuid::Uuid;

use crate::transport::websocket::AgentClient;
use crate::config::Config;
use crate::ai::dialogue::DialogueClient;
use crate::ai::lifespan::LifespanCalculator;
use crate::ai::llm::LlmClient;
use crate::ai::memory::{MemoryManager, MemoryManagerConfig};
use crate::ai::relationship::RelationshipStore;
use crate::ai::validator::{IntentValidator, Validator};
use cyber_jianghu_protocol::WorldBuildingRules;

use super::{Agent, DecisionCallback, DecisionWithFeedbackCallback, DecisionWithMemoryCallback, ValidatorConfig};

/// Agent 构建器
pub struct AgentBuilder {
    config: Config,
    decision_callback: DecisionCallback,
    decision_with_memory_callback: Option<DecisionWithMemoryCallback>,
    decision_with_feedback_callback: Option<DecisionWithFeedbackCallback>,
    enable_memory: bool,
    memory_config: Option<MemoryManagerConfig>,
    llm_client: Option<Arc<dyn LlmClient>>,
    dialogue_client: Option<DialogueClient>,
    relationship_store: Option<RelationshipStore>,
    validator: Option<Arc<dyn Validator>>,
    lifespan_calculator: Option<LifespanCalculator>,
    validator_config: ValidatorConfig,
}

impl AgentBuilder {
    /// 创建新的构建器
    pub fn new(config: Config, decision_callback: DecisionCallback) -> Self {
        Self {
            config,
            decision_callback,
            decision_with_memory_callback: None,
            decision_with_feedback_callback: None,
            enable_memory: true,
            memory_config: None,
            llm_client: None,
            dialogue_client: None,
            relationship_store: None,
            validator: None,
            lifespan_calculator: None,
            validator_config: ValidatorConfig::default(),
        }
    }

    /// 启用/禁用记忆系统
    pub fn enable_memory(mut self, enable: bool) -> Self {
        self.enable_memory = enable;
        self
    }

    /// 设置记忆管理器配置
    pub fn with_memory_config(mut self, config: MemoryManagerConfig) -> Self {
        self.memory_config = Some(config);
        self
    }

    /// 设置带反馈的决策回调
    pub fn with_decision_feedback(mut self, callback: DecisionWithFeedbackCallback) -> Self {
        self.decision_with_feedback_callback = Some(callback);
        self
    }

    /// 设置带记忆上下文的决策回调
    ///
    /// 此回调接收世界状态和记忆上下文，用于认知引擎集成
    pub fn with_decision_memory(mut self, callback: DecisionWithMemoryCallback) -> Self {
        self.decision_with_memory_callback = Some(callback);
        self
    }

    /// 设置对话客户端
    pub fn with_dialogue_client(mut self, client: DialogueClient) -> Self {
        self.dialogue_client = Some(client);
        self
    }

    /// 设置关系存储
    pub fn with_relationship_store(mut self, store: RelationshipStore) -> Self {
        self.relationship_store = Some(store);
        self
    }

    /// 设置验证器
    pub fn with_validator(mut self, validator: Arc<dyn Validator>) -> Self {
        self.validator = Some(validator);
        self
    }

    /// 设置 LLM 客户端（自动创建 IntentValidator）
    pub fn with_llm_client(
        mut self,
        llm_client: Arc<dyn LlmClient>,
        rules: Option<WorldBuildingRules>,
    ) -> Self {
        let rules = rules.unwrap_or_default();
        let validator = Arc::new(IntentValidator::new(rules, llm_client.clone()));
        self.validator = Some(validator);
        self.llm_client = Some(llm_client);
        self
    }

    /// 设置寿命计算器
    pub fn with_lifespan_calculator(mut self, calculator: LifespanCalculator) -> Self {
        self.lifespan_calculator = Some(calculator);
        self
    }

    /// 设置验证器配置
    pub fn with_validator_config(mut self, config: ValidatorConfig) -> Self {
        self.validator_config = config;
        self
    }

    /// 构建 Agent
    pub fn build(self) -> Agent {
        let client = AgentClient::new(self.config.server.clone());

        // 初始化记忆系统
        let memory_manager = if self.enable_memory {
            let agent_id = Uuid::new_v4();
            let config = self.memory_config.unwrap_or_else(|| MemoryManagerConfig {
                agent_id,
                ..Default::default()
            });

            // 如果有 LLM 客户端，使用 new_with_llm 初始化
            let result = if let Some(llm_client) = self.llm_client {
                MemoryManager::new_with_llm(config, llm_client)
            } else {
                MemoryManager::new(config)
            };

            match result {
                Ok(manager) => {
                    info!(
                        "Memory system initialized for agent '{}'",
                        self.config.agent.name
                    );
                    Some(manager)
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to initialize memory system: {}. Running without memory.",
                        e
                    );
                    None
                }
            }
        } else {
            None
        };

        Agent {
            config: self.config,
            client,
            decision_callback: self.decision_callback,
            decision_with_memory_callback: self.decision_with_memory_callback,
            decision_with_feedback_callback: self.decision_with_feedback_callback,
            memory_manager,
            dialogue_client: self.dialogue_client,
            relationship_store: self.relationship_store,
            validator: self.validator,
            lifespan_calculator: self.lifespan_calculator,
            validator_config: self.validator_config,
        }
    }
}
