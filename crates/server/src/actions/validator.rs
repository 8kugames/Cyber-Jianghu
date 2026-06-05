// ============================================================================
// 动作验证器 - 完全数据驱动
// ============================================================================
//
// 实现动作执行前的验证逻辑
// 所有验证规则从配置文件读取，不硬编码任何动作类型
// ============================================================================

use crate::db::DbPool;
use crate::game_data::types::{ActionValidation, FieldValidation};
use crate::game_data::{ActionRegistry, ActionRequirement};
use crate::models::{AgentState, Intent};
use cyber_jianghu_protocol::GameError;
use uuid::Uuid;

/// 验证动作是否可以执行（完全数据驱动）
///
/// 在动作执行前进行验证，确保：
/// 1. Agent 存活
/// 2. 动作类型已注册
/// 3. 根据配置中的验证规则进行验证
/// 4. 满足通用需求（如属性要求）
///
/// # 参数
/// - intent: Agent 上报的意图
/// - agent_state: Agent 当前状态
/// - all_states: 所有 Agent 状态（用于验证目标）
/// - db_pool: 数据库连接池（用于验证物品需求）
///
/// # 返回
/// - Ok(()): 验证通过
/// - Err(GameError): 验证失败
pub async fn validate_action(
    intent: &Intent,
    agent_state: &AgentState,
    all_states: &[AgentState],
    db_pool: &DbPool,
) -> Result<(), GameError> {
    // 检查 Agent 是否存活
    if !agent_state.is_alive {
        return Err(GameError::AgentDead {
            agent_id: agent_state.agent_id,
        });
    }

    // 获取动作配置
    let action_str = intent.action_type.as_str();
    let config = ActionRegistry::get(action_str).ok_or_else(|| GameError::InvalidActionData {
        reason: format!("未知的动作类型: {}", action_str),
    })?;

    // 验证通用需求
    validate_generic_requirements(intent, agent_state, db_pool).await?;

    // 制造动作：校验配方知晓度
    if action_str == "制造" {
        validate_recipe_knowledge(intent, agent_state, db_pool).await?;
    }

    // 传授动作：校验传授者配方知晓度 + 同地点 + 防自传授
    if action_str == "传授" {
        validate_teach_recipe(intent, agent_state, all_states, db_pool).await?;
    }

    // 动作冷却检查 — 需要 actions.yaml 添加 cooldown 字段
    // + AgentState.last_action_ticks: HashMap<String, i64>
    // + current_tick - last_action_ticks[action_type] >= cooldown

    // 数据驱动验证：根据配置中的 validation 规则进行验证
    if let Some(validation) = &config.validation {
        validate_by_rules(intent, agent_state, all_states, validation)?;
    }

    Ok(())
}

/// 根据配置中的验证规则进行验证（数据驱动）
fn validate_by_rules(
    intent: &Intent,
    agent_state: &AgentState,
    all_states: &[AgentState],
    validation: &ActionValidation,
) -> Result<(), GameError> {
    let action_data = intent.action_data.clone();

    // 构造临时 Intent 用于后续验证
    let temp_intent = Intent {
        action_data: action_data.clone(),
        ..intent.clone()
    };

    // 验证必需的字段
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

    // 验证字段规则
    for field_validation in &validation.field_validations {
        validate_field(&temp_intent, field_validation)?;
    }

    // 验证目标 Agent
    if validation.requires_target.unwrap_or(false) {
        validate_target_exists(&temp_intent, all_states)?;
    }

    // 验证目标存活
    if validation.requires_target_alive.unwrap_or(false) {
        validate_target_alive(&temp_intent, all_states)?;
    }

    // 验证目标同地点
    if validation.requires_target_colocated.unwrap_or(false) {
        validate_target_colocated(&temp_intent, agent_state, all_states)?;
    }

    Ok(())
}

/// 验证通用需求（数据驱动方式）
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
                _ => {
                    // 未知类型的需求，跳过（可扩展）
                }
            }
        }
    }
    Ok(())
}

/// 检查 action_data 中是否存在指定字段
fn has_field(action_data: &Option<serde_json::Value>, field: &str) -> bool {
    if let Some(data) = action_data
        && let Some(obj) = data.as_object()
    {
        return obj.contains_key(field);
    }
    false
}

/// 获取字段的字符串值
fn get_field_string(action_data: &Option<serde_json::Value>, field: &str) -> Option<String> {
    action_data
        .as_ref()
        .and_then(|d| d.get(field))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// 获取字段的 i32 值
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

/// 获取背包中物品的数量
pub async fn get_inventory_item_quantity(
    db_pool: &DbPool,
    agent_id: uuid::Uuid,
    item_id: &str,
) -> i32 {
    sqlx::query_scalar::<_, i32>(
        "SELECT COALESCE(SUM(quantity), 0) FROM agent_inventory WHERE agent_id = $1 AND item_id = $2"
    )
    .bind(agent_id)
    .bind(item_id)
    .fetch_one(db_pool)
    .await
    .unwrap_or(0)
}

/// 检测 LLM 输出的占位符内容（如 "..." "…" "。。。"）
///
/// LLM 偶尔用省略号替代实际对话内容，导致前端经历日志显示 "..." 而非实际文字
fn is_placeholder_content(s: &str) -> bool {
    matches!(s, "..." | "…" | "。。。" | ".." | "。" | "-" | "--" | "---")
}

/// 验证单个字段
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
        _ => {
            // 未知验证类型，跳过
        }
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

    // recipe_id 或 item_id 匹配：LLM 可能传配方 ID 或产物 ID
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

/// 校验传授动作：传授者配方知晓度 + 防自传授
/// 同地点校验由数据驱动的 requires_target_colocated 处理（actions.yaml）
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
            reason: "传授需要指定配方 ID（recipe_id）".to_string(),
        });
    }

    // 防自传授
    let target_id_str = action_data
        .as_ref()
        .and_then(|d| d.get("target_agent_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if let Ok(target_uuid) = uuid::Uuid::parse_str(target_id_str) {
        if target_uuid == agent_state.agent_id {
            return Err(GameError::InvalidActionData {
                reason: "不能向自己传授配方".to_string(),
            });
        }
    }

    // 传授者必须知道该配方
    let known = crate::db::get_known_recipe_ids(db_pool, agent_state.agent_id)
        .await
        .unwrap_or_default();
    if !known.contains(&recipe_id.to_string()) {
        return Err(GameError::InvalidActionData {
            reason: format!("你尚未学会配方「{}」，无法传授", recipe_id),
        });
    }

    Ok(())
}

/// 验证目标 Agent 存在
fn validate_target_exists(intent: &Intent, all_states: &[AgentState]) -> Result<(), GameError> {
    let target_id_str =
        get_field_string(&intent.action_data, "target_agent_id").ok_or_else(|| {
            GameError::InvalidActionData {
                reason: "缺少 target_agent_id 字段".to_string(),
            }
        })?;

    let target_id = Uuid::parse_str(&target_id_str).map_err(|_| GameError::InvalidActionData {
        reason: format!("无效的 target_agent_id: {}", target_id_str),
    })?;

    let target_exists = all_states.iter().any(|s| s.agent_id == target_id);

    if !target_exists {
        return Err(GameError::TargetNotFound { target_id });
    }

    Ok(())
}

/// 验证目标 Agent 存活
fn validate_target_alive(intent: &Intent, all_states: &[AgentState]) -> Result<(), GameError> {
    let target_id_str =
        get_field_string(&intent.action_data, "target_agent_id").ok_or_else(|| {
            GameError::InvalidActionData {
                reason: "缺少 target_agent_id 字段".to_string(),
            }
        })?;

    let target_id = Uuid::parse_str(&target_id_str).map_err(|_| GameError::InvalidActionData {
        reason: format!("无效的 target_agent_id: {}", target_id_str),
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

/// 验证目标 Agent 与发起者在同一地点
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

    let target_id = Uuid::parse_str(&target_id_str).map_err(|_| GameError::InvalidActionData {
        reason: format!("无效的 target_agent_id: {}", target_id_str),
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

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_data::init_test_registry;
    use crate::models::ActionType;

    fn create_test_intent(
        action_type: ActionType,
        action_data: Option<serde_json::Value>,
    ) -> Intent {
        Intent {
            intent_id: Uuid::new_v4(),
            agent_id: Uuid::new_v4(),
            tick_id: 1,
            thought_log: None,
            action_type,
            action_data,
            priority: 5,
            reflector_thought: None,
            narrative: None,
            already_broadcast: false,
            session_id: None,
            subsequent_intents: vec![],
            chaos_marker: None,
            dream_marker: None,
        }
    }

    fn create_test_state(agent_id: Uuid, is_alive: bool) -> AgentState {
        let mut state = AgentState::new(agent_id, 1);
        let _ = state.status.set("hp", if is_alive { 100 } else { 0 });
        let _ = state.status.set("stamina", 100);
        let _ = state.status.set("hunger", 100);
        let _ = state.status.set("thirst", 100);
        state.is_alive = is_alive;
        state
    }

    #[tokio::test]
    async fn test_validate_idle() {
        init_test_registry();
        let agent_id = Uuid::new_v4();
        let intent = create_test_intent(ActionType::new("休息"), None);
        let state = create_test_state(agent_id, true);

        let db_pool = DbPool::connect_lazy("postgres://postgres@localhost/postgres").unwrap();
        let result: Result<(), GameError> = validate_action(&intent, &state, &[], &db_pool).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_speak_valid() {
        init_test_registry();
        let agent_id = Uuid::new_v4();
        let data = serde_json::json!({ "content": "大家好！" });
        let intent = create_test_intent(ActionType::new("说话"), Some(data));
        let state = create_test_state(agent_id, true);

        let db_pool = DbPool::connect_lazy("postgres://postgres@localhost/postgres").unwrap();
        let result: Result<(), GameError> = validate_action(&intent, &state, &[], &db_pool).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_speak_empty() {
        init_test_registry();
        let agent_id = Uuid::new_v4();
        let data = serde_json::json!({ "content": "" });
        let intent = create_test_intent(ActionType::new("说话"), Some(data));
        let state = create_test_state(agent_id, true);

        let db_pool = DbPool::connect_lazy("postgres://postgres@localhost/postgres").unwrap();
        let result: Result<(), GameError> = validate_action(&intent, &state, &[], &db_pool).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_give_target_dead() {
        init_test_registry();
        let agent_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let data = serde_json::json!({
            "target_agent_id": target_id.to_string(),
            "item_id": "馒头",
            "quantity": 1
        });
        let intent = create_test_intent(ActionType::new("给予"), Some(data));
        let state = create_test_state(agent_id, true);
        let target_state = create_test_state(target_id, false);

        let db_pool = DbPool::connect_lazy("postgres://postgres@localhost/postgres").unwrap();
        let result: Result<(), GameError> =
            validate_action(&intent, &state, &[target_state], &db_pool).await;
        assert!(matches!(result, Err(GameError::TargetDead { .. })));
    }

    #[tokio::test]
    async fn test_validate_give_invalid_quantity() {
        init_test_registry();
        let agent_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let data = serde_json::json!({
            "target_agent_id": target_id.to_string(),
            "item_id": "馒头",
            "quantity": 0
        });
        let intent = create_test_intent(ActionType::new("给予"), Some(data));
        let state = create_test_state(agent_id, true);
        let target_state = create_test_state(target_id, true);

        let db_pool = DbPool::connect_lazy("postgres://postgres@localhost/postgres").unwrap();
        let result: Result<(), GameError> =
            validate_action(&intent, &state, &[target_state], &db_pool).await;
        assert!(matches!(result, Err(GameError::InvalidActionData { .. })));
    }

    #[tokio::test]
    async fn test_validate_agent_dead() {
        init_test_registry();
        let agent_id = Uuid::new_v4();
        let intent = create_test_intent(ActionType::new("休息"), None);
        let state = create_test_state(agent_id, false);

        let db_pool = DbPool::connect_lazy("postgres://postgres@localhost/postgres").unwrap();
        let result: Result<(), GameError> = validate_action(&intent, &state, &[], &db_pool).await;
        assert!(matches!(result, Err(GameError::AgentDead { .. })));
    }

    #[tokio::test]
    async fn test_validate_unknown_action() {
        init_test_registry();
        let agent_id = Uuid::new_v4();
        let intent = create_test_intent(ActionType::new("unknown_action"), None);
        let state = create_test_state(agent_id, true);

        let db_pool = DbPool::connect_lazy("postgres://postgres@localhost/postgres").unwrap();
        let result: Result<(), GameError> = validate_action(&intent, &state, &[], &db_pool).await;
        assert!(matches!(result, Err(GameError::InvalidActionData { .. })));
    }

    #[tokio::test]
    async fn test_validate_attack() {
        init_test_registry();
        let agent_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let data = serde_json::json!({
            "target_agent_id": target_id.to_string()
        });
        let intent = create_test_intent(ActionType::new("攻击"), Some(data));
        let state = create_test_state(agent_id, true);
        let target_state = create_test_state(target_id, true);

        let db_pool = DbPool::connect_lazy("postgres://postgres@localhost/postgres").unwrap();
        let result: Result<(), GameError> =
            validate_action(&intent, &state, &[target_state], &db_pool).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_move() {
        init_test_registry();
        let agent_id = Uuid::new_v4();
        let data = serde_json::json!({
            "target_location": "inn"
        });
        let intent = create_test_intent(ActionType::new("移动"), Some(data));
        let state = create_test_state(agent_id, true);

        let db_pool = DbPool::connect_lazy("postgres://postgres@localhost/postgres").unwrap();
        let result: Result<(), GameError> = validate_action(&intent, &state, &[], &db_pool).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_move_empty_location() {
        init_test_registry();
        let agent_id = Uuid::new_v4();
        let data = serde_json::json!({
            "target_location": ""
        });
        let intent = create_test_intent(ActionType::new("移动"), Some(data));
        let state = create_test_state(agent_id, true);

        let db_pool = DbPool::connect_lazy("postgres://postgres@localhost/postgres").unwrap();
        let result: Result<(), GameError> = validate_action(&intent, &state, &[], &db_pool).await;
        assert!(matches!(result, Err(GameError::InvalidActionData { .. })));
    }
}
