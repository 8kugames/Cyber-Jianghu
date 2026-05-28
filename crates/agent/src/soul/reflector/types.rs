// ============================================================================
// 验证器类型定义
// ============================================================================

use serde::Deserialize;

/// 人设信息
#[derive(Debug, Clone)]
pub struct PersonaInfo {
    /// 角色名字
    pub name: Option<String>,
    /// 性别
    pub gender: String,
    /// 年龄
    pub age: u8,
    /// 性格特点
    pub personality: Vec<String>,
    /// 三观倾向
    pub values: Vec<String>,
}

impl Default for PersonaInfo {
    fn default() -> Self {
        Self {
            name: None,
            gender: "男".to_string(),
            age: 28,
            personality: vec!["沉稳".into(), "重情义".into()],
            values: vec!["江湖道义为先".into()],
        }
    }
}

// ============================================================================
// LLM 响应格式
// ============================================================================

/// LLM 返回的验证结果格式
#[derive(Debug, Clone, Deserialize)]
pub struct LlmValidationResponse {
    /// 结果：approved 或 rejected
    #[serde(default)]
    pub result: String,
    /// 原因
    #[serde(default)]
    pub reason: String,
    /// 驳回类型（approved 时为 null 或缺失）
    #[serde(default, deserialize_with = "deserialize_null_string")]
    pub rejection_type: String,
    /// 叙事摘要（仅 approved 时有值）
    #[serde(default)]
    pub narrative: String,
}

/// 验证结果（内部使用）
#[derive(Debug, Clone, PartialEq)]
pub enum ValidationResult {
    /// 通过验证
    Approved {
        /// 验证通过的原因（可选）
        reason: Option<String>,
        /// 叙事摘要
        narrative: String,
    },
    /// 被驳回
    Rejected {
        /// 驳回原因
        reason: String,
        /// 驳回类型
        rejection_type: RejectionType,
    },
}

/// 天魂单层审查结果
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct LayerResult {
    /// 层标识
    pub layer: &'static str,
    /// 是否通过
    pub passed: bool,
    /// 详情，通过时为 None，驳回时包含原因
    pub detail: Option<String>,
}

/// ReflectorSoul 完整审查结果
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum PipelineValidationResult {
    /// 审查通过，携带修正后的 Intent、三层中间结果
    Approved {
        intent: crate::models::Intent,
        layers: Vec<LayerResult>,
        narrative: Option<String>,
    },
    /// 审查拒绝，携带原因和三层中间结果
    Rejected {
        reason: String,
        layers: Vec<LayerResult>,
    },
}

/// 驳回类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectionType {
    /// 时代设定冲突
    EraViolation,
    /// 力量体系冲突
    PowerSystemViolation,
    /// 角色人设冲突
    OutOfCharacter,
    /// 元游戏行为（打破第四面墙）
    MetaGaming,
    /// 语义重复（车轱辘话）
    SemanticRepeat,
    /// 其他原因
    Other,
}

impl RejectionType {
    /// 从字符串解析
    pub fn parse(s: &str) -> Self {
        match s {
            "era_violation" => Self::EraViolation,
            "power_system_violation" => Self::PowerSystemViolation,
            "out_of_character" => Self::OutOfCharacter,
            "meta_gaming" => Self::MetaGaming,
            "semantic_repeat" => Self::SemanticRepeat,
            _ => Self::Other,
        }
    }

    /// 转换为字符串
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::EraViolation => "era_violation",
            Self::PowerSystemViolation => "power_system_violation",
            Self::OutOfCharacter => "out_of_character",
            Self::MetaGaming => "meta_gaming",
            Self::SemanticRepeat => "semantic_repeat",
            Self::Other => "other",
        }
    }
}

// ============================================================================
// 验证请求
// ============================================================================

/// 验证请求
#[derive(Debug, Clone, Default)]
pub struct ValidationRuntimeConfig {
    /// 分级 LLM 校验配置
    pub graded_config: Option<cyber_jianghu_protocol::GradedValidationConfig>,
    /// 最近同类 intent 的完整决策内容（用于语义去重）
    pub recent_same_type_decisions: Vec<String>,
}

/// 验证请求
#[derive(Debug, Clone)]
pub struct ValidationRequest {
    /// 待验证的意图
    pub intent: crate::models::Intent,
    /// 人设信息
    pub persona: PersonaInfo,
    /// 当前世界状态（自然语言描述）
    pub world_context: String,
    /// 当前 WorldState，用于提取合法 ID 列表
    pub world_state: Option<cyber_jianghu_protocol::WorldState>,
    /// 运行时校验上下文
    pub runtime: ValidationRuntimeConfig,
}

/// 驳回原因
#[derive(Debug, Clone)]
pub struct RejectionReason {
    /// 意图 ID
    pub intent_id: uuid::Uuid,
    /// 驳回原因
    pub reason: String,
    /// 驳回类型
    pub rejection_type: RejectionType,
}

/// Deserializer that treats `null` as empty string
fn deserialize_null_string<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt = Option::<String>::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyber_jianghu_protocol::{
        AdjacentNode, AgentSelfState, GradedValidationConfig, InventoryItem, Location, WorldState,
        WorldTime,
    };
    use std::collections::HashMap;
    use uuid::Uuid;

    #[test]
    fn test_rejection_type_from_str() {
        assert_eq!(
            RejectionType::parse("era_violation"),
            RejectionType::EraViolation
        );
        assert_eq!(
            RejectionType::parse("out_of_character"),
            RejectionType::OutOfCharacter
        );
        assert_eq!(
            RejectionType::parse("semantic_repeat"),
            RejectionType::SemanticRepeat
        );
        assert_eq!(RejectionType::parse("unknown"), RejectionType::Other);
    }

    #[test]
    fn test_rejection_type_roundtrip() {
        let types = [
            RejectionType::EraViolation,
            RejectionType::PowerSystemViolation,
            RejectionType::OutOfCharacter,
            RejectionType::MetaGaming,
            RejectionType::SemanticRepeat,
            RejectionType::Other,
        ];
        for rt in &types {
            assert_eq!(RejectionType::parse(rt.as_str()), *rt);
        }
    }

    #[test]
    fn test_validation_request_keeps_runtime_context() {
        let request = ValidationRequest {
            intent: crate::models::Intent::new(Uuid::new_v4(), 7, "follow", None),
            persona: PersonaInfo::default(),
            world_context: "测试上下文".to_string(),
            world_state: Some(WorldState {
                event_type: "world_state".to_string(),
                tick_id: 7,
                agent_id: Some(Uuid::new_v4()),
                world_time: WorldTime {
                    year: 1,
                    month: 1,
                    day: 1,
                    hour: 8,
                    minute: 0,
                    second: 0,
                    weather: "晴".to_string(),
                },
                location: Location {
                    node_id: "loc_a".to_string(),
                    name: "地点A".to_string(),
                    node_type: "inn".to_string(),
                    adjacent_nodes: vec![AdjacentNode {
                        node_id: "loc_b".to_string(),
                        name: "地点B".to_string(),
                        travel_cost: 1,
                        aliases: vec![],
                    }],
                    gatherable_items: vec![],
                },
                self_state: AgentSelfState {
                    attributes: HashMap::new(),
                    derived_attributes: HashMap::new(),
                    attribute_descriptions: HashMap::new(),
                    status_effects: vec![],
                    inventory: vec![InventoryItem {
                        item_id: "mantou".to_string(),
                        name: "馒头".to_string(),
                        item_type: "food".to_string(),
                        quantity: 1,
                        is_equipped: false,
                        aliases: vec![],
                    }],
                    skills: vec![],
                    age_years: None,
                    max_age: None,
                    recipe_details: vec![],
                },
                entities: vec![],
                nearby_items: vec![],
                events_log: vec![],
                private_dialogue_log: vec![],
                last_execution_summary: None,
                lessons_learned: vec![],
            }),
            runtime: ValidationRuntimeConfig {
                graded_config: Some(GradedValidationConfig::default()),
                recent_same_type_decisions: vec![],
            },
        };

        assert!(request.runtime.graded_config.is_some());
        assert!(request.world_state.is_some());
    }
}
