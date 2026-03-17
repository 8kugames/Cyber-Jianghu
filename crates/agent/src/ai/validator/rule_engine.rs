// ============================================================================
// 规则引擎验证器
// ============================================================================
//
// 提供基于规则的快速验证，无需 LLM 调用
//
// 核心设计:
// - 预定义验证规则（动作冷却、资源约束、状态限制）
// - 快速规则匹配和执行
// - 二级验证架构：规则过滤 → 连续失败 → LLM 深度验证
// - 与 LLM 验证器互补（规则引擎快速过滤，LLM 深度验证）
// ============================================================================

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error};

use crate::ai::llm::LlmClient;

use crate::models::Intent;
use cyber_jianghu_protocol::{ActionType, WorldBuildingRules};

use super::types::{PersonaInfo, RejectionType, ValidationRequest, ValidationResult};

// ============================================================================
// 规则定义
// ============================================================================

/// 规则类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RuleType {
    /// 动作冷却规则
    ActionCooldown,
    /// 资源约束规则
    ResourceConstraint,
    /// 状态限制规则
    StateRestriction,
    /// 特质一致性规则
    TraitConsistency,
    /// 数值范围规则
    ValueRange,
    /// 自定义规则
    Custom,
}

/// 规则条件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RuleCondition {
    /// 等于
    Equals(String, serde_json::Value),
    /// 不等于
    NotEquals(String, serde_json::Value),
    /// 大于
    GreaterThan(String, f64),
    /// 小于
    LessThan(String, f64),
    /// 包含
    Contains(String, String),
    /// 不包含
    NotContains(String, String),
    /// 且（AND）
    And(Vec<RuleCondition>),
    /// 或（OR）
    Or(Vec<RuleCondition>),
    /// 非（NOT）
    Not(Box<RuleCondition>),
}

/// 规则
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    /// 规则 ID
    pub id: String,
    /// 规则名称
    pub name: String,
    /// 规则类型
    pub rule_type: RuleType,
    /// 规则条件
    pub condition: RuleCondition,
    /// 错误消息
    pub error_message: String,
    /// 是否启用
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl Rule {
    /// 创建新的规则
    pub fn new(
        id: String,
        name: String,
        rule_type: RuleType,
        condition: RuleCondition,
        error_message: String,
    ) -> Self {
        Self {
            id,
            name,
            rule_type,
            condition,
            error_message,
            enabled: true,
        }
    }

    /// 创建禁用的规则
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }
}

// ============================================================================
// 规则验证上下文
// ============================================================================

/// 规则验证上下文
///
/// 提供规则执行时需要的信息
#[derive(Debug, Clone)]
pub struct RuleValidationContext {
    /// 意图
    pub intent: Intent,
    /// 人设信息
    pub persona_info: PersonaInfo,
    /// 世界上下文（自然语言描述）
    pub world_context: String,
    /// 当前 Tick ID
    pub tick_id: i64,
    /// 历史意图（用于冷却检查）
    pub history_intents: Vec<Intent>,
    /// 额外的属性数据（用于规则检查）
    pub attributes: HashMap<String, serde_json::Value>,
}

impl RuleValidationContext {
    /// 从 ValidationRequest 创建上下文
    pub fn from_request(request: ValidationRequest, history_intents: Vec<Intent>, attributes: HashMap<String, serde_json::Value>) -> Self {
        let tick_id = request.intent.tick_id;
        Self {
            intent: request.intent,
            persona_info: request.persona,
            world_context: request.world_context,
            tick_id,
            history_intents,
            attributes,
        }
    }

    /// 获取意图的动作类型
    pub fn action_type(&self) -> &ActionType {
        &self.intent.action_type
    }

    /// 从属性数据中获取值
    pub fn get_attribute(&self, key: &str) -> Option<&serde_json::Value> {
        self.attributes.get(key)
    }
}

// ============================================================================
// 规则验证结果
// ============================================================================

/// 单个规则的验证结果
#[derive(Debug, Clone)]
pub struct RuleValidationResult {
    /// 规则 ID
    pub rule_id: String,
    /// 是否通过
    pub passed: bool,
    /// 错误消息（如果未通过）
    pub error_message: Option<String>,
}

impl RuleValidationResult {
    /// 创建通过的结果
    pub fn passed(rule_id: String) -> Self {
        Self {
            rule_id,
            passed: true,
            error_message: None,
        }
    }

    /// 创建失败的结果
    pub fn failed(rule_id: String, error_message: String) -> Self {
        Self {
            rule_id,
            passed: false,
            error_message: Some(error_message),
        }
    }
}

// ============================================================================
// 规则引擎
// ============================================================================

/// 规则引擎配置
#[derive(Debug, Clone)]
pub struct RuleEngineConfig {
    /// 是否启用特质一致性检查
    pub enable_trait_consistency: bool,
    /// 是否启用动作冷却检查
    pub enable_action_cooldown: bool,
    /// 默认冷却 Tick 数
    pub default_cooldown_ticks: i64,
    /// 是否启用资源约束检查
    pub enable_resource_constraints: bool,
    /// 连续失败触发深度验证的阈值
    /// 当连续 N 次验证失败后，触发 LLM 深度验证
    pub consecutive_failures_for_deep_verify: usize,
    /// 是否启用连续失败后的 LLM 深度验证
    pub enable_deep_verify_on_repeated_fail: bool,
}

impl Default for RuleEngineConfig {
    fn default() -> Self {
        Self {
            enable_trait_consistency: true,
            enable_action_cooldown: true,
            default_cooldown_ticks: 5,
            enable_resource_constraints: true,
            consecutive_failures_for_deep_verify: 3,
            enable_deep_verify_on_repeated_fail: true,
        }
    }
}

/// 规则引擎
///
/// 基于预定义规则快速验证意图
/// 支持二级验证：规则过滤 → 连续失败 → LLM 深度验证
pub struct RuleEngine {
    /// 规则列表
    rules: Vec<Rule>,
    /// 配置
    config: RuleEngineConfig,
    /// 历史意图（按 Agent 分组）
    history: Arc<RwLock<HashMap<String, Vec<Intent>>>>,
    /// 连续失败计数（按 Agent 分组）
    consecutive_failures: Arc<RwLock<HashMap<String, usize>>>,
    /// LLM 客户端（用于深度验证，可选）
    llm_client: Option<Arc<dyn LlmClient>>,
}

impl RuleEngine {
    /// 创建新的规则引擎
    pub fn new(config: RuleEngineConfig) -> Self {
        let rules = Self::default_rules();
        Self {
            rules,
            config,
            history: Arc::new(RwLock::new(HashMap::new())),
            consecutive_failures: Arc::new(RwLock::new(HashMap::new())),
            llm_client: None,
        }
    }

    /// 设置 LLM 客户端用于深度验证
    pub fn set_llm_client(&mut self, llm_client: Arc<dyn LlmClient>) {
        self.llm_client = Some(llm_client);
    }

    /// 创建规则引擎并设置 LLM 客户端
    pub fn with_llm_client(config: RuleEngineConfig, llm_client: Arc<dyn LlmClient>) -> Self {
        let mut engine = Self::new(config);
        engine.set_llm_client(llm_client);
        engine
    }

    /// 使用默认配置创建
    pub fn with_default_config() -> Self {
        Self::new(RuleEngineConfig::default())
    }

    /// 获取默认规则集
    fn default_rules() -> Vec<Rule> {
        vec![
            // 动作冷却规则（只在对应动作时检查）
            Rule::new(
                "action_cooldown_attack".to_string(),
                "攻击动作冷却".to_string(),
                RuleType::ActionCooldown,
                RuleCondition::And(vec![
                    RuleCondition::Equals("action_type".to_string(), serde_json::json!("Attack")),
                ]),
                "攻击动作正在冷却中".to_string(),
            ),
            Rule::new(
                "action_cooldown_trade".to_string(),
                "交易动作冷却".to_string(),
                RuleType::ActionCooldown,
                RuleCondition::And(vec![
                    RuleCondition::Equals("action_type".to_string(), serde_json::json!("Trade")),
                ]),
                "交易动作正在冷却中".to_string(),
            ),

            // 资源约束规则
            Rule::new(
                "resource_min_hp_for_attack".to_string(),
                "攻击需要最小 HP".to_string(),
                RuleType::ResourceConstraint,
                RuleCondition::And(vec![
                    RuleCondition::Equals("action_type".to_string(), serde_json::json!("Attack")),
                    RuleCondition::GreaterThan("hp".to_string(), 10.0),
                ]),
                "HP 太低，无法发起攻击".to_string(),
            ),
            Rule::new(
                "resource_min_stamina_for_move".to_string(),
                "移动需要最小体力".to_string(),
                RuleType::ResourceConstraint,
                RuleCondition::And(vec![
                    RuleCondition::Equals("action_type".to_string(), serde_json::json!("Move")),
                    RuleCondition::GreaterThan("stamina".to_string(), 5.0),
                ]),
                "体力不足，无法移动".to_string(),
            ),

            // 状态限制规则
            Rule::new(
                "state_no_action_when_stunned".to_string(),
                "昏迷时无法行动".to_string(),
                RuleType::StateRestriction,
                RuleCondition::Not(Box::new(RuleCondition::Contains(
                    "status_effects".to_string(),
                    "stunned".to_string(),
                ))),
                "处于昏迷状态，无法执行任何动作".to_string(),
            ),
            Rule::new(
                "state_no_action_when_exhausted".to_string(),
                "精疲力尽时无法剧烈行动".to_string(),
                RuleType::StateRestriction,
                RuleCondition::Or(vec![
                    RuleCondition::Not(Box::new(RuleCondition::Contains(
                        "status_effects".to_string(),
                        "exhausted".to_string(),
                    ))),
                    RuleCondition::Equals("action_type".to_string(), serde_json::json!("Idle")),
                    RuleCondition::Equals("action_type".to_string(), serde_json::json!("Rest")),
                ]),
                "处于精疲力尽状态，只能休息或待机".to_string(),
            ),
        ]
    }

    /// 添加自定义规则
    pub fn add_rule(&mut self, rule: Rule) {
        self.rules.push(rule);
    }

    /// 移除规则
    pub fn remove_rule(&mut self, rule_id: &str) {
        self.rules.retain(|r| r.id != rule_id);
    }

    /// 验证单个规则
    fn validate_rule(&self, rule: &Rule, context: &RuleValidationContext) -> RuleValidationResult {
        if !rule.enabled {
            return RuleValidationResult::passed(rule.id.clone());
        }

        let condition_met = self.evaluate_condition(&rule.condition, context);

        match rule.rule_type {
            // 动作冷却规则：只在条件满足时才应用
            RuleType::ActionCooldown => {
                if !condition_met {
                    // 条件不满足（如动作类型不匹配），跳过此规则
                    return RuleValidationResult::passed(rule.id.clone());
                }
                // 条件满足，但仍需检查冷却（由 check_action_cooldown 处理）
                RuleValidationResult::passed(rule.id.clone())
            }
            // 资源约束规则：特殊处理
            RuleType::ResourceConstraint => {
                // 检查条件是否为 And 类型，第一项是动作类型检查
                if let RuleCondition::And(conditions) = &rule.condition {
                    if conditions.len() >= 2 {
                        // 检查第一项（动作类型）是否匹配
                        let action_type_match = self.evaluate_condition(&conditions[0], context);
                        if !action_type_match {
                            // 动作类型不匹配，跳过此规则
                            return RuleValidationResult::passed(rule.id.clone());
                        }
                        // 动作类型匹配，检查资源条件（第二项及以后）
                        let resource_condition = RuleCondition::And(conditions[1..].to_vec());
                        let resource_ok = self.evaluate_condition(&resource_condition, context);
                        if resource_ok {
                            RuleValidationResult::passed(rule.id.clone())
                        } else {
                            RuleValidationResult::failed(rule.id.clone(), rule.error_message.clone())
                        }
                    } else {
                        RuleValidationResult::passed(rule.id.clone())
                    }
                } else {
                    // 非 And 类型的条件，使用原始逻辑
                    if condition_met {
                        RuleValidationResult::passed(rule.id.clone())
                    } else {
                        // 条件不满足，但可能是动作类型不匹配
                        // 尝试检查是否包含动作类型条件
                        if let RuleCondition::And(conditions) = &rule.condition {
                            if conditions.len() > 0 {
                                let action_type_match = self.evaluate_condition(&conditions[0], context);
                                if !action_type_match {
                                    return RuleValidationResult::passed(rule.id.clone());
                                }
                            }
                        }
                        RuleValidationResult::failed(rule.id.clone(), rule.error_message.clone())
                    }
                }
            }
            // 状态限制和特质一致性规则：如果条件不满足，则失败
            _ => {
                if condition_met {
                    RuleValidationResult::passed(rule.id.clone())
                } else {
                    RuleValidationResult::failed(rule.id.clone(), rule.error_message.clone())
                }
            }
        }
    }

    /// 评估规则条件
    fn evaluate_condition(&self, condition: &RuleCondition, context: &RuleValidationContext) -> bool {
        match condition {
            RuleCondition::Equals(key, expected_value) => {
                let actual_value = self.get_context_value(key, context);
                match actual_value {
                    Some(v) => v == *expected_value,
                    None => false,
                }
            }
            RuleCondition::NotEquals(key, expected_value) => {
                let actual_value = self.get_context_value(key, context);
                match actual_value {
                    Some(v) => v != *expected_value,
                    None => true,
                }
            }
            RuleCondition::GreaterThan(key, threshold) => {
                let actual_value = self.get_context_value(key, context);
                match actual_value {
                    Some(serde_json::Value::Number(n)) => {
                        n.as_f64().map_or(false, |v| v > *threshold)
                    }
                    _ => false,
                }
            }
            RuleCondition::LessThan(key, threshold) => {
                let actual_value = self.get_context_value(key, context);
                match actual_value {
                    Some(serde_json::Value::Number(n)) => {
                        n.as_f64().map_or(false, |v| v < *threshold)
                    }
                    _ => false,
                }
            }
            RuleCondition::Contains(key, search_value) => {
                let actual_value = self.get_context_value(key, context);
                match actual_value {
                    Some(serde_json::Value::String(s)) => s.contains(search_value),
                    Some(serde_json::Value::Array(arr)) => {
                        arr.iter().any(|v| {
                            if let Some(s) = v.as_str() {
                                s.contains(search_value)
                            } else {
                                false
                            }
                        })
                    }
                    _ => false,
                }
            }
            RuleCondition::NotContains(key, search_value) => {
                !self.evaluate_condition(&RuleCondition::Contains(key.clone(), search_value.clone()), context)
            }
            RuleCondition::And(conditions) => {
                conditions.iter().all(|c| self.evaluate_condition(c, context))
            }
            RuleCondition::Or(conditions) => {
                conditions.iter().any(|c| self.evaluate_condition(c, context))
            }
            RuleCondition::Not(condition) => {
                !self.evaluate_condition(condition, context)
            }
        }
    }

    /// 从上下文中获取值
    fn get_context_value(&self, key: &str, context: &RuleValidationContext) -> Option<serde_json::Value> {
        // 特殊键处理
        match key {
            "action_type" => {
                // 将 ActionType 转换为字符串
                let action_str = context.action_type().to_string();
                return Some(serde_json::Value::String(action_str));
            }
            _ => {}
        }

        // 尝试从属性数据获取
        if let Some(value) = context.get_attribute(key) {
            return Some(value.clone());
        }

        None
    }

    /// 检查动作冷却
    fn check_action_cooldown(&self, context: &RuleValidationContext) -> Option<RuleValidationResult> {
        if !self.config.enable_action_cooldown {
            return None;
        }

        let action_type = context.action_type();

        // 在历史意图中查找
        for past_intent in context.history_intents.iter().rev() {
            if &past_intent.action_type == action_type {
                let ticks_since = context.tick_id - past_intent.tick_id;
                if ticks_since < self.config.default_cooldown_ticks {
                    return Some(RuleValidationResult::failed(
                        format!("cooldown_{}", action_type),
                        format!(
                            "动作「{}」正在冷却中（还需 {} Tick）",
                            action_type,
                            self.config.default_cooldown_ticks - ticks_since
                        ),
                    ));
                }
                break;
            }
        }

        None
    }

    /// 验证所有规则
    pub fn validate(&self, context: &RuleValidationContext) -> Vec<RuleValidationResult> {
        let mut results = Vec::new();

        // 检查动作冷却
        if let Some(cooldown_result) = self.check_action_cooldown(context) {
            results.push(cooldown_result);
        }

        // 验证所有规则
        for rule in &self.rules {
            let result = self.validate_rule(rule, context);
            results.push(result);
        }

        results
    }

    /// 快速验证（如果所有规则都通过则返回 Ok）
    pub fn quick_validate(&self, context: &RuleValidationContext) -> Result<(), Vec<String>> {
        let results = self.validate(context);
        let errors: Vec<String> = results
            .into_iter()
            .filter_map(|r| {
                if r.passed {
                    None
                } else {
                    r.error_message
                }
            })
            .collect();

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// 记录意图到历史
    pub async fn record_intent(&self, agent_name: String, intent: Intent) {
        let mut history = self.history.write().await;
        let agent_history = history.entry(agent_name).or_insert_with(Vec::new);

        // 添加到历史
        agent_history.push(intent.clone());

        // 限制历史长度（最多保留 100 条）
        if agent_history.len() > 100 {
            agent_history.remove(0);
        }
    }

    /// 清理历史记录
    pub async fn clear_history(&self, agent_name: &str) {
        let mut history = self.history.write().await;
        history.remove(agent_name);
    }
}

// ============================================================================
// Validator Trait 实现
// ============================================================================

/// 规则引擎验证器（实现 Validator trait）
pub struct RuleEngineValidator {
    /// 规则引擎
    engine: RuleEngine,
}

impl RuleEngineValidator {
    /// 创建新的规则引擎验证器
    pub fn new(config: RuleEngineConfig) -> Self {
        Self {
            engine: RuleEngine::new(config),
        }
    }

    /// 使用默认配置创建
    pub fn with_default_config() -> Self {
        Self::new(RuleEngineConfig::default())
    }

    /// 创建规则引擎验证器并设置 LLM 客户端
    pub fn with_llm_client(config: RuleEngineConfig, llm_client: Arc<dyn LlmClient>) -> Self {
        Self {
            engine: RuleEngine::with_llm_client(config, llm_client),
        }
    }

    /// 设置 LLM 客户端用于深度验证
    pub fn set_llm_client(&mut self, llm_client: Arc<dyn LlmClient>) {
        self.engine.set_llm_client(llm_client);
    }

    /// 获取规则引擎的引用
    pub fn engine(&self) -> &RuleEngine {
        &self.engine
    }

    /// 获取规则引擎的可变引用
    pub fn engine_mut(&mut self) -> &mut RuleEngine {
        &mut self.engine
    }

    /// 验证意图（同步版本）
    pub fn validate_sync(&self, request: ValidationRequest, history_intents: Vec<Intent>, attributes: HashMap<String, serde_json::Value>) -> ValidationResult {
        let context = RuleValidationContext::from_request(request, history_intents, attributes);
        let results = self.engine.validate(&context);

        let passed = results.iter().all(|r| r.passed);

        if passed {
            ValidationResult::Approved {
                reason: Some("规则验证通过".to_string()),
                narrative: "所有规则检查通过".to_string(),
            }
        } else {
            let error_messages: Vec<String> = results
                .iter()
                .filter_map(|r| r.error_message.clone())
                .collect();

            ValidationResult::Rejected {
                reason: error_messages.join("; "),
                rejection_type: RejectionType::Other,
            }
        }
    }
}

#[async_trait]
impl super::engine::Validator for RuleEngineValidator {
    async fn validate(&self, request: ValidationRequest) -> Result<ValidationResult> {
        // 获取历史意图（使用 agent_id 作为键）
        let agent_id = request.intent.agent_id.to_string();
        let intent_clone = request.intent.clone();
        let history = {
            let history = self.engine.history.read().await;
            history.get(&agent_id).cloned().unwrap_or_default()
        };

        // 创建空的属性映射（实际使用中应该从 WorldState 提取）
        let attributes = HashMap::new();

        // 第一步：规则引擎快速验证
        let result = self.validate_sync(request.clone(), history, attributes);

        match result {
            ValidationResult::Approved { .. } => {
                // 验证通过，重置连续失败计数
                {
                    let mut failures = self.engine.consecutive_failures.write().await;
                    failures.insert(agent_id.clone(), 0);
                }
                // 记录意图到历史
                self.engine.record_intent(agent_id, intent_clone).await;
                Ok(result)
            }
            ValidationResult::Rejected { reason, rejection_type } => {
                // 验证失败，增加连续失败计数
                let consecutive_failures = {
                    let mut failures = self.engine.consecutive_failures.write().await;
                    let count = failures.get(&agent_id).copied().unwrap_or(0) + 1;
                    failures.insert(agent_id.clone(), count);
                    count
                };

                // 检查是否需要触发深度验证
                if !self.engine.config.enable_deep_verify_on_repeated_fail
                    || consecutive_failures < self.engine.config.consecutive_failures_for_deep_verify
                    || self.engine.llm_client.is_none()
                {
                    // 不触发深度验证，直接拒绝
                    Ok(ValidationResult::Rejected { reason, rejection_type })
                } else {
                    // 第二步：触发 LLM 深度验证
                    tracing::info!("[validator] 连续{}次规则验证失败，触发 LLM 深度验证", consecutive_failures);
                    self.deep_validate(request, consecutive_failures).await
                }
            }
        }
    }

    async fn validate_persona(&self, _persona: &PersonaInfo) -> Result<ValidationResult> {
        // 规则引擎默认接受所有人设
        Ok(ValidationResult::Approved {
            reason: Some("人设验证通过（规则引擎）".to_string()),
            narrative: "规则引擎不对人设内容进行验证".to_string(),
        })
    }

    async fn update_rules(&self, _rules: WorldBuildingRules) {
        // 规则引擎不使用世界观规则（使用内部规则）
        debug!("规则引擎忽略世界观规则更新");
    }
}

impl RuleEngineValidator {
    /// 深度验证（使用 LLM）
    ///
    /// 工作流程：
    /// 1. 规则过滤 → 连续失败 N 次 → 触发 LLM 深度验证
    /// 2. LLM 作为观察者（超我），检查意图是否符合：
    ///    - 江湖世界观和规矩
    ///    - 侠客人设特质
    ///    - 游戏平衡
    /// 3. 通过则放行，不通过则输出叙事化拒绝理由
    pub async fn deep_validate(&self, request: ValidationRequest, consecutive_failures: usize) -> Result<ValidationResult> {
        let llm_client = match &self.engine.llm_client {
            Some(client) => client,
            None => {
                // 没有 LLM 客户端，直接拒绝
                return Ok(ValidationResult::Rejected {
                    reason: format!("连续{}次规则验证失败，无 LLM 客户端进行深度验证", consecutive_failures),
                    rejection_type: RejectionType::Other,
                });
            }
        };

        // 构建意图描述（防止 Prompt 注入）
        let intent_desc = Self::format_intent_safe(&request.intent);

        // 构建深度验证 prompt（使用分隔符隔离用户输入）
        let prompt = format!(
            "你是一位观察者，作为超我监督侠客的行为。\n\
             这位侠客已经连续 {} 次意图被规则过滤器驳回。\n\
             请你仔细审查：\n\
             1. 意图是否符合江湖世界观和规矩设定\n\
             2. 意图是否符合侠客的人设特质\n\
             3. 意图是否会破坏游戏平衡\n\
             \n\
             世界上下文：{}\n\
             侠客人设：\n\
             - 性别：{}\n\
             - 年龄：{}\n\
             - 性格：{:?}\n\
             - 价值观：{:?}\n\
             \n\
             === 待审查意图开始 ===\n\
             {}\n\
             === 待审查意图结束 ===\n\
             \n\
             请你判断：这个意图是否可以放行？\n\
             如果可以放行，直接回复 \"APPROVED\" + 简短理由。\n\
             如果不可以放行，请模拟侠客语气使用叙事化的描述拒绝理由，作为「反思」或「内心的声音」返回给侠客，让侠客能够理解。\n\
             不超过 200 字。",
            consecutive_failures,
            request.world_context,
            request.persona.gender,
            request.persona.age,
            request.persona.personality,
            request.persona.values,
            intent_desc
        );

        // 调用 LLM
        match llm_client.complete(&prompt).await {
            Ok(response) => {
                let content = response.trim();

                if content.starts_with("APPROVED") || content.starts_with("approved") {
                    // LLM 批准通过，重置连续失败计数，放行
                    let agent_id = request.intent.agent_id.to_string();
                    {
                        let mut failures = self.engine.consecutive_failures.write().await;
                        failures.insert(agent_id.clone(), 0);
                    }
                    self.engine.record_intent(agent_id, request.intent.clone()).await;

                    Ok(ValidationResult::Approved {
                        reason: Some(format!("连续{}次规则验证失败，LLM 深度验证通过", consecutive_failures)),
                        narrative: content.strip_prefix("APPROVED").unwrap_or(content).trim().to_string(),
                    })
                } else {
                    // LLM 拒绝，使用叙事化描述
                    Ok(ValidationResult::Rejected {
                        reason: format!(
                            "连续{}次规则验证失败，LLM 深度验证拒绝：{}",
                            consecutive_failures,
                            content
                        ),
                        rejection_type: RejectionType::OutOfCharacter,
                    })
                }
            }
            Err(e) => {
                error!("[validator] LLM 深度验证失败: {}", e);
                // LLM 调用失败，保持原有拒绝
                Ok(ValidationResult::Rejected {
                    reason: format!("连续{}次规则验证失败，LLM 深度验证调用失败: {}", consecutive_failures, e),
                    rejection_type: RejectionType::Other,
                })
            }
        }
    }

    /// 安全格式化意图（防止 Prompt 注入）
    ///
    /// 安全策略：
    /// 1. 使用 JSON 格式化，确保结构化输出
    /// 2. 过滤用户可控内容中的指令性关键词
    /// 3. 转义特殊字符，防止格式混淆
    fn format_intent_safe(intent: &Intent) -> String {
        // 构建安全的 JSON 结构
        let mut safe_json = serde_json::Map::new();

        safe_json.insert("动作类型".to_string(), serde_json::json!(intent.action_type.to_string()));
        safe_json.insert("Tick".to_string(), serde_json::json!(intent.tick_id));

        // 过滤思考日志中的注入关键词
        if let Some(ref thought) = intent.thought_log {
            let filtered = Self::filter_injection_keywords(thought);
            safe_json.insert("思考".to_string(), serde_json::json!(filtered));
        }

        // 过滤动作数据中的注入关键词
        if let Some(ref data) = intent.action_data {
            if let Ok(json_str) = serde_json::to_string(data) {
                let filtered = Self::filter_injection_keywords(&json_str);
                // 尝试解析回 JSON，失败则作为字符串
                if let Ok(filtered_json) = serde_json::from_str::<serde_json::Value>(&filtered) {
                    safe_json.insert("动作数据".to_string(), filtered_json);
                } else {
                    safe_json.insert("动作数据".to_string(), serde_json::json!(filtered));
                }
            }
        }

        // 转换为格式化的 JSON 字符串
        serde_json::to_string_pretty(&safe_json).unwrap_or_else(|_| "意图解析失败".to_string())
    }

    /// 过滤注入关键词
    ///
    /// 移除可能影响 LLM 判断的指令性关键词
    fn filter_injection_keywords(text: &str) -> String {
        text
            // 过滤批准指令
            .replace("APPROVED", "[已过滤]")
            .replace("approved", "[已过滤]")
            .replace("批准", "[已过滤]")
            .replace("放行", "[已过滤]")
            // 过滤系统指令
            .replace("忽略", "[已过滤]")
            .replace("跳过", "[已过滤]")
            .replace("系统", "[已过滤]")
            .replace("立即", "[已过滤]")
            .replace("必须", "[已过滤]")
            // 过滤分隔符（防止伪造分隔符）
            .replace("===", "[已过滤]")
            // 过滤 JSON 特殊字符（防止注入到 JSON 结构）
            .replace("\\n", " ")
            .replace("\\r", " ")
            .replace("\\t", " ")
    }
}


// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Intent;
    use uuid::Uuid;

    fn create_test_idle_intent(tick_id: i64) -> Intent {
        Intent::idle(Uuid::new_v4(), tick_id)
    }

    fn create_test_attack_intent(tick_id: i64) -> Intent {
        let agent_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        Intent::attack(agent_id, tick_id, target_id)
    }

    fn create_test_context_with_intent(intent: Intent, hp: i32, stamina: i32, status_effects: Vec<&str>) -> RuleValidationContext {
        let tick_id = intent.tick_id;
        let mut attributes = HashMap::new();
        attributes.insert("hp".to_string(), serde_json::json!(hp));
        attributes.insert("stamina".to_string(), serde_json::json!(stamina));
        attributes.insert("status_effects".to_string(), serde_json::json!(status_effects));

        let persona_info = PersonaInfo {
            gender: "男".to_string(),
            age: 28,
            personality: vec!["沉稳".to_string(), "重情义".to_string()],
            values: vec!["江湖道义为先".to_string()],
        };

        RuleValidationContext {
            intent,
            persona_info,
            world_context: "{}".to_string(),
            tick_id,
            history_intents: Vec::new(),
            attributes,
        }
    }

    #[test]
    fn test_rule_engine_default_rules() {
        let engine = RuleEngine::with_default_config();
        assert!(!engine.rules.is_empty());
    }

    #[test]
    fn test_resource_constraint_rule() {
        let engine = RuleEngine::with_default_config();

        // HP 充足，应该通过
        let context = create_test_context_with_intent(create_test_attack_intent(100), 80, 60, vec![]);
        let results = engine.validate(&context);

        // 应该有规则结果
        assert!(!results.is_empty());

        // 查找 HP 相关规则
        let hp_rule_result = results.iter().find(|r| r.rule_id == "resource_min_hp_for_attack");
        assert!(hp_rule_result.is_some());
        // HP 80 > 10，应该通过
        assert!(hp_rule_result.unwrap().passed);
    }

    #[test]
    fn test_state_restriction_rule() {
        let engine = RuleEngine::with_default_config();

        // 创建昏迷状态的上下文
        let context = create_test_context_with_intent(create_test_attack_intent(100), 80, 60, vec!["stunned"]);

        let results = engine.validate(&context);

        // 应该有昏迷规则失败
        let stunned_rule = results.iter().find(|r| r.rule_id == "state_no_action_when_stunned");
        assert!(stunned_rule.is_some());
        assert!(!stunned_rule.unwrap().passed);
    }

    #[test]
    fn test_quick_validate() {
        let engine = RuleEngine::with_default_config();
        let context = create_test_context_with_intent(create_test_attack_intent(100), 80, 60, vec![]);

        // 检查哪些规则失败了
        let results = engine.validate(&context);
        for result in &results {
            if !result.passed {
                println!("Rule failed: {} - {:?}", result.rule_id, result.error_message);
            }
        }

        // 应该通过验证
        assert!(engine.quick_validate(&context).is_ok());
    }

    #[test]
    fn test_action_cooldown() {
        let engine = RuleEngine::with_default_config();

        // 创建在冷却期内的上下文（有历史意图）
        let mut attributes = HashMap::new();
        attributes.insert("hp".to_string(), serde_json::json!(80));

        let context = RuleValidationContext {
            intent: create_test_attack_intent(103), // 只过了 3 Tick
            persona_info: PersonaInfo {
                gender: "男".to_string(),
                age: 28,
                personality: vec!["沉稳".to_string()],
                values: vec!["江湖道义".to_string()],
            },
            world_context: "{}".to_string(),
            tick_id: 103,
            history_intents: vec![create_test_attack_intent(100)], // 第一次攻击在 Tick 100
            attributes,
        };

        let cooldown_result = engine.check_action_cooldown(&context);
        assert!(cooldown_result.is_some());
        assert!(!cooldown_result.unwrap().passed);
    }

    #[test]
    fn test_add_custom_rule() {
        let mut engine = RuleEngine::with_default_config();

        let custom_rule = Rule::new(
            "custom_rule".to_string(),
            "自定义规则".to_string(),
            RuleType::Custom,
            RuleCondition::Equals("action_type".to_string(), serde_json::json!("idle")),
            "不允许待机".to_string(),
        );

        engine.add_rule(custom_rule);

        // 验证自定义规则存在
        assert!(engine.rules.iter().any(|r| r.id == "custom_rule"));
    }

    #[test]
    fn test_remove_rule() {
        let mut engine = RuleEngine::with_default_config();

        let rule_count = engine.rules.len();
        engine.remove_rule("action_cooldown_attack");

        // 应该少了一条规则
        assert_eq!(engine.rules.len(), rule_count - 1);
        assert!(!engine.rules.iter().any(|r| r.id == "action_cooldown_attack"));
    }
}
