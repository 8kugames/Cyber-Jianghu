// ============================================================================
// 验证器类型定义
// ============================================================================

use serde::Deserialize;

/// 人设信息
#[derive(Debug, Clone)]
pub struct PersonaInfo {
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
            Self::Other => "other",
        }
    }
}

// ============================================================================
// 验证请求
// ============================================================================

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
}

/// 批次验证结果
#[derive(Debug, Clone)]
pub struct BatchValidationResult {
    /// 通过验证的 Intent
    pub valid_intents: Vec<crate::models::Intent>,
    /// 被驳回的 Intent 及原因
    pub rejections: Vec<(crate::models::Intent, RejectionReason)>,
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
        assert_eq!(RejectionType::parse("unknown"), RejectionType::Other);
    }
}
