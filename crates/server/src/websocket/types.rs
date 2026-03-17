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

use crate::game_data::{ActionRegistry, InitialInventoryRegistry};
use crate::models::Intent;
use chrono::Utc;
use cyber_jianghu_protocol::{AvailableAction, GameRules, InitialItem, WorldBuildingRules};

/// WebSocket 升级请求的查询参数
#[derive(Debug, Deserialize)]
pub struct WebSocketQuery {
    /// 认证 token
    pub token: String,
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
                .and_then(|config| Some(config.description))
                .unwrap_or_else(|| "".to_string());

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
/// 路径由 paths 模块统一管理
pub fn load_world_building_rules() -> Option<WorldBuildingRules> {
    let config_path = crate::paths::get_config_dir().join("world-building-rules.json");

    match std::fs::read_to_string(config_path) {
        Ok(content) => match serde_json::from_str::<WorldBuildingRules>(&content) {
            Ok(rules) => {
                tracing::info!("📜 Loaded world building rules: version {}", rules.version);
                Some(rules)
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to parse world-building-rules.json: {}. Using default rules.",
                    e
                );
                None
            }
        },
        Err(e) => {
            tracing::warn!(
                "Failed to read world-building-rules.json: {}. Using default rules.",
                e
            );
            None
        }
    }
}
