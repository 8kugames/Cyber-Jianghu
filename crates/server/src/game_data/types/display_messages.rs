// ============================================================================
// 显示消息配置类型
// ============================================================================
//
// 数据驱动的显示消息配置，将硬编码字符串移至配置文件

use serde::{Deserialize, Serialize};

/// 显示消息配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DisplayMessagesConfig {
    /// 版本号
    pub version: String,

    /// 描述
    #[serde(default)]
    pub description: String,

    /// 实体状态描述
    pub entity_states: EntityStatesConfig,

    /// 天气描述
    pub weather: WeatherConfig,

    /// 天气环境事件描述（key = 天气类型, value = 事件描述文本）
    #[serde(default)]
    pub weather_events: std::collections::HashMap<String, String>,

    /// 系统通知
    pub notifications: NotificationsConfig,
}

/// 实体状态描述配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EntityStatesConfig {
    /// 存活状态描述
    pub alive: String,

    /// 死亡状态描述
    pub dead: String,
}

/// 天气描述配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WeatherConfig {
    /// 晴天
    pub sunny: String,

    /// 多云
    #[serde(default = "default_cloudy")]
    pub cloudy: String,

    /// 雨天
    #[serde(default = "default_rainy")]
    pub rainy: String,

    /// 暴风雨
    #[serde(default = "default_stormy")]
    pub stormy: String,
}

fn default_cloudy() -> String {
    "多云".to_string()
}
fn default_rainy() -> String {
    "雨".to_string()
}
fn default_stormy() -> String {
    "暴风雨".to_string()
}

/// 系统通知配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NotificationsConfig {
    /// 死亡通知
    pub death: String,

    /// 重生通知
    #[serde(default = "default_rebirth")]
    pub rebirth: String,
}

fn default_rebirth() -> String {
    "大侠已转世重生。".to_string()
}


