use crate::db::DbPool;
use crate::game_data::types::actions::ValidatorKind;
use crate::game_data::types::{ActionValidation, FieldValidation};
use crate::game_data::{ActionRegistry, ActionRequirement};
use crate::models::{AgentState, Intent};
use cyber_jianghu_protocol::GameError;
use uuid::Uuid;

/// 验证动作是否可以执行（完全数据驱动）
pub async fn validate_action(
    intent: &Intent,
    agent_state: &AgentState,
    all_states: &[AgentState],
    db_pool: &DbPool,
) -> Result<(), GameError> {
    if !agent_state.is_alive {
        return Err(GameError::AgentDead {
            agent_id: agent_state.agent_id,
        });
    }

    let action_str = intent.action_type.as_str();
    let config = ActionRegistry::get(action_str).ok_or_else(|| GameError::InvalidActionData {
        reason: format!("未知的动作类型: {}", action_str),
    })?;

    validate_generic_requirements(intent, agent_state, db_pool).await?;

    match config.validator_kind {
        Some(ValidatorKind::RecipeKnowledge) => {
            validate_recipe_knowledge(intent, agent_state, db_pool).await?;
        }
        Some(ValidatorKind::TeachRecipe) => {
            validate_teach_recipe(intent, agent_state, all_states, db_pool).await?;
        }
        None => {}
    }

    if let Some(validation) = &config.validation {
        validate_by_rules(intent, agent_state, all_states, validation)?;
    }

    Ok(())
}

fn validate_by_rules(
    intent: &Intent,
    agent_state: &AgentState,
    all_states: &[AgentState],
    validation: &ActionValidation,
) -> Result<(), GameError> {
    let action_data = intent.action_data.clone();

    for field in &validation.required_fields {
        if !has_field(&action_data, field) {
            if field == "target_id" && has_field(&action_data, "item_id") {
                continue;
            }
            tracing::warn!(
                "动作验证缺少字段: action={}, field={}, action_data={:?}",
                intent.action_type,
                field,
                intent.action_data
            );
            return Err(GameError::InvalidActionData {
                reason: format!("缺少必需字段: {}", field),
            });
        }
    }

    for field_validation in &validation.field_validations {
        validate_field(intent, field_validation)?;
    }

    if validation.requires_target.unwrap_or(false) {
        validate_target_exists(intent, all_states)?;
    }

    if validation.requires_target_alive.unwrap_or(false) {
        validate_target_alive(intent, all_states)?;
    }

    if validation.requires_target_colocated.unwrap_or(false) {
        validate_target_colocated(intent, agent_state, all_states)?;
    }

    Ok(())
}

async fn validate_generic_requirements(
    intent: &Intent,
    agent_state: &AgentState,
    db_pool: &DbPool,
) -> Result<(), GameError> {
    let action_name = intent.action_type.to_string();
    if let Some(config) = ActionRegistry::get(&action_name) {
        for req in &config.requirements {
            match req.requirement_type.as_str() {
                ActionRequirement::REQUIREMENT_TYPE_ATTRIBUTE => {
                    let attribute = req.get_str("attribute").unwrap_or("unknown");
                    let min = req.get_i32("min").unwrap_or(0);

                    let current = agent_state.get_i32(attribute).unwrap_or(0);
                    if current < min {
                        return Err(GameError::Unknown(format!(
                            "属性 {} 不足: 需要 {}, 当前 {}",
                            attribute, min, current
                        )));
                    }
                }
                ActionRequirement::REQUIREMENT_TYPE_ITEM => {
                    let item_id = req.get_str("item_id").unwrap_or("unknown");
                    let min_qty = req.get_i32("quantity").unwrap_or(1);

                    let item_quantity =
                        get_inventory_item_quantity(db_pool, agent_state.agent_id, item_id).await;
                    if item_quantity < min_qty {
                        return Err(GameError::Unknown(format!(
                            "物品 {} 不足: 需要 {}, 当前 {}",
                            item_id, min_qty, item_quantity
                        )));
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn has_field(action_data: &Option<serde_json::Value>, field: &str) -> bool {
    if let Some(data) = action_data
        && let Some(obj) = data.as_object()
    {
        return obj.contains_key(field);
    }
    false
}

fn get_field_string(action_data: &Option<serde_json::Value>, field: &str) -> Option<String> {
    action_data
        .as_ref()
        .and_then(|d| d.get(field))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn get_field_i32(action_data: &Option<serde_json::Value>, field: &str) -> Option<i32> {
    action_data
        .as_ref()
        .and_then(|d| d.get(field))
        .and_then(|v| {
            if v.is_i64() {
                v.as_i64().map(|v| v as i32)
            } else if v.is_f64() {
                v.as_f64().map(|v| v as i32)
            } else {
                None
            }
        })
}

pub async fn get_inventory_item_quantity(
    db_pool: &DbPool,
    agent_id: uuid::Uuid,
    item_id: &str,
) -> i32 {
    sqlx::query_scalar::<_, i32>(
        "SELECT COALESCE(SUM(quantity), 0) FROM agent_inventory WHERE agent_id = $1 AND item_id = $2",
    )
    .bind(agent_id)
    .bind(item_id)
    .fetch_one(db_pool)
    .await
    .unwrap_or(0)
}

fn is_placeholder_content(s: &str) -> bool {
    matches!(s, "..." | "…" | "。。。" | ".." | "。" | "-" | "--" | "---")
}

fn validate_field(intent: &Intent, field_validation: &FieldValidation) -> Result<(), GameError> {
    let field = &field_validation.field;
    let validation_type = &field_validation.validation_type;

    match validation_type.as_str() {
        FieldValidation::TYPE_NOT_EMPTY => {
            let value = get_field_string(&intent.action_data, field).ok_or_else(|| {
                GameError::InvalidActionData {
                    reason: format!("字段 {} 缺失", field),
                }
            })?;

            let trimmed = value.trim();
            if trimmed.is_empty() || is_placeholder_content(trimmed) {
                return Err(GameError::InvalidActionData {
                    reason: format!("字段 {} 不能为空或占位符", field),
                });
            }
        }
        FieldValidation::TYPE_MIN_VALUE => {
            let min_value = field_validation.get_i32("min_value").ok_or_else(|| {
                GameError::InvalidActionData {
                    reason: format!("字段 {} 的 min_value 验证参数缺失", field),
                }
            })?;

            let value = get_field_i32(&intent.action_data, field).ok_or_else(|| {
                GameError::InvalidActionData {
                    reason: format!("字段 {} 缺失或不是数字", field),
                }
            })?;

            if value < min_value {
                return Err(GameError::InvalidActionData {
                    reason: format!("字段 {} 的值必须 >= {}", field, min_value),
                });
            }
        }
        FieldValidation::TYPE_MAX_VALUE => {
            let max_value = field_validation.get_i32("max_value").ok_or_else(|| {
                GameError::InvalidActionData {
                    reason: format!("字段 {} 的 max_value 验证参数缺失", field),
                }
            })?;

            let value = get_field_i32(&intent.action_data, field).ok_or_else(|| {
                GameError::InvalidActionData {
                    reason: format!("字段 {} 缺失或不是数字", field),
                }
            })?;

            if value > max_value {
                return Err(GameError::InvalidActionData {
                    reason: format!("字段 {} 的值必须 <= {}", field, max_value),
                });
            }
        }
        FieldValidation::TYPE_MIN_LENGTH => {
            let min_length = field_validation.get_i32("min_length").ok_or_else(|| {
                GameError::InvalidActionData {
                    reason: format!("字段 {} 的 min_length 验证参数缺失", field),
                }
            })?;

            let value = get_field_string(&intent.action_data, field).ok_or_else(|| {
                GameError::InvalidActionData {
                    reason: format!("字段 {} 缺失", field),
                }
            })?;

            if value.len() < min_length as usize {
                return Err(GameError::InvalidActionData {
                    reason: format!("字段 {} 的长度必须 >= {}", field, min_length),
                });
            }
        }
        FieldValidation::TYPE_MAX_LENGTH => {
            let max_length = field_validation.get_i32("max_length").ok_or_else(|| {
                GameError::InvalidActionData {
                    reason: format!("字段 {} 的 max_length 验证参数缺失", field),
                }
            })?;

            let value = get_field_string(&intent.action_data, field).ok_or_else(|| {
                GameError::InvalidActionData {
                    reason: format!("字段 {} 缺失", field),
                }
            })?;

            if value.len() > max_length as usize {
                return Err(GameError::InvalidActionData {
                    reason: format!("字段 {} 的长度必须 <= {}", field, max_length),
                });
            }
        }
        _ => {}
    }

    Ok(())
}

/// 校验制造动作的配方知晓度
async fn validate_recipe_knowledge(
    intent: &Intent,
    agent_state: &AgentState,
    db_pool: &DbPool,
) -> Result<(), GameError> {
    let recipe_id = intent
        .action_data
        .as_ref()
        .and_then(|d| d.get("recipe_id"))
        .and_then(|v| v.as_str())
        .or_else(|| {
            intent
                .action_data
                .as_ref()
                .and_then(|d| d.get("item_id"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("");

    if recipe_id.is_empty() {
        return Ok(());
    }

    let known = crate::db::get_known_recipe_ids(db_pool, agent_state.agent_id)
        .await
        .unwrap_or_default();

    let knows = known.iter().any(|r| {
        r == recipe_id
            || crate::game_data::registry::RecipeRegistry::get(r)
                .map(|def| def.result_item == recipe_id)
                .unwrap_or(false)
    });

    if !knows {
        return Err(GameError::InvalidActionData {
            reason: format!("你尚未学会配方「{}」", recipe_id),
        });
    }

    Ok(())
}

/// 校验传授动作
async fn validate_teach_recipe(
    intent: &Intent,
    agent_state: &AgentState,
    _all_states: &[AgentState],
    db_pool: &DbPool,
) -> Result<(), GameError> {
    let action_data = &intent.action_data;
    let recipe_id = action_data
        .as_ref()
        .and_then(|d| d.get("recipe_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if recipe_id.is_empty() {
        return Err(GameError::InvalidActionData {
            reason: "教导需要指定配方 ID（recipe_id）".to_string(),
        });
    }

    let target_id_str = action_data
        .as_ref()
        .and_then(|d| d.get("target_agent_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if let Ok(target_uuid) = uuid::Uuid::parse_str(target_id_str)
        && target_uuid == agent_state.agent_id
    {
        return Err(GameError::InvalidActionData {
            reason: "不能向自己传授配方".to_string(),
        });
    }
    let candidates = vec![agent_state.agent_id];
    if let Some(target_uuid) =
        cyber_jianghu_protocol::resolve_agent_id_lenient(target_id_str, &candidates)
        && target_uuid == agent_state.agent_id
    {
        return Err(GameError::InvalidActionData {
            reason: "不能向自己传授配方".to_string(),
        });
    }

    let known = crate::db::get_known_recipe_ids(db_pool, agent_state.agent_id)
        .await
        .unwrap_or_default();
    if !known.contains(&recipe_id.to_string()) {
        return Err(GameError::InvalidActionData {
            reason: format!("你尚未学会配方「{}」，无法教导", recipe_id),
        });
    }

    Ok(())
}

fn validate_target_exists(intent: &Intent, all_states: &[AgentState]) -> Result<(), GameError> {
    let target_id_str =
        get_field_string(&intent.action_data, "target_agent_id").ok_or_else(|| {
            GameError::InvalidActionData {
                reason: "缺少 target_agent_id 字段".to_string(),
            }
        })?;

    let candidates: Vec<Uuid> = all_states.iter().map(|s| s.agent_id).collect();
    let target_id =
        cyber_jianghu_protocol::resolve_agent_id(&target_id_str, &candidates).map_err(|_| {
            GameError::InvalidActionData {
                reason: "无效的 target_agent_id".to_string(),
            }
        })?;

    if !all_states.iter().any(|s| s.agent_id == target_id) {
        return Err(GameError::TargetNotFound { target_id });
    }

    Ok(())
}

fn validate_target_alive(intent: &Intent, all_states: &[AgentState]) -> Result<(), GameError> {
    let target_id_str =
        get_field_string(&intent.action_data, "target_agent_id").ok_or_else(|| {
            GameError::InvalidActionData {
                reason: "缺少 target_agent_id 字段".to_string(),
            }
        })?;

    let candidates: Vec<Uuid> = all_states.iter().map(|s| s.agent_id).collect();
    let target_id =
        cyber_jianghu_protocol::resolve_agent_id(&target_id_str, &candidates).map_err(|_| {
            GameError::InvalidActionData {
                reason: "无效的 target_agent_id".to_string(),
            }
        })?;

    let target_state = all_states
        .iter()
        .find(|s| s.agent_id == target_id)
        .ok_or(GameError::TargetNotFound { target_id })?;

    if !target_state.is_alive {
        return Err(GameError::TargetDead { target_id });
    }

    Ok(())
}

fn validate_target_colocated(
    intent: &Intent,
    agent_state: &AgentState,
    all_states: &[AgentState],
) -> Result<(), GameError> {
    let target_id_str =
        get_field_string(&intent.action_data, "target_agent_id").ok_or_else(|| {
            GameError::InvalidActionData {
                reason: "缺少 target_agent_id 字段".to_string(),
            }
        })?;

    let candidates: Vec<Uuid> = all_states.iter().map(|s| s.agent_id).collect();
    let target_id =
        cyber_jianghu_protocol::resolve_agent_id(&target_id_str, &candidates).map_err(|_| {
            GameError::InvalidActionData {
                reason: "无效的 target_agent_id".to_string(),
            }
        })?;

    let target_state = all_states
        .iter()
        .find(|s| s.agent_id == target_id)
        .ok_or(GameError::TargetNotFound { target_id })?;

    if target_state.node_id != agent_state.node_id {
        return Err(GameError::InvalidActionData {
            reason: "目标不在同一地点".to_string(),
        });
    }

    Ok(())
}
