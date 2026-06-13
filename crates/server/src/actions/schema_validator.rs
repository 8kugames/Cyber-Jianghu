// ============================================================================
// 动作数据 Schema 校验器（P0-2）
// ============================================================================
//
// 在 action_data 写入 agent_action_logs 前，校验其字段是否匹配
// actions.yaml 中对应 action_type 的 validation.required_fields/optional_fields。
//
// 设计原则：
// - warning 模式：校验失败不阻断执行，violation 写入 soul_cycle_metadata
// - 无额外依赖：复用 serde_json::Value，无需新 crate
// - 数据驱动：schema 来源是 actions.yaml（非硬编码）
// ============================================================================

use crate::game_data::registry::ActionRegistry;
use crate::game_data::types::actions::ActionValidation;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 单条 schema 违规记录
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SchemaViolation {
    /// 违规类型
    pub violation_type: ViolationType,
    /// 涉及的字段名
    pub field: String,
    /// 动作类型
    pub action_type: String,
}

/// 违规类型
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViolationType {
    /// required_fields 中定义的字段在 action_data 中缺失
    MissingRequired,
    /// action_data 中存在未在 required_fields/optional_fields 中定义的字段
    UnknownField,
}

/// 校验 action_data 的 schema 合规性
///
/// # 返回
/// - Vec<SchemaViolation>: 发现的违规列表（空列表 = 合规）
///
/// # 行为
/// - 不阻断执行（warning 模式）
/// - 配置中未定义 validation 字段的动作跳过校验
pub fn validate_action_data_schema(
    action_type: &str,
    action_data: &Option<Value>,
) -> Vec<SchemaViolation> {
    let Some(config) = ActionRegistry::get(action_type) else {
        return Vec::new();
    };
    let Some(ref validation) = config.validation else {
        return Vec::new();
    };
    validate_against_schema(action_type, action_data, validation)
}

/// 针对给定的 validation schema 校验 action_data（可测试）
pub fn validate_against_schema(
    action_type: &str,
    action_data: &Option<Value>,
    validation: &ActionValidation,
) -> Vec<SchemaViolation> {
    let mut violations = Vec::new();

    let Some(data) = action_data.as_ref() else {
        return violations;
    };

    let data_keys: Vec<&str> = match data {
        Value::Object(map) => map.keys().map(|k| k.as_str()).collect(),
        _ => return violations,
    };

    let allowed: std::collections::HashSet<&str> = validation
        .required_fields
        .iter()
        .map(|s| s.as_str())
        .chain(validation.optional_fields.iter().map(|s| s.as_str()))
        .collect();

    for field in &validation.required_fields {
        if !data_keys.contains(&field.as_str()) {
            violations.push(SchemaViolation {
                violation_type: ViolationType::MissingRequired,
                field: field.clone(),
                action_type: action_type.to_string(),
            });
        }
    }

    if !allowed.is_empty() {
        for key in &data_keys {
            if !allowed.contains(key) {
                violations.push(SchemaViolation {
                    violation_type: ViolationType::UnknownField,
                    field: key.to_string(),
                    action_type: action_type.to_string(),
                });
            }
        }
    }

    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_validation() -> ActionValidation {
        ActionValidation {
            required_fields: vec!["item_id".into(), "quantity".into()],
            ..Default::default()
        }
    }

    fn speak_validation() -> ActionValidation {
        ActionValidation {
            required_fields: vec!["content".into()],
            optional_fields: vec!["channel".into(), "target_agent_id".into()],
            ..Default::default()
        }
    }

    #[test]
    fn test_missing_required_fields() {
        let validation = sample_validation();
        let action_data = Some(serde_json::json!({"quantity": 1}));
        let violations = validate_against_schema("用", &action_data, &validation);
        let missing: Vec<&SchemaViolation> = violations
            .iter()
            .filter(|v| matches!(v.violation_type, ViolationType::MissingRequired))
            .collect();
        assert_eq!(
            missing.len(),
            1,
            "应检测到缺少 required field: {:?}",
            violations
        );
        assert_eq!(missing[0].field, "item_id");
    }

    #[test]
    fn test_unknown_fields_detected() {
        let validation = speak_validation();
        let action_data = Some(serde_json::json!({
            "content": "你好",
            "channel": "public",
            "gadget": "magic"
        }));
        let violations = validate_against_schema("说话", &action_data, &validation);
        let unknown: Vec<&SchemaViolation> = violations
            .iter()
            .filter(|v| matches!(v.violation_type, ViolationType::UnknownField))
            .collect();
        assert_eq!(
            unknown.len(),
            1,
            "应检测到未知字段 gadget: {:?}",
            violations
        );
        assert_eq!(unknown[0].field, "gadget");
    }

    #[test]
    fn test_valid_action_data_no_violations() {
        let validation = sample_validation();
        let action_data = Some(serde_json::json!({
            "item_id": "mantou",
            "quantity": 1
        }));
        let violations = validate_against_schema("予", &action_data, &validation);
        assert!(
            violations.is_empty(),
            "有效的 action_data 不应有违规: {:?}",
            violations
        );
    }

    #[test]
    fn test_none_action_data_skipped() {
        let validation = sample_validation();
        let violations = validate_against_schema("移动", &None, &validation);
        assert!(
            violations.is_empty(),
            "None action_data 不应校验: {:?}",
            violations
        );
    }

    #[test]
    fn test_optional_fields_not_required() {
        let validation = speak_validation();
        let action_data = Some(serde_json::json!({
            "content": "你好"
        }));
        let violations = validate_against_schema("说话", &action_data, &validation);
        assert!(
            violations.is_empty(),
            "仅传 required field 应无违规: {:?}",
            violations
        );
    }

    #[test]
    fn test_both_required_and_unknown() {
        let validation = speak_validation();
        let action_data = Some(serde_json::json!({
            "content": "你好",
            "magic_field": "xyz"
        }));
        let violations = validate_against_schema("说话", &action_data, &validation);
        assert_eq!(violations.len(), 1, "应只有 1 个 unknown field 违规");
        assert!(
            matches!(violations[0].violation_type, ViolationType::UnknownField),
            "应为 UnknownField 类型"
        );
        assert_eq!(violations[0].field, "magic_field");
    }
}
