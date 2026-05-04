// ============================================================================
// WebSocket 类型定义
// ============================================================================
//
// 本模块定义 WebSocket 相关的类型和辅助函数
// ============================================================================

use serde::Deserialize;
use uuid::Uuid;

use crate::game_data::types::UnifiedWorldBuildingRulesConfig;
use crate::game_data::{ActionRegistry, InitialInventoryRegistry};
use chrono::Utc;
use cyber_jianghu_protocol::{GameRules, InitialItem, WorldBuildingRules};

/// WebSocket 升级请求的查询参数
#[derive(Debug, Deserialize)]
pub struct WebSocketQuery {
    /// 设备 ID（客户端生成的 UUID）
    pub device_id: Uuid,
    /// 认证 token（服务器生成的 auth_token）
    pub token: String,
    /// Agent ID（可选，角色创建后由服务器分配）
    #[serde(default)]
    pub agent_id: Option<Uuid>,
}

// ============================================================================
// 辅助函数
// ============================================================================

/// 生存配置参数（从 game_rules.yaml 读取）
///
/// 天道无为：干预阈值已移除，仅保留重生配置
pub struct SurvivalConfig {
    pub rebirth_delay_ticks: i32,
    pub rebirth_retry_max_attempts: u32,
    pub rebirth_retry_interval_secs: u64,
}

/// 构建游戏规则（从配置注册表）
///
/// 从 GameData 缓存中读取配置，包括 immediate_events 和 intent_batch。
/// llm_validation 的 always/adaptive/skip 分级从 actions.yaml 的 ooc_risk 动态生成。
pub fn build_game_rules_from_config(
    tick_duration_secs: u64,
    survival: SurvivalConfig,
    version: String,
    immediate_events: Option<cyber_jianghu_protocol::ImmediateEventConfig>,
    intent_batch: Option<cyber_jianghu_protocol::IntentBatchConfig>,
) -> GameRules {
    let available_actions = ActionRegistry::build_available_actions();

    // 从 ooc_risk 动态构建分级审核列表
    let mut always_types: Vec<String> = Vec::new();
    let mut adaptive_types: Vec<String> = Vec::new();
    let mut skip_types: Vec<String> = Vec::new();

    for action in &available_actions {
        match action.ooc_risk.as_str() {
            "high" => always_types.push(action.action.clone()),
            "medium" => adaptive_types.push(action.action.clone()),
            _ => skip_types.push(action.action.clone()),
        }
    }

    tracing::info!(
        "[OOC分级] always={:?}, adaptive={:?}, skip={:?}",
        always_types,
        adaptive_types,
        skip_types
    );

    // 构建 intent_batch：ooc_risk 覆盖 types 列表，保留 yaml 中的其他配置
    let mut llm_validation = intent_batch
        .as_ref()
        .map(|ib| ib.llm_validation.clone())
        .unwrap_or_else(|| cyber_jianghu_protocol::GradedValidationConfig {
            always_types: Vec::new(),
            adaptive_types: Vec::new(),
            skip_types: Vec::new(),
            minimum_per_tick: 1,
            restricted_area_keywords: Vec::new(),
            high_value_item_keywords: Vec::new(),
            adaptive_field_mapping: std::collections::HashMap::new(),
        });

    llm_validation.always_types = always_types;
    llm_validation.adaptive_types = adaptive_types;
    llm_validation.skip_types = skip_types;

    let intent_batch = cyber_jianghu_protocol::IntentBatchConfig {
        max_intents_per_tick: intent_batch
            .as_ref()
            .map(|ib| ib.max_intents_per_tick)
            .unwrap_or(5),
        max_retries: intent_batch.as_ref().map(|ib| ib.max_retries).unwrap_or(3),
        pipeline_execution_enabled: intent_batch
            .as_ref()
            .map(|ib| ib.pipeline_execution_enabled)
            .unwrap_or(true),
        partial_execution_enabled: intent_batch
            .as_ref()
            .map(|ib| ib.partial_execution_enabled)
            .unwrap_or(true),
        llm_validation,
        llm_chaos_threshold: intent_batch
            .as_ref()
            .map(|ib| ib.llm_chaos_threshold)
            .unwrap_or(12),
    };

    let initial_items = InitialInventoryRegistry::items()
        .into_iter()
        .map(|item| InitialItem {
            item_id: item.item_id,
            name: item.name,
            quantity: item.quantity,
            description: item.description,
        })
        .collect();

    // 从动作配置中提取 survival 标签的动作名称
    let survival_actions = ActionRegistry::action_names_with_tag("survival");

    GameRules {
        tick_duration_secs,
        available_actions,
        initial_items,
        survival_actions,
        version,
        last_updated: Utc::now().to_rfc3339(),
        intent_batch: Some(intent_batch),
        immediate_events,
        rebirth_delay_ticks: survival.rebirth_delay_ticks,
        rebirth_retry_max_attempts: survival.rebirth_retry_max_attempts,
        rebirth_retry_interval_secs: survival.rebirth_retry_interval_secs,
        lifespan: None, // ConfigUpdate 路径不含 lifespan，由注册时下发
        calendar: crate::game_data::registry::TimeRegistry::get_config().map(|tc| {
            cyber_jianghu_protocol::CalendarConfig {
                days_per_season: tc.days_per_season as u32,
                seasons_per_year: tc.seasons_per_year as u32,
            }
        }),
        daily_summary: None,
    }
}

/// 加载世界观规则（从配置文件）
///
/// 优先加载 YAML 格式，回退到 JSON 格式。
/// 使用统一配置格式（带 version/description/meta/data 包装层）
pub fn load_world_building_rules() -> Option<WorldBuildingRules> {
    use crate::game_data::loaders::config_format::ConfigFormat;

    let config_dir = crate::paths::get_config_dir();

    // 优先尝试 YAML 格式
    let yaml_path = config_dir.join("world_building_rules.yaml");
    let json_path = config_dir.join("world_building_rules.json");

    let (content, format) = if yaml_path.exists() {
        match std::fs::read_to_string(&yaml_path) {
            Ok(c) => (c, ConfigFormat::Yaml),
            Err(e) => {
                tracing::warn!("Failed to read world_building_rules.yaml: {}", e);
                return None;
            }
        }
    } else if json_path.exists() {
        match std::fs::read_to_string(&json_path) {
            Ok(c) => (c, ConfigFormat::Json),
            Err(e) => {
                tracing::warn!("Failed to read world_building_rules.json: {}", e);
                return None;
            }
        }
    } else {
        tracing::warn!("No world_building_rules config file found");
        return None;
    };

    // 解析为统一配置格式
    let unified_config: UnifiedWorldBuildingRulesConfig =
        match crate::game_data::loaders::config_format::parse_config(&content, format) {
            Ok(config) => config,
            Err(e) => {
                tracing::warn!(
                    "Failed to parse world_building_rules: {}. Using default rules.",
                    e
                );
                return None;
            }
        };

    let data = unified_config.data;
    let rules = WorldBuildingRules {
        version: unified_config.version,
        era: cyber_jianghu_protocol::EraSettings {
            name: data.era.name,
            tech_level: data.era.tech_level,
            social_structure: data.era.social_structure,
        },
        allowed_concepts: data.allowed_concepts,
        forbidden_concepts: data.forbidden_concepts,
        narrative_rules: data.narrative_rules,
        last_updated: data.last_updated.unwrap_or_else(|| Utc::now().to_rfc3339()),
    };

    tracing::info!(
        "📜 Loaded world building rules: version {} ({:?})",
        rules.version,
        format
    );
    Some(rules)
}
