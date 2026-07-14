use crate::actions::{
    AttackData, CraftData, MoveData, ObserveData, ParsedActionData, QuData, SpeakData, TeachData,
    YongData, YuData, parse_action_data,
};
use crate::db::DbPool;
use crate::game_data::types::actions::{ValidationType, ValidatorKind};
use crate::game_data::types::ActionValidation;
use crate::game_data::ActionRegistry;
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
        other => Err(GameError::InvalidActionData {
            reason: format!("未知的动作类型: {}", other),
        }),
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
    }

    Ok(parsed)
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
                        get_inventory_item_quantity(db_pool, agent_state.agent_id, item_id).await;
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
                // 校验字段值（通常是 item_id）必须是 items.yaml 中已配置的合法物品。
                // 拦截 LLM 幻觉产生的、不在物品注册表中的无效 ID。
                // 若该字段在此动作类型上不存在（get_field_str 返回 None），跳过校验——
                // 字段存在性已由 required_fields / not_empty 校验覆盖，避免误伤非物品动作。
                let Some(value) = parsed.get_field_str(field) else {
                    continue;
                };
                if !crate::game_data::registry::ItemRegistry::exists(&value) {
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
    if !known.contains(&recipe_id.to_string()) {
        return Err(GameError::InvalidActionData {
            reason: format!("你尚未学会配方「{}」，无法教导", recipe_id),
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
