// ============================================================================
// 生存 Reward 配置类型（数据驱动）
// ============================================================================
//
// 对应 config/reward.yaml。所有 reward 数值由此类型承载，零硬编码。
// 哲学锚点：天道无为；reward 纯锚定生存因果（寿数 + 死亡 penalty）。
// ============================================================================

use serde::{Deserialize, Serialize};

/// Reward 配置根结构（对应 reward.yaml 全文）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewardConfig {
    pub version: String,
    #[serde(default)]
    pub description: String,
    pub daily: DailyRewardConfig,
    pub lifetime: LifetimeRewardConfig,
    #[serde(default)]
    pub output: OutputConfig,
}

/// 每日 reward 分量配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyRewardConfig {
    /// 生存分量：该日存活即得
    pub survival_score: f64,
    pub physiological: PhysiologicalConfig,
    #[serde(default)]
    pub tianhun: TianhunConfig,
}
/// 生理分量配置：satiation/hydration 归一化权重
///
/// max_value 运行时从 StateRegistry 读 attributes 配置求值，不在此硬编码
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhysiologicalConfig {
    #[serde(default = "default_satiation_weight")]
    pub satiation_weight: f64,
    #[serde(default = "default_hydration_weight")]
    pub hydration_weight: f64,
}

fn default_satiation_weight() -> f64 {
    0.25
}
fn default_hydration_weight() -> f64 {
    0.25
}

/// 天魂审查分量配置（P1 阶段 server 读不到 agent 端 soul_cycle.db，judgment 暂为 None）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TianhunConfig {
    #[serde(default = "default_approved_score")]
    pub approved_score: f64,
    #[serde(default = "default_rejected_score")]
    pub rejected_score: f64,
}

impl Default for TianhunConfig {
    fn default() -> Self {
        Self {
            approved_score: default_approved_score(),
            rejected_score: default_rejected_score(),
        }
    }
}

fn default_approved_score() -> f64 {
    0.5
}
fn default_rejected_score() -> f64 {
    -0.5
}

/// 一生 reward 配置（死亡结算）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LifetimeRewardConfig {
    /// 统一死亡 penalty（不分死因）
    pub death_penalty: f64,
}

/// 输出开关配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_base_dir")]
    pub base_dir: String,
    #[serde(default = "default_flush_on_death")]
    pub flush_on_death: bool,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            base_dir: default_base_dir(),
            flush_on_death: default_flush_on_death(),
        }
    }
}

fn default_enabled() -> bool {
    true
}
fn default_base_dir() -> String {
    "rewards".to_string()
}
fn default_flush_on_death() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reward_config_deserialize() {
        let yaml = r#"
version: "0.0.1"
description: "test"
daily:
  survival_score: 1.0
  physiological:
    satiation_weight: 0.25
    hydration_weight: 0.25
  tianhun:
    approved_score: 0.5
    rejected_score: -0.5
lifetime:
  death_penalty: -50.0
output:
  enabled: true
  base_dir: "rewards"
  flush_on_death: true
"#;
        let cfg: RewardConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.daily.survival_score, 1.0);
        assert_eq!(cfg.daily.physiological.satiation_weight, 0.25);
        assert_eq!(cfg.lifetime.death_penalty, -50.0);
        assert!(cfg.output.enabled);
    }

    #[test]
    fn test_reward_config_defaults() {
        // 最小配置：仅必填项
        let yaml = r#"
version: "0.0.1"
daily:
  survival_score: 1.0
  physiological:
    satiation_weight: 0.25
    hydration_weight: 0.25
lifetime:
  death_penalty: -50.0
"#;
        let cfg: RewardConfig = serde_yaml::from_str(yaml).unwrap();
        // 可选项走 default
        assert_eq!(cfg.daily.tianhun.approved_score, 0.5);
        assert_eq!(cfg.daily.tianhun.rejected_score, -0.5);
        assert!(cfg.output.enabled);
    }
}
