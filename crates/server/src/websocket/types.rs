// ============================================================================
// WebSocket 类型定义
// ============================================================================
//
// 本模块定义 WebSocket 相关的类型和辅助函数
// ============================================================================

use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::game_data::types::UnifiedWorldBuildingRulesConfig;
use crate::game_data::{ActionRegistry, InitialInventoryRegistry};
use crate::models::Intent;
use chrono::Utc;
use cyber_jianghu_protocol::{AvailableAction, GameRules, InitialItem, WorldBuildingRules};

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
// Intent 管理器类型
// ============================================================================

/// Intent 管理器
///
/// 管理临时的 Intent 缓存
/// - Agent 通过 WebSocket 发送 Intent → 存储到这里
/// - Tick Engine 在每个 Tick 开始时从这里读取所有 Intent
/// - Tick 结束后清空缓存
pub type IntentManager = Arc<RwLock<HashMap<Uuid, Intent>>>;

// ============================================================================
// 辅助函数
// ============================================================================

/// 构建游戏规则（从配置注册表）
pub fn build_game_rules_from_config(tick_duration_secs: u64, version: String) -> GameRules {
    let available_actions = ActionRegistry::all_action_names()
        .into_iter()
        .map(|action_name| {
            // 从配置获取描述
            let description = ActionRegistry::get(&action_name)
                .map(|config| config.description)
                .unwrap_or_default();

            AvailableAction {
                action: action_name,
                description,
                valid_targets: None, // MVP 阶段不提供
            }
        })
        .collect();

    let initial_items = InitialInventoryRegistry::items()
        .into_iter()
        .map(|item| InitialItem {
            item_id: item.item_id,
            name: item.name,
            quantity: item.quantity,
            description: item.description,
        })
        .collect();

    GameRules {
        tick_duration_secs,
        available_actions,
        initial_items,
        version,
        last_updated: Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
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
    let yaml_path = config_dir.join("world-building-rules.yaml");
    let json_path = config_dir.join("world-building-rules.json");

    let (content, format) = if yaml_path.exists() {
        match std::fs::read_to_string(&yaml_path) {
            Ok(c) => (c, ConfigFormat::Yaml),
            Err(e) => {
                tracing::warn!("Failed to read world-building-rules.yaml: {}", e);
                return None;
            }
        }
    } else if json_path.exists() {
        match std::fs::read_to_string(&json_path) {
            Ok(c) => (c, ConfigFormat::Json),
            Err(e) => {
                tracing::warn!("Failed to read world-building-rules.json: {}", e);
                return None;
            }
        }
    } else {
        tracing::warn!("No world-building-rules config file found");
        return None;
    };

    // 解析为统一配置格式
    let unified_config: UnifiedWorldBuildingRulesConfig =
        match crate::game_data::loaders::config_format::parse_config(&content, format) {
            Ok(config) => config,
            Err(e) => {
                tracing::warn!(
                    "Failed to parse world-building-rules: {}. Using default rules.",
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
