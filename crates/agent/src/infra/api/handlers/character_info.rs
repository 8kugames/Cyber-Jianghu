// 角色信息 API Handlers
// ============================================================================

use axum::{
    extract::{Path as AxumPath, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use serde::Serialize;
use std::collections::HashMap;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::config::CharacterStatus;

use super::HttpApiState;
use super::basic::ErrorResponse;
use super::character_helpers::{get_active_character, get_character_by_id_sync};

/// 角色信息响应（合并配置文件 + WorldState 实时数据）
#[derive(Debug, Serialize)]
pub struct CharacterInfoResponse {
    // === 配置文件数据（注册时提供） ===
    /// 角色 ID
    pub agent_id: Option<String>,
    /// 服务器地址
    pub server_url: Option<String>,
    /// 姓名
    pub name: String,
    /// 年龄
    pub age: u8,
    /// 性别
    pub gender: String,
    /// 外貌描述
    pub appearance: Option<String>,
    /// 身份背景
    pub identity: Option<String>,
    /// 性格特征
    pub personality: Vec<String>,
    /// 核心价值观
    pub values: Vec<String>,

    // === 注册信息 ===
    /// 注册时间（ISO 8601 格式）
    pub registered_at: Option<String>,

    // === WorldState 实时数据 ===
    /// 当前属性（带叙事描述）
    pub attributes: Option<serde_json::Value>,
    /// 先天属性（注册时的属性值）
    pub birth_attributes: Option<serde_json::Value>,
    /// 持有物品
    pub inventory: Option<serde_json::Value>,
    /// 派生属性（带叙事描述）
    pub derived_attributes: Option<serde_json::Value>,
    /// 当前位置
    pub location: Option<String>,
    /// 当前 Tick
    pub tick_id: Option<i64>,
    /// 游戏时间
    pub world_time: Option<serde_json::Value>,

    // === 状态 ===
    /// 角色状态（alive, dead, etc.）
    pub status: Option<String>,
    /// 数据是否来自缓存（true = 数据可能已过时）
    pub is_stale: bool,

    // === 技能 ===
    /// 已掌握技能列表
    pub skills: Vec<SkillBrief>,
}

/// 技能简要信息（API 响应用）
#[derive(Debug, serde::Serialize)]
pub struct SkillBrief {
    pub skill_id: String,
    pub name: String,
}

/// 获取角色信息
///
/// GET /api/v1/character - 获取当前角色完整信息
///
/// 数据来源：
/// - 配置文件：name, age, gender, appearance, identity, personality, values
/// - WorldState：attributes, inventory, location, tick_id, world_time
pub(crate) async fn get_character_handler(State(state): State<HttpApiState>) -> impl IntoResponse {
    // 1. 从文件系统读取活跃角色配置
    let character = match get_active_character(&state).await {
        Ok(Some(ch)) => ch,
        Ok(None) => {
            return (
                StatusCode::PRECONDITION_FAILED,
                Json(ErrorResponse {
                    error_code: "character_not_registered".to_string(),
                    message: "角色尚未注册，请先创建角色".to_string(),
                }),
            )
                .into_response();
        }
        Err(e) => {
            error!("读取角色配置失败: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error_code: "character_read_error".to_string(),
                    message: format!("读取角色配置失败: {}", e),
                }),
            )
                .into_response();
        }
    };

    // 2. 加载叙事配置（用于属性描述）
    let narrative_config = state.narrative_config.read().await.clone();

    // 3. 从当前 WorldState 获取实时状态
    let current = state.current_state.read().await;

    // 是否使用缓存数据（当角色已死或服务器未连接时）
    let is_dead_flag = state.is_dead.load(std::sync::atomic::Ordering::Relaxed);
    let is_stale = current.is_none() || is_dead_flag;

    let (agent_id, raw_attributes, inventory, location, tick_id, world_time) =
        match current.as_ref() {
            Some(ws) => {
                let agent_id = ws.agent_id.map(|id| id.to_string());
                let attrs = serde_json::to_value(&ws.self_state.attributes).ok();
                let inv = serde_json::to_value(&ws.self_state.inventory).ok();
                let loc = Some(format!("{} ({})", ws.location.name, ws.location.node_type));
                let time = enrich_world_time_json(&ws.world_time);
                (agent_id, attrs, inv, loc, Some(ws.tick_id), time)
            }
            None => {
                // 降级使用配置数据（birth_attributes 作为 attributes 的兜底）
                let fallback_attrs = character
                    .birth_attributes
                    .as_ref()
                    .and_then(|a| serde_json::to_value(a).ok());
                (
                    character.agent_id.map(|id| id.to_string()),
                    fallback_attrs,
                    None,
                    None,
                    None,
                    None,
                )
            }
        };

    // 4. 计算角色状态（在 move attributes 之前）
    // 优先使用 is_dead 标志（当 AgentDied 消息已收到但 WorldState 尚未更新时）
    let status = if state.is_dead.load(std::sync::atomic::Ordering::Relaxed) {
        Some("dead".to_string())
    } else {
        raw_attributes
            .as_ref()
            .and_then(|a| a.get("hp"))
            .and_then(|hp| hp.as_i64())
            .map(|hp| if hp > 0 { "alive" } else { "dead" }.to_string())
            .or_else(|| match character.status {
                CharacterStatus::Dead => Some("dead".to_string()),
                CharacterStatus::Retired => Some("retired".to_string()),
                CharacterStatus::Alive => Some("alive".to_string()),
            })
    };

    // 5. 丰富属性数据（添加叙事描述）
    let attributes = enrich_attributes_with_descriptions(raw_attributes, &narrative_config);

    // 6. 获取服务器地址
    let current_server_url = state.server_http_url.read().await.clone();
    let server_url = character.server_url.clone().or(Some(current_server_url));

    // 7. 构建响应
    let response = CharacterInfoResponse {
        agent_id,
        server_url,
        name: character.name.clone(),
        age: character.age,
        gender: character.gender.clone(),
        appearance: character.appearance.clone(),
        identity: character.identity.clone(),
        personality: character.personality.clone(),
        values: character.values.clone(),
        registered_at: character.registered_at.map(|t| t.to_rfc3339()),
        attributes,
        birth_attributes: character
            .birth_attributes
            .as_ref()
            .and_then(|a| serde_json::to_value(a).ok()),
        inventory,
        location,
        tick_id,
        world_time,
        status,
        is_stale,
        derived_attributes: enrich_derived_attributes(
            current
                .as_ref()
                .map(|ws| ws.self_state.derived_attributes.clone()),
            &narrative_config,
        ),
        skills: current
            .as_ref()
            .map(|ws| {
                ws.self_state
                    .skills
                    .iter()
                    .map(|s| SkillBrief {
                        skill_id: s.skill_id.clone(),
                        name: s.name.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
    };

    Json(response).into_response()
}

/// GET /api/v1/characters/:id
///
/// 获取指定角色的完整信息（用于抽屉展示）
pub(crate) async fn get_character_by_id_handler(
    State(state): State<HttpApiState>,
    AxumPath(agent_id): AxumPath<Uuid>,
) -> impl IntoResponse {
    // 1. 从文件系统读取角色配置
    let character_dir = state.character_dir.read().await.clone();
    let character = match get_character_by_id_sync(&character_dir, agent_id) {
        Ok(Some(ch)) => ch,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error_code: "character_not_found".to_string(),
                    message: "角色不存在".to_string(),
                }),
            )
                .into_response();
        }
        Err(e) => {
            error!("读取角色配置失败: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error_code: "character_read_error".to_string(),
                    message: format!("读取角色配置失败: {}", e),
                }),
            )
                .into_response();
        }
    };

    // 2. 如果是当前角色，返回完整 WorldState 数据
    let current_agent_id = *state.agent_id.read().await;
    let is_current = current_agent_id == agent_id;

    if is_current {
        // 复用当前角色的 WorldState 数据
        let narrative_config = state.narrative_config.read().await.clone();
        let current = state.current_state.read().await;
        let is_dead_flag = state.is_dead.load(std::sync::atomic::Ordering::Relaxed);
        let is_stale = current.is_none() || is_dead_flag;

        let (raw_attributes, inventory, location, tick_id, world_time) = match current.as_ref() {
            Some(ws) => {
                let attrs = serde_json::to_value(&ws.self_state.attributes).ok();
                let inv = serde_json::to_value(&ws.self_state.inventory).ok();
                let loc = Some(format!("{} ({})", ws.location.name, ws.location.node_type));
                let time = enrich_world_time_json(&ws.world_time);
                (attrs, inv, loc, Some(ws.tick_id), time)
            }
            None => (None, None, None, None, None),
        };

        let status = if state.is_dead.load(std::sync::atomic::Ordering::Relaxed) {
            Some("dead".to_string())
        } else {
            raw_attributes
                .as_ref()
                .and_then(|a| a.get("hp"))
                .and_then(|hp| hp.as_i64())
                .map(|hp| if hp > 0 { "alive" } else { "dead" }.to_string())
        };

        let attributes = enrich_attributes_with_descriptions(raw_attributes, &narrative_config);
        let current_server_url = state.server_http_url.read().await.clone();
        let server_url = character.server_url.clone().or(Some(current_server_url));

        return Json(CharacterInfoResponse {
            agent_id: character.agent_id.map(|id| id.to_string()),
            server_url,
            name: character.name.clone(),
            age: character.age,
            gender: character.gender.clone(),
            appearance: character.appearance.clone(),
            identity: character.identity.clone(),
            personality: character.personality.clone(),
            values: character.values.clone(),
            registered_at: character.registered_at.map(|t| t.to_rfc3339()),
            attributes,
            birth_attributes: character
                .birth_attributes
                .as_ref()
                .and_then(|a| serde_json::to_value(a).ok()),
            inventory,
            location,
            tick_id,
            world_time,
            status,
            is_stale,
            derived_attributes: enrich_derived_attributes(
                current
                    .as_ref()
                    .map(|ws| ws.self_state.derived_attributes.clone()),
                &narrative_config,
            ),
            skills: current
                .as_ref()
                .map(|ws| {
                    ws.self_state
                        .skills
                        .iter()
                        .map(|s| SkillBrief {
                            skill_id: s.skill_id.clone(),
                            name: s.name.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
        .into_response();
    }

    // 3. 非当前角色，返回配置文件数据（不包含实时状态）
    let current_server_url = state.server_http_url.read().await.clone();
    let server_url = character.server_url.clone().or(Some(current_server_url));

    // 非当前角色也做属性丰富化，以便前端正确渲染
    let raw_attrs = character
        .birth_attributes
        .as_ref()
        .and_then(|a| serde_json::to_value(a).ok());
    let narrative_config = state.narrative_config.read().await.clone();
    let attributes = enrich_attributes_with_descriptions(raw_attrs.clone(), &narrative_config);

    let response = CharacterInfoResponse {
        agent_id: character.agent_id.map(|id| id.to_string()),
        server_url,
        name: character.name.clone(),
        age: character.age,
        gender: character.gender.clone(),
        appearance: character.appearance.clone(),
        identity: character.identity.clone(),
        personality: character.personality.clone(),
        values: character.values.clone(),
        registered_at: character.registered_at.map(|t| t.to_rfc3339()),
        attributes,
        birth_attributes: raw_attrs,
        inventory: None,
        location: None,
        tick_id: None,
        world_time: None,
        status: Some(match character.status {
            CharacterStatus::Alive => "alive".to_string(),
            CharacterStatus::Dead => "dead".to_string(),
            CharacterStatus::Retired => "retired".to_string(),
        }),
        is_stale: true,
        derived_attributes: None,
        skills: vec![],
    };

    Json(response).into_response()
}

/// 属性元数据响应
#[derive(Debug, Serialize)]
pub struct AttributeMetaResponse {
    /// 属性分类
    pub categories: HashMap<String, Vec<String>>,
    /// 属性显示名称映射
    pub display_names: HashMap<String, String>,
}

pub(crate) async fn get_attribute_meta_handler(
    State(state): State<HttpApiState>,
) -> impl IntoResponse {
    // 1. 优先从内存读取
    let narrative = {
        let guard = state.narrative_config.read().await;
        guard.clone()
    };

    // 2. 内存为空时，尝试从磁盘加载
    let narrative = if narrative.is_none() {
        if let Some(home) = dirs::home_dir() {
            let path = home
                .join(".cyber-jianghu")
                .join("config")
                .join("narrative_config.json");
            if path.exists() {
                match tokio::fs::read_to_string(&path).await {
                    Ok(content) => {
                        match serde_json::from_str::<cyber_jianghu_protocol::NarrativeConfig>(
                            &content,
                        ) {
                            Ok(cfg) => {
                                info!("从磁盘加载 narrative_config: {:?}", path);
                                // 回填内存，供后续请求使用
                                *state.narrative_config.write().await = Some(cfg.clone());
                                Some(cfg)
                            }
                            Err(e) => {
                                warn!("解析磁盘 narrative_config 失败: {}", e);
                                None
                            }
                        }
                    }
                    Err(e) => {
                        warn!("读取磁盘 narrative_config 失败: {}", e);
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        }
    } else {
        narrative
    };

    let categories = narrative
        .as_ref()
        .map(|c| c.attribute_categories.clone())
        .unwrap_or_default();

    let mut display_names = HashMap::new();
    if let Some(n) = narrative.as_ref() {
        for (key, attr) in &n.attributes {
            display_names.insert(key.clone(), attr.display_name.clone());
        }
    }

    Json(AttributeMetaResponse {
        categories,
        display_names,
    })
    .into_response()
}

/// 丰富属性数据，添加叙事描述
/// 为 WorldTime JSON 添加 `display` 字段（中文格式）
fn enrich_world_time_json(
    world_time: &cyber_jianghu_protocol::WorldTime,
) -> Option<serde_json::Value> {
    let mut val = serde_json::to_value(world_time).ok()?;
    if let Some(obj) = val.as_object_mut() {
        obj.insert(
            "display".to_string(),
            serde_json::Value::String(world_time.to_chinese()),
        );
    }
    Some(val)
}

///
/// 从服务器返回的原始属性中：
/// - 提取 `{key}_max` 字段作为属性最大值（服务器通过 max_value_formula 计算）
/// - 如果没有 `{key}_max` 字段，说明该属性没有上限（如声望、派生属性）
fn enrich_attributes_with_descriptions(
    raw_attributes: Option<serde_json::Value>,
    narrative_config: &Option<cyber_jianghu_protocol::NarrativeConfig>,
) -> Option<serde_json::Value> {
    let attrs = raw_attributes?;
    let attrs_obj = attrs.as_object()?;

    // 预先收集所有 _max 字段
    let max_values: std::collections::HashMap<&str, i64> = attrs_obj
        .iter()
        .filter_map(|(key, value)| {
            key.strip_suffix("_max")
                .and_then(|base| value.as_i64().map(|v| (base, v)))
        })
        .collect();

    // 将属性转换为带描述的格式（排除 _max 冗余字段）
    let enriched: serde_json::Map<String, serde_json::Value> = attrs_obj
        .iter()
        .filter(|(key, _)| !key.ends_with("_max")) // 排除 _max 字段
        .filter_map(|(key, value)| {
            // 获取当前值
            let current = match value.as_i64() {
                Some(v) => v,
                None => return None,
            };

            // 从叙事配置获取属性信息
            let (display_name, description) = narrative_config
                .as_ref()
                .and_then(|cfg| cfg.attributes.get(key))
                .map(|attr_cfg| {
                    let name = attr_cfg.display_name.clone();
                    let current_i32 = current as i32;
                    let desc = attr_cfg
                        .thresholds
                        .iter()
                        .rev()
                        .find(|t| current_i32 >= t.min && current_i32 <= t.max)
                        .map(|t| t.description.clone())
                        .unwrap_or_else(|| format!("{}: {}", name, current));
                    (name, desc)
                })
                .unwrap_or_else(|| (key.clone(), format!("{}: {}", key, current)));

            // 从服务器返回的 {key}_max 字段获取最大值
            // 如果没有 _max 字段，说明该属性没有上限（如声望、派生属性）
            let max = max_values.get(key.as_str()).copied();

            // 构建属性对象
            let attr_obj = if let Some(max_val) = max {
                serde_json::json!({
                    "name": display_name,
                    "current": current,
                    "max": max_val,
                    "description": description
                })
            } else {
                // 没有上限的属性，不设置 max 字段
                serde_json::json!({
                    "name": display_name,
                    "current": current,
                    "description": description
                })
            };

            Some((key.clone(), attr_obj))
        })
        .collect();

    Some(serde_json::Value::Object(enriched))
}

fn enrich_derived_attributes(
    derived: Option<std::collections::HashMap<String, f32>>,
    narrative_config: &Option<cyber_jianghu_protocol::NarrativeConfig>,
) -> Option<serde_json::Value> {
    let derived = derived?;
    let enriched: serde_json::Map<String, serde_json::Value> = derived
        .into_iter()
        .map(|(key, value)| {
            let (display_name, description) = narrative_config
                .as_ref()
                .and_then(|cfg| cfg.attributes.get(&key))
                .map(|attr_cfg| {
                    let name = attr_cfg.display_name.clone();
                    let desc = attr_cfg
                        .thresholds
                        .iter()
                        .rev()
                        .find(|t| (value as i32) >= t.min && (value as i32) <= t.max)
                        .map(|t| t.description.clone())
                        .unwrap_or_else(|| format!("{}: {:.3}", name, value));
                    (name, desc)
                })
                .unwrap_or_else(|| (key.clone(), format!("{}: {:.3}", key, value)));

            let attr_obj = serde_json::json!({
                "name": display_name,
                "current": value,
                "description": description,
            });
            (key, attr_obj)
        })
        .collect();

    if enriched.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(enriched))
    }
}

// ============================================================================
