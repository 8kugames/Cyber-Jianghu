use crate::actions::{
    AttackData, CraftData, MoveData, ObserveData, ParsedActionData, QuData, SpeakData, TeachData,
    YongData, YuData, parse_action_data,
};
use crate::db::DbPool;
use crate::game_data::ActionRegistry;
use crate::game_data::types::ActionValidation;
use crate::game_data::types::actions::{ValidationType, ValidatorKind};
use crate::models::{AgentState, Intent};
use cyber_jianghu_protocol::GameError;
use uuid::Uuid;

/// 将 intent.action_data 反序列化为对应 typed struct
fn parse_action_data_by_type(intent: &Intent) -> Result<ParsedActionData, GameError> {
    // 吃/喝 归一化为 用（共享同一数据结构和执行器）
    let normalized = match intent.action_type.as_str() {
        "吃" | "喝" => "用",
        s => s,
    };
    match normalized {
        "予" => Ok(ParsedActionData::Yu(parse_action_data::<YuData>(
            &intent.action_data,
            "予",
        )?)),
        "取" => Ok(ParsedActionData::Qu(parse_action_data::<QuData>(
            &intent.action_data,
            "取",
        )?)),
        "用" => Ok(ParsedActionData::Yong(parse_action_data::<YongData>(
            &intent.action_data,
            "用",
        )?)),
        "说话" => Ok(ParsedActionData::Speak(parse_action_data::<SpeakData>(
            &intent.action_data,
            "说话",
        )?)),
        "移动" => Ok(ParsedActionData::Move(parse_action_data::<MoveData>(
            &intent.action_data,
            "移动",
        )?)),
        "观察" => Ok(ParsedActionData::Observe(parse_action_data::<ObserveData>(
            &intent.action_data,
            "观察",
        )?)),
        "攻击" => Ok(ParsedActionData::Attack(parse_action_data::<AttackData>(
            &intent.action_data,
            "攻击",
        )?)),
        "制造" => Ok(ParsedActionData::Craft(parse_action_data::<CraftData>(
            &intent.action_data,
            "制造",
        )?)),
        "教导" => Ok(ParsedActionData::Teach(parse_action_data::<TeachData>(
            &intent.action_data,
            "教导",
        )?)),
        "休整" => Ok(ParsedActionData::None),
        // 编译期闭集之外、注册表之内的动作（动作演化 approve 产物）：
        // validate_action 第一道门（ActionRegistry::get）已放行，此处不再拦截——
        // 拦截会让通过审议的演化动作永远收到"未知的动作类型"→ UnknownAction
        // → Agent 无限重复提案。数据驱动的 field_validations / requirements /
        // effects 对 Generic 形态照常生效
        _ => Ok(ParsedActionData::Generic(
            intent
                .action_data
                .clone()
                .unwrap_or(serde_json::Value::Null),
        )),
    }
}

/// 验证动作是否可以执行
///
/// 返回类型安全的 [`ParsedActionData`] 供执行层直接使用，消除双重解析。
pub async fn validate_action(
    intent: &Intent,
    agent_state: &AgentState,
    all_states: &[AgentState],
    db_pool: &DbPool,
) -> Result<ParsedActionData, GameError> {
    if !agent_state.is_alive {
        return Err(GameError::AgentDead {
            agent_id: agent_state.agent_id,
        });
    }

    let action_str = intent.action_type.as_str();
    let config = ActionRegistry::get(action_str).ok_or_else(|| GameError::InvalidActionData {
        reason: format!("未知的动作类型: {}", action_str),
    })?;
    // 吃/喝 共享"用"的校验配置：归一化后再取一次 config，使 field_validations
    // （含 item_exists）对快捷动作同样生效。与 parse_action_data_by_type 的归一化对齐。
    let config = if matches!(action_str, "吃" | "喝") {
        ActionRegistry::get("用").unwrap_or(config)
    } else {
        config
    };

    validate_generic_requirements(intent, agent_state, db_pool).await?;

    // 先反序列化到 typed struct —— 类型验证本身即为字段存在性/类型校验
    let parsed = parse_action_data_by_type(intent)?;

    // ValidatorKind 校验（在 typed 数据上执行）
    match config.validator_kind {
        Some(ValidatorKind::RecipeKnowledge) => {
            validate_recipe_knowledge_typed(&parsed, agent_state, db_pool).await?;
        }
        Some(ValidatorKind::TeachRecipe) => {
            validate_teach_recipe_typed(&parsed, agent_state, all_states, db_pool).await?;
        }
        None => {}
    }

    // field_validations（在 typed 数据上执行，不再需要 has_field/get_field_string）
    if let Some(validation) = &config.validation {
        apply_field_validations(&parsed, validation)?;

        if validation.requires_target.unwrap_or(false) {
            validate_target_exists_typed(&parsed, all_states)?;
        }
        if validation.requires_target_alive.unwrap_or(false) {
            validate_target_alive_typed(&parsed, all_states)?;
        }
        if validation.requires_target_colocated.unwrap_or(false) {
            validate_target_colocated_typed(&parsed, agent_state, all_states)?;
        }
        if validation.requires_item_ownership.unwrap_or(false) {
            validate_item_ownership(&parsed, intent, agent_state, db_pool).await?;
        }
    }

    Ok(parsed)
}

/// 执行前背包持有预检（requires_item_ownership）
///
/// Agent 侧天魂 layer0 基于可能过期的 WorldState 快照校验，拦截不了快照过期
/// 竞态（同 tick 前序意图已消耗该物品、期间被夺等）——Agent 真诚地以为自己
/// 持有，layer0 不拦也不应拦。此处按权威 DB 在验证阶段前置拦截，失败走
/// action_failed 反馈具体原因，供 Agent 下一 tick 自纠（而非执行期回滚的
/// 「状态变更未能全部应用」笼统文案）。
async fn validate_item_ownership(
    parsed: &ParsedActionData,
    intent: &Intent,
    agent_state: &AgentState,
    db_pool: &DbPool,
) -> Result<(), GameError> {
    // item_id 为完整 uuid，须反解回内部 item_id 再查库（与执行器一致；
    // 直连查询恒为 0 会造成全量误拒）
    let Some(item_uuid) = parsed.get_field_str("item_id") else {
        // 字段缺失由 required_fields / not_empty 校验覆盖
        return Ok(());
    };
    let Some(internal_id) = crate::items::resolve_item_id(&item_uuid) else {
        // 未注册物品由 item_exists 校验覆盖（其报错信息更精确）
        return Ok(());
    };
    let needed = parsed.get_field_i32("quantity").unwrap_or(1).max(1);
    let owned = get_inventory_item_quantity(db_pool, agent_state.agent_id, &internal_id)
        .await
        .map_err(|_| storage_unavailable())?;
    if owned < needed {
        let name = crate::display::display_item_name(&internal_id);
        return Err(GameError::Unknown(format!(
            "你没有{}（需要 {}，持有 {}），无法{}",
            name,
            needed,
            owned,
            intent.action_type.as_str()
        )));
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
            match req.requirement_type {
                cyber_jianghu_protocol::RequirementType::Attribute => {
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
                cyber_jianghu_protocol::RequirementType::Item => {
                    let item_id = req.get_str("item_id").unwrap_or("unknown");
                    let min_qty = req.get_i32("quantity").unwrap_or(1);

                    let item_quantity =
                        get_inventory_item_quantity(db_pool, agent_state.agent_id, item_id)
                            .await
                            .map_err(|_| storage_unavailable())?;
                    if item_quantity < min_qty {
                        return Err(GameError::Unknown(format!(
                            "物品 {} 不足: 需要 {}, 当前 {}",
                            item_id, min_qty, item_quantity
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}

pub async fn get_inventory_item_quantity(
    db_pool: &DbPool,
    agent_id: uuid::Uuid,
    item_id: &str,
) -> Result<i32, sqlx::Error> {
    sqlx::query_scalar::<_, i32>(
        "SELECT COALESCE(SUM(quantity), 0) FROM agent_inventory WHERE agent_id = $1 AND item_id = $2",
    )
    .bind(agent_id)
    .bind(item_id)
    .fetch_one(db_pool)
    .await
}

/// 持有量查询失败的统一错误（此前 unwrap_or(0) 把 DB 故障伪装成「持有 0」，
/// 诱导 Agent 错误自纠——诚实报存储异常并给重试指引）
fn storage_unavailable() -> GameError {
    GameError::Unknown("背包持有量查询失败（服务端存储异常），请稍后重试".to_string())
}

fn is_placeholder_content(s: &str) -> bool {
    matches!(s, "..." | "…" | "。。。" | ".." | "。" | "-" | "--" | "---")
}

/// 在 typed [`ParsedActionData`] 上执行 field_validations
fn apply_field_validations(
    parsed: &ParsedActionData,
    validation: &ActionValidation,
) -> Result<(), GameError> {
    for fv in &validation.field_validations {
        let field = &fv.field;
        match &fv.validation_type {
            ValidationType::NotEmpty => {
                let value =
                    parsed
                        .get_field_str(field)
                        .ok_or_else(|| GameError::InvalidActionData {
                            reason: format!("字段 {} 缺失", field),
                        })?;
                let trimmed = value.trim();
                if trimmed.is_empty() || is_placeholder_content(trimmed) {
                    return Err(GameError::InvalidActionData {
                        reason: format!("字段 {} 不能为空或占位符", field),
                    });
                }
            }
            ValidationType::MinValue => {
                let min_value =
                    fv.get_i32("min_value")
                        .ok_or_else(|| GameError::InvalidActionData {
                            reason: format!("字段 {} 的 min_value 验证参数缺失", field),
                        })?;
                let value =
                    parsed
                        .get_field_i32(field)
                        .ok_or_else(|| GameError::InvalidActionData {
                            reason: format!("字段 {} 缺失或不是数字", field),
                        })?;
                if value < min_value {
                    return Err(GameError::InvalidActionData {
                        reason: format!("字段 {} 的值必须 >= {}", field, min_value),
                    });
                }
            }
            ValidationType::MaxValue => {
                let max_value =
                    fv.get_i32("max_value")
                        .ok_or_else(|| GameError::InvalidActionData {
                            reason: format!("字段 {} 的 max_value 验证参数缺失", field),
                        })?;
                let value =
                    parsed
                        .get_field_i32(field)
                        .ok_or_else(|| GameError::InvalidActionData {
                            reason: format!("字段 {} 缺失或不是数字", field),
                        })?;
                if value > max_value {
                    return Err(GameError::InvalidActionData {
                        reason: format!("字段 {} 的值必须 <= {}", field, max_value),
                    });
                }
            }
            ValidationType::MinLength => {
                let min_length =
                    fv.get_i32("min_length")
                        .ok_or_else(|| GameError::InvalidActionData {
                            reason: format!("字段 {} 的 min_length 验证参数缺失", field),
                        })?;
                let value =
                    parsed
                        .get_field_str(field)
                        .ok_or_else(|| GameError::InvalidActionData {
                            reason: format!("字段 {} 缺失", field),
                        })?;
                if value.len() < min_length as usize {
                    return Err(GameError::InvalidActionData {
                        reason: format!("字段 {} 的长度必须 >= {}", field, min_length),
                    });
                }
            }
            ValidationType::MaxLength => {
                let max_length =
                    fv.get_i32("max_length")
                        .ok_or_else(|| GameError::InvalidActionData {
                            reason: format!("字段 {} 的 max_length 验证参数缺失", field),
                        })?;
                let value =
                    parsed
                        .get_field_str(field)
                        .ok_or_else(|| GameError::InvalidActionData {
                            reason: format!("字段 {} 缺失", field),
                        })?;
                if value.len() > max_length as usize {
                    return Err(GameError::InvalidActionData {
                        reason: format!("字段 {} 的长度必须 <= {}", field, max_length),
                    });
                }
            }
            ValidationType::ItemExists => {
                // 严格 uuid 审查：与执行器 resolve_item_id（仅接受完整 uuid）对齐，
                // 全链路强制 uuid 引用。拒绝两类输入：
                // 1. 非完整 uuid（裸物品名/短码/格式错误）；
                // 2. 合法 uuid 但反解不到注册物品（items.yaml 配置漂移或 LLM 幻觉）。
                // 若该字段在此动作类型上不存在（get_field_str 返回 None），跳过校验——
                // 字段存在性已由 required_fields / not_empty 校验覆盖，避免误伤非物品动作。
                let Some(value) = parsed.get_field_str(field) else {
                    continue;
                };
                if Uuid::parse_str(&value).is_err() {
                    return Err(GameError::InvalidActionData {
                        reason: format!(
                            "物品 \"{}\" 非法：item_id 必须是完整 uuid（36 位，与背包/世界状态一致），不接受名称或短码",
                            value
                        ),
                    });
                }
                if crate::items::resolve_item_id(&value).is_none() {
                    return Err(GameError::InvalidActionData {
                        reason: format!("物品 \"{}\" 不存在（不在物品配置中）", value),
                    });
                }
            }
        }
    }
    Ok(())
}

/// 校验制造动作的配方知晓度（基于 typed CraftData）
async fn validate_recipe_knowledge_typed(
    parsed: &ParsedActionData,
    agent_state: &AgentState,
    db_pool: &DbPool,
) -> Result<(), GameError> {
    let recipe_id = match parsed {
        ParsedActionData::Craft(data) => Some(data.recipe_id.as_str()),
        _ => None,
    };
    let Some(recipe_id) = recipe_id else {
        return Ok(());
    };

    if recipe_id.is_empty() {
        return Ok(());
    }

    let known = crate::db::get_known_recipe_ids(db_pool, agent_state.agent_id)
        .await
        .unwrap_or_default();

    // Agent 侧只见到 uuid（broadcaster 下发），DB 已知清单是内部 id：
    // 先归一提交值（uuid / 内部 id / 产物物品 id）再比对，三种输入形态一致可用
    let resolved = crate::game_data::registry::RecipeRegistry::normalize(recipe_id);
    let knows = resolved
        .as_ref()
        .is_some_and(|rid| known.iter().any(|r| r == rid));
    if !knows {
        return Err(GameError::InvalidActionData {
            reason: match &resolved {
                Some(_) => format!("你尚未学会配方「{}」", recipe_id),
                None => format!("配方不存在或无效: {}", recipe_id),
            },
        });
    }

    Ok(())
}

/// 校验传授动作（基于 typed TeachData）
async fn validate_teach_recipe_typed(
    parsed: &ParsedActionData,
    agent_state: &AgentState,
    _all_states: &[AgentState],
    db_pool: &DbPool,
) -> Result<(), GameError> {
    let (recipe_id, target_agent_id) = match parsed {
        ParsedActionData::Teach(data) => (data.recipe_id.as_str(), data.target_agent_id.as_str()),
        _ => {
            return Err(GameError::InvalidActionData {
                reason: "教导需要指定配方 ID（recipe_id）".to_string(),
            });
        }
    };

    if recipe_id.is_empty() {
        return Err(GameError::InvalidActionData {
            reason: "教导需要指定配方 ID（recipe_id）".to_string(),
        });
    }

    if let Ok(target_uuid) = uuid::Uuid::parse_str(target_agent_id)
        && target_uuid == agent_state.agent_id
    {
        return Err(GameError::InvalidActionData {
            reason: "不能向自己传授配方".to_string(),
        });
    }
    let candidates = vec![agent_state.agent_id];
    if let Some(target_uuid) =
        cyber_jianghu_protocol::resolve_agent_id_lenient(target_agent_id, &candidates)
        && target_uuid == agent_state.agent_id
    {
        return Err(GameError::InvalidActionData {
            reason: "不能向自己传授配方".to_string(),
        });
    }

    let known = crate::db::get_known_recipe_ids(db_pool, agent_state.agent_id)
        .await
        .unwrap_or_default();
    // 与制造同规：归一（uuid / 内部 id / 产物物品 id）后与已知内部 id 比对
    let resolved = crate::game_data::registry::RecipeRegistry::normalize(recipe_id);
    let knows = resolved
        .as_ref()
        .is_some_and(|rid| known.iter().any(|r| r == rid));
    if !knows {
        return Err(GameError::InvalidActionData {
            reason: match &resolved {
                Some(_) => format!("你尚未学会配方「{}」，无法教导", recipe_id),
                None => format!("配方不存在或无效: {}", recipe_id),
            },
        });
    }

    Ok(())
}

fn validate_target_exists_typed(
    parsed: &ParsedActionData,
    all_states: &[AgentState],
) -> Result<(), GameError> {
    let target_id_str =
        parsed
            .get_target_agent_id()
            .ok_or_else(|| GameError::InvalidActionData {
                reason: "缺少 target_agent_id 字段".to_string(),
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

fn validate_target_alive_typed(
    parsed: &ParsedActionData,
    all_states: &[AgentState],
) -> Result<(), GameError> {
    let target_id_str =
        parsed
            .get_target_agent_id()
            .ok_or_else(|| GameError::InvalidActionData {
                reason: "缺少 target_agent_id 字段".to_string(),
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

fn validate_target_colocated_typed(
    parsed: &ParsedActionData,
    agent_state: &AgentState,
    all_states: &[AgentState],
) -> Result<(), GameError> {
    let target_id_str =
        parsed
            .get_target_agent_id()
            .ok_or_else(|| GameError::InvalidActionData {
                reason: "缺少 target_agent_id 字段".to_string(),
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

#[cfg(test)]
mod item_exists_tests {
    use super::*;
    use crate::actions::{ParsedActionData, YongData};
    use crate::game_data::types::actions::{ActionValidation, FieldValidation};
    use std::collections::HashMap;

    fn item_exists_validation() -> ActionValidation {
        let mut validation = ActionValidation::default();
        validation.field_validations.push(FieldValidation {
            field: "item_id".to_string(),
            validation_type: ValidationType::ItemExists,
            params: HashMap::new(),
        });
        validation
    }

    fn yong(item_id: &str) -> ParsedActionData {
        ParsedActionData::Yong(YongData {
            item_id: item_id.to_string(),
        })
    }

    #[test]
    fn item_exists_accepts_registered_full_uuid() {
        crate::game_data::init_test_registry();
        let uuid = crate::items::item_uuid("馒头").to_string();
        assert!(apply_field_validations(&yong(&uuid), &item_exists_validation()).is_ok());
    }

    #[test]
    fn item_exists_rejects_bare_item_name() {
        crate::game_data::init_test_registry();
        let err = apply_field_validations(&yong("馒头"), &item_exists_validation()).unwrap_err();
        assert!(err.to_string().contains("完整 uuid"), "got: {err}");
    }

    #[test]
    fn item_exists_rejects_short_uuid() {
        crate::game_data::init_test_registry();
        let short = crate::items::item_uuid("馒头").to_string()[..8].to_string();
        let err = apply_field_validations(&yong(&short), &item_exists_validation()).unwrap_err();
        assert!(err.to_string().contains("完整 uuid"), "got: {err}");
    }

    #[test]
    fn item_exists_rejects_unregistered_uuid() {
        crate::game_data::init_test_registry();
        let fake = uuid::Uuid::new_v4().to_string();
        let err = apply_field_validations(&yong(&fake), &item_exists_validation()).unwrap_err();
        assert!(err.to_string().contains("不在物品配置中"), "got: {err}");
    }

    #[test]
    fn item_exists_skips_when_field_absent() {
        crate::game_data::init_test_registry();
        assert!(
            apply_field_validations(&ParsedActionData::None, &item_exists_validation()).is_ok()
        );
    }
}

#[cfg(test)]
mod item_ownership_tests {
    use super::*;

    #[test]
    fn requires_item_ownership_deserializes_from_config() {
        // 数据驱动接线：actions.yaml 的 requires_item_ownership 经 JSON 反序列化
        // 进入 ActionValidation（用/予 配置该标志，吃/喝 归一化复用用的配置）
        let v: ActionValidation = serde_json::from_str(
            r#"{"required_fields":["item_id"],"requires_item_ownership":true}"#,
        )
        .unwrap();
        assert_eq!(v.requires_item_ownership, Some(true));
    }

    #[test]
    fn requires_item_ownership_defaults_to_none() {
        // 旧配置/其他动作未携带该标志时默认关闭（预检不启用，行为不变）
        let v: ActionValidation =
            serde_json::from_str(r#"{"required_fields":["target_agent_id"]}"#).unwrap();
        assert_eq!(v.requires_item_ownership, None);
    }
}
