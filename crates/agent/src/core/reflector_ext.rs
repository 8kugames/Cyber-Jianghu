// ============================================================================
// ReflectorSoul 审查扩展
// ============================================================================
//
// Agent 的天魂审查相关方法：
// - 分级审核策略（should_skip_llm_validation / adaptive_needs_llm）
// - 确定性 action_type 校验
// - RuleEngine 规则校验
// - ReflectorSoul 三层审查入口
// ============================================================================

use anyhow::Result;
use cyber_jianghu_protocol::{Intent, WorldState};
use tracing::{info, warn};

/// 天魂单层审查结果
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LayerResult {
    /// 层标识
    pub layer: &'static str,
    /// 是否通过
    pub passed: bool,
    /// 详情，通过时为 None，驳回时包含原因
    pub detail: Option<String>,
}

/// ReflectorSoul 审查结果
#[allow(clippy::large_enum_variant)]
pub enum ReflectorResult {
    /// 审查通过，携带修正后的 Intent、三层中间结果
    Approved {
        intent: Intent,
        layers: Vec<LayerResult>,
        #[allow(dead_code)]
        narrative: Option<String>,
    },
    /// 审查拒绝，携带叙事化原因和三层中间结果
    Rejected {
        reason: String,
        layers: Vec<LayerResult>,
    },
}

/// 人设验证结果
#[derive(Debug)]
pub enum PersonaValidationResult {
    /// 验证通过
    Approved,
    /// 需要修改
    NeedsRevision {
        reason: String,
        rejection_type: crate::soul::reflector::RejectionType,
    },
    /// 跳过验证（无验证器）
    Skipped,
}

impl super::Agent {
    // ========================================================================
    // 分级审核策略
    // ========================================================================

    /// 判断 Intent 是否应跳过 LLM 审核（分级审核策略）
    ///
    /// Skip 类型（idle, wait）→ true
    /// Always 类型（speak, shout, whisper）→ false
    /// Adaptive 类型 → 根据 action_data 判断
    pub(crate) fn should_skip_llm_validation(
        intent: &Intent,
        config: Option<&cyber_jianghu_protocol::GradedValidationConfig>,
    ) -> bool {
        let Some(config) = config else {
            return false;
        };

        let action_str = intent.action_type.as_str().to_string();

        if config.skip_types.contains(&action_str) {
            return true;
        }
        if config.always_types.contains(&action_str) {
            return false;
        }
        if config.adaptive_types.contains(&action_str) {
            return !Self::adaptive_needs_llm(intent, config);
        }

        true
    }

    /// Adaptive 检查：判断是否需要 LLM 审核
    fn adaptive_needs_llm(
        intent: &Intent,
        config: &cyber_jianghu_protocol::GradedValidationConfig,
    ) -> bool {
        let action_data = match &intent.action_data {
            Some(d) => d,
            None => return false,
        };

        // 数据驱动：根据配置的字段映射决定如何检查
        if let Some(field_name) = config
            .adaptive_field_mapping
            .get(intent.action_type.as_str())
        {
            match field_name.as_str() {
                "target_location" => action_data
                    .get(field_name)
                    .and_then(|v| v.as_str())
                    .map(|loc| {
                        config
                            .restricted_area_keywords
                            .iter()
                            .any(|k| loc.contains(k.as_str()))
                    })
                    .unwrap_or(false),
                "item_id" => action_data
                    .get(field_name)
                    .and_then(|v| v.as_str())
                    .map(|id| {
                        config
                            .high_value_item_keywords
                            .iter()
                            .any(|k| id.contains(k.as_str()))
                    })
                    .unwrap_or(false),
                _ => true,
            }
        } else {
            true
        }
    }

    // ========================================================================
    // 确定性校验
    // ========================================================================

    /// 确定性 action_type 校验（不经过 LLM）
    ///
    /// 从本地 actions.json 加载合法 action 列表，检查 intent 的 action_type 是否在列。
    /// idle 动作始终放行。
    /// actions.json 不存在时放行（无数据不做拦截）。
    ///
    /// 翻译层已将中文→英文，正常情况只匹配英文 canonical key。
    /// 此处别名匹配是防御性安全网。
    fn validate_action_type(&self, intent: &Intent) -> Result<(), String> {
        // idle 始终合法
        if intent.action_type.as_str() == "休息" {
            return Ok(());
        }

        // Fail-safe: "narrative" sentinel 来自旧翻译架构
        // 人魂直连后不应出现此情况
        if intent.action_type.as_str() == "narrative" {
            tracing::error!(
                "narrative sentinel 泄漏到 validate_action_type（人魂直连后不应出现），强制拒绝"
            );
            return Err("意图格式异常：narrative 未被翻译".to_string());
        }

        let actions = crate::infra::api::cognitive_context::load_available_actions_from_file();
        if actions.is_empty() {
            // 无数据不做拦截
            return Ok(());
        }

        // 1. 精确匹配英文 canonical action key
        let valid_names: Vec<&str> = actions.iter().map(|a| a.action.as_str()).collect();
        if valid_names.contains(&intent.action_type.as_str()) {
            return Ok(());
        }

        // 2. 安全网：匹配中文名或别名（翻译层漏网之鱼）
        let action_input = intent.action_type.as_str().to_lowercase();
        for a in &actions {
            if a.name.to_lowercase() == action_input {
                return Ok(());
            }
            if a.aliases
                .iter()
                .any(|alias| alias.to_lowercase() == action_input)
            {
                return Ok(());
            }
        }

        // 找最接近的合法 action（用中文名做模糊匹配）
        let suggestion = actions
            .iter()
            .find(|a| {
                let name_lower = a.name.to_lowercase();
                name_lower.contains(&action_input) || action_input.contains(&name_lower)
            })
            .map(|a| a.name.as_str())
            .unwrap_or("休息");

        Err(format!(
            "action '{}' 不在合法列表中，合法值: [{}]，最接近: '{}'",
            intent.action_type,
            valid_names.join(", "),
            suggestion,
        ))
    }

    /// RuleEngine 规则校验（Layer 2）
    async fn validate_with_rule_engine(
        &self,
        intent: &Intent,
        world_state: &WorldState,
    ) -> Result<(), String> {
        // 连续 follow 限制（社交死循环防护）
        if intent.action_type.as_str() == "follow" {
            let max_consecutive = self.config.llm.max_consecutive_follow;
            if (self.consecutive_follow_count as usize) >= max_consecutive {
                return Err(format!(
                    "已连续跟随 {} 次，请尝试其他行为（如 说话、采集、休息）",
                    max_consecutive
                ));
            }
        }

        use crate::soul::reflector::rule_engine::{
            RuleValidationContext, types::extract_ids_from_world_state,
        };

        let (available_item_ids, reachable_node_ids) = extract_ids_from_world_state(world_state);

        let context = RuleValidationContext {
            intent: intent.clone(),
            persona_info: self.extract_persona(),
            world_context: String::new(),
            tick_id: world_state.tick_id,
            history_intents: vec![],
            attributes: std::collections::HashMap::new(),
            available_item_ids,
            reachable_node_ids,
        };

        match self.rule_engine.validate_context(&context).await {
            Ok(crate::soul::reflector::ValidationResult::Approved { .. }) => Ok(()),
            Ok(crate::soul::reflector::ValidationResult::Rejected { reason, .. }) => Err(reason),
            Err(e) => {
                tracing::warn!("RuleEngine error, bypassing: {}", e);
                Ok(())
            }
        }
    }

    /// 仅做确定性规则校验（Layer 1 + Layer 2，不经过 LLM）
    ///
    /// 用于分级审核中 Skip 级别的 Intent（idle、wait 等），
    /// 只检查 action_type 合法性和 RuleEngine 规则，跳过 LLM 审查。
    pub(crate) async fn validate_rules_only(
        &self,
        intent: &Intent,
        world_state: &WorldState,
    ) -> Result<(), String> {
        // Layer 1: action_type
        self.validate_action_type(intent)?;
        // Layer 2: RuleEngine
        self.validate_with_rule_engine(intent, world_state).await
    }

    // ========================================================================
    // ReflectorSoul 三层审查入口
    // ========================================================================

    /// ReflectorSoul 同步审查 Intent
    ///
    /// 三层审查，规则型在 LLM 之前：
    /// 1. action_type 确定性校验：是否在合法动作列表中
    /// 2. RuleEngine 规则校验：eat/drink item_id 有效性、move 目标可达性等
    /// 3. LLM 审查：人设/世界观合规
    pub async fn validate_with_reflector(
        &mut self,
        intent: Intent,
        world_state: &WorldState,
    ) -> Result<ReflectorResult> {
        let mut layers = Vec::with_capacity(3);

        // 第一层：action_type 确定性校验（不经过 LLM）
        match self.validate_action_type(&intent) {
            Ok(()) => {
                layers.push(LayerResult {
                    layer: "layer1",
                    passed: true,
                    detail: None,
                });
            }
            Err(e) => {
                warn!("Action type validation failed: {}", e);
                layers.push(LayerResult {
                    layer: "layer1",
                    passed: false,
                    detail: Some(e.clone()),
                });
                return Ok(ReflectorResult::Rejected { reason: e, layers });
            }
        }

        // 第二层：RuleEngine 规则校验（确定性，不经过 LLM）
        match self.validate_with_rule_engine(&intent, world_state).await {
            Ok(()) => {
                layers.push(LayerResult {
                    layer: "layer2",
                    passed: true,
                    detail: None,
                });
            }
            Err(e) => {
                warn!("Rule engine validation failed: {}", e);
                layers.push(LayerResult {
                    layer: "layer2",
                    passed: false,
                    detail: Some(e.clone()),
                });
                return Ok(ReflectorResult::Rejected { reason: e, layers });
            }
        }

        // 第三层：LLM 审查（人设/世界观）
        let validator = match &self.validator {
            Some(v) => v,
            None => {
                layers.push(LayerResult {
                    layer: "layer3",
                    passed: true,
                    detail: None,
                });
                return Ok(ReflectorResult::Approved {
                    intent,
                    layers,
                    narrative: None,
                });
            }
        };

        let request = crate::soul::reflector::ValidationRequest {
            intent: intent.clone(),
            persona: self.extract_persona(),
            world_context: self.build_world_context(world_state),
            world_state: Some(world_state.clone()),
        };

        // LLM 错误时 fail-open（自动通过）
        let validation_result = match validator.validate(request).await {
            Ok(result) => result,
            Err(e) => {
                warn!("ReflectorSoul validation error, auto-approving: {}", e);
                layers.push(LayerResult {
                    layer: "layer3",
                    passed: true,
                    detail: Some(format!("LLM error, bypassed: {}", e)),
                });
                return Ok(ReflectorResult::Approved {
                    intent,
                    layers,
                    narrative: None,
                });
            }
        };

        match validation_result {
            crate::soul::reflector::ValidationResult::Approved { .. } => {
                info!("ReflectorSoul approved");
                layers.push(LayerResult {
                    layer: "layer3",
                    passed: true,
                    detail: None,
                });
                Ok(ReflectorResult::Approved {
                    intent,
                    layers,
                    narrative: None,
                })
            }
            crate::soul::reflector::ValidationResult::Rejected {
                reason,
                rejection_type: _,
            } => {
                warn!("ReflectorSoul rejected: {}", reason);
                layers.push(LayerResult {
                    layer: "layer3",
                    passed: false,
                    detail: Some(reason.clone()),
                });
                Ok(ReflectorResult::Rejected { reason, layers })
            }
        }
    }

    // ========================================================================
    // 人设验证
    // ========================================================================

    /// 验证人设合规性
    pub async fn validate_persona(&self) -> Result<PersonaValidationResult> {
        let validator = match &self.validator {
            Some(v) => v,
            None => return Ok(PersonaValidationResult::Skipped),
        };

        let persona = self.extract_persona();

        match validator.validate_persona(&persona).await? {
            crate::soul::reflector::ValidationResult::Approved { .. } => {
                Ok(PersonaValidationResult::Approved)
            }
            crate::soul::reflector::ValidationResult::Rejected {
                reason,
                rejection_type,
            } => Ok(PersonaValidationResult::NeedsRevision {
                reason,
                rejection_type,
            }),
        }
    }
}
