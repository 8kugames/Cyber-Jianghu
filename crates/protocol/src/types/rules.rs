//! 游戏规则相关类型
//!
//! 包含游戏规则和世界观规则

use serde::{Deserialize, Serialize};

use super::entities::{AvailableAction, InitialItem};

/// 游戏规则
///
/// 服务端下发的游戏规则配置，包含可用动作和初始物品
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameRules {
    /// Tick 周期（秒）
    pub tick_duration_secs: u64,

    /// 可用动作列表
    pub available_actions: Vec<AvailableAction>,

    /// 初始物品（注册时发放）
    pub initial_items: Vec<InitialItem>,

    /// 规则版本（用于检测变更）
    pub version: String,

    /// 最后更新时间
    pub last_updated: String,
}

// ============================================================================
// 世界观规则
// ============================================================================

/// 时代设定
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EraSettings {
    /// 时代名称
    pub name: String,

    /// 技术水平上限
    pub tech_level: String,

    /// 社会形态
    pub social_structure: String,
}

/// 世界观规则（服务端下发 + SDK 内置基础）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldBuildingRules {
    /// 规则版本
    pub version: String,

    /// 时代设定
    pub era: EraSettings,

    /// 允许的概念（内力、轻功等）
    pub allowed_concepts: Vec<String>,

    /// 禁止的概念（魔法、现代科技等）
    pub forbidden_concepts: Vec<String>,

    /// 叙事规则（自然语言，供 LLM 理解）
    pub narrative_rules: String,

    /// 最后更新时间
    pub last_updated: String,
}

impl Default for WorldBuildingRules {
    fn default() -> Self {
        Self {
            version: "0.0.1".to_string(),
            era: EraSettings {
                name: "北宋前期（约10世纪中国）".to_string(),
                tech_level: "冷兵器时代，火药仅用于烟火".to_string(),
                social_structure: "封建帝制，江湖与庙堂并存".to_string(),
            },
            allowed_concepts: vec![
                "内力".into(),
                "轻功".into(),
                "武功".into(),
                "点穴".into(),
                "暗器".into(),
                "毒术".into(),
                "医术".into(),
                "易容".into(),
                "阵法".into(),
                "奇门遁甲".into(),
                "相术".into(),
            ],
            forbidden_concepts: vec![
                "魔法".into(),
                "仙术".into(),
                "法术".into(),
                "热武器".into(),
                "现代科技".into(),
                "超能力".into(),
                "异能".into(),
                "穿越".into(),
                "系统".into(),
            ],
            narrative_rules: include_str!("../default_world_rules.md").to_string(),
            last_updated: chrono::Utc::now().to_rfc3339(),
        }
    }
}

impl WorldBuildingRules {
    /// 从 JSON 文件加载规则
    pub fn from_json_file<P: AsRef<std::path::Path>>(path: P) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path.as_ref())
            .map_err(|e| anyhow::anyhow!("Failed to read world rules file: {}", e))?;
        let rules: Self = serde_json::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Failed to parse world rules JSON: {}", e))?;
        Ok(rules)
    }
}
