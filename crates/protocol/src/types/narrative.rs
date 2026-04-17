// ============================================================================
// 叙事化配置类型 - 用于 Server 下发给 Agent
// ============================================================================
//
// 将数值状态转换为叙事化描述的配置，由 Server 统一管理并下发给 Agent。

use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;

/// 单个阈值配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NarrativeThreshold {
    /// 最小值（包含）
    pub min: i32,
    /// 最大值（包含）
    pub max: i32,
    /// 叙事描述
    pub description: String,
}

/// 单个属性的叙事配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NarrativeAttributeConfig {
    /// 属性名称
    pub name: String,
    /// 显示名称
    pub display_name: String,
    /// 阈值列表（按优先级排序，从高到低）
    pub thresholds: Vec<NarrativeThreshold>,
    /// 备注（可选）
    #[serde(default)]
    pub note: Option<String>,
}

/// 状态效果配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StatusEffectConfig {
    /// 效果描述
    pub description: String,
}

/// 完整的叙事配置（协议层）
///
/// 由 Server 加载并通过注册接口下发给 Agent
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NarrativeConfig {
    /// 版本号
    pub version: String,
    /// 描述
    #[serde(default)]
    pub description: String,
    /// 属性分类（primary/status/derived）
    #[serde(default)]
    pub attribute_categories: HashMap<String, Vec<String>>,
    /// 属性配置映射
    pub attributes: HashMap<String, NarrativeAttributeConfig>,
    /// 状态效果配置
    #[serde(default)]
    pub status_effects: HashMap<String, StatusEffectConfig>,
}

impl Default for NarrativeConfig {
    fn default() -> Self {
        Self::builtin()
    }
}

impl NarrativeConfig {
    /// 创建内置的默认配置（当无法加载外部配置时使用）
    pub fn builtin() -> Self {
        let mut attributes = HashMap::new();

        // HP 叙事配置
        attributes.insert(
            "hp".to_string(),
            NarrativeAttributeConfig {
                name: "hp".to_string(),
                display_name: "生命值".to_string(),
                thresholds: vec![
                    NarrativeThreshold {
                        min: 90,
                        max: 100,
                        description: "身体状况极佳，精力充沛".to_string(),
                    },
                    NarrativeThreshold {
                        min: 70,
                        max: 89,
                        description: "身体状态良好，虽有轻微疲惫".to_string(),
                    },
                    NarrativeThreshold {
                        min: 50,
                        max: 69,
                        description: "身体状况一般，能感受到明显疲劳".to_string(),
                    },
                    NarrativeThreshold {
                        min: 30,
                        max: 49,
                        description: "身体虚弱，伤痛明显".to_string(),
                    },
                    NarrativeThreshold {
                        min: 10,
                        max: 29,
                        description: "身受重伤，意识模糊".to_string(),
                    },
                    NarrativeThreshold {
                        min: 0,
                        max: 9,
                        description: "生命垂危".to_string(),
                    },
                ],
                note: None,
            },
        );

        // 饥饿叙事配置
        attributes.insert(
            "hunger".to_string(),
            NarrativeAttributeConfig {
                name: "hunger".to_string(),
                display_name: "饥饿".to_string(),
                thresholds: vec![
                    NarrativeThreshold {
                        min: 80,
                        max: 100,
                        description: "肚子很饱，完全没有饥饿感".to_string(),
                    },
                    NarrativeThreshold {
                        min: 60,
                        max: 79,
                        description: "肚子还算饱，暂时不需要进食".to_string(),
                    },
                    NarrativeThreshold {
                        min: 40,
                        max: 59,
                        description: "肚子有些饿了，该考虑找东西吃".to_string(),
                    },
                    NarrativeThreshold {
                        min: 20,
                        max: 39,
                        description: "肚子很饿，饥肠辘辘".to_string(),
                    },
                    NarrativeThreshold {
                        min: 0,
                        max: 19,
                        description: "饥饿难耐，已饿得头昏眼花".to_string(),
                    },
                ],
                note: Some("值越高表示越饱".to_string()),
            },
        );

        // 口渴叙事配置
        attributes.insert(
            "thirst".to_string(),
            NarrativeAttributeConfig {
                name: "thirst".to_string(),
                display_name: "口渴".to_string(),
                thresholds: vec![
                    NarrativeThreshold {
                        min: 80,
                        max: 100,
                        description: "完全不渴".to_string(),
                    },
                    NarrativeThreshold {
                        min: 60,
                        max: 79,
                        description: "略有口渴".to_string(),
                    },
                    NarrativeThreshold {
                        min: 40,
                        max: 59,
                        description: "口渴明显".to_string(),
                    },
                    NarrativeThreshold {
                        min: 20,
                        max: 39,
                        description: "非常口渴".to_string(),
                    },
                    NarrativeThreshold {
                        min: 0,
                        max: 19,
                        description: "渴得难以忍受".to_string(),
                    },
                ],
                note: Some("值越高表示越不渴".to_string()),
            },
        );

        // 体力叙事配置
        attributes.insert(
            "stamina".to_string(),
            NarrativeAttributeConfig {
                name: "stamina".to_string(),
                display_name: "体力".to_string(),
                thresholds: vec![
                    NarrativeThreshold {
                        min: 80,
                        max: 100,
                        description: "体力充沛，精力旺盛".to_string(),
                    },
                    NarrativeThreshold {
                        min: 60,
                        max: 79,
                        description: "体力尚可，虽有些疲惫".to_string(),
                    },
                    NarrativeThreshold {
                        min: 40,
                        max: 59,
                        description: "体力有些不支".to_string(),
                    },
                    NarrativeThreshold {
                        min: 20,
                        max: 39,
                        description: "精疲力竭".to_string(),
                    },
                    NarrativeThreshold {
                        min: 0,
                        max: 19,
                        description: "体力耗尽".to_string(),
                    },
                ],
                note: None,
            },
        );

        NarrativeConfig {
            version: "0.0.1-builtin".to_string(),
            description: "内置默认叙事配置".to_string(),
            attribute_categories: HashMap::new(),
            attributes,
            status_effects: HashMap::new(),
        }
    }

    /// 根据属性值获取叙事描述
    pub fn get_description(&self, attr_name: &str, value: i32) -> Option<&str> {
        self.attributes.get(attr_name).and_then(|config| {
            config
                .thresholds
                .iter()
                .find(|t| value >= t.min && value <= t.max)
                .map(|t| t.description.as_str())
        })
    }

    /// 获取属性的显示名称
    pub fn get_display_name(&self, attr_name: &str) -> Option<&str> {
        self.attributes
            .get(attr_name)
            .map(|c| c.display_name.as_str())
    }
}

// ============================================================================
// 叙事化感知上下文 - 人魂数值隔离
// ============================================================================
//
// 人魂直连 WorldState 后，叙事感知层已旁路。此类型仅用于兼容旧路径。

/// 自身感知
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfPerception {
    /// 状态概述（如"你感到饥饿"）
    pub status_summary: String,
    /// 显著特征（如"你感到虚弱无力"）
    #[serde(default)]
    pub notable_attributes: Vec<String>,
    /// 背包叙事（如"你行囊里有三个馒头"）
    pub inventory_narrative: String,
}

/// 环境感知
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentPerception {
    /// 位置描述（如"你现在身处龙门客栈的大堂"）
    pub location_description: String,
    /// 环境氛围（如"炉灶上冒着热气，空气中飘着饭菜香"）
    pub ambient_features: String,
    /// 可互动物品
    #[serde(default)]
    pub interactive_elements: Vec<String>,
    /// 可达位置（含距离描述）
    #[serde(default)]
    pub reachable_locations: Vec<String>,
}

/// 其他 Agent 识别信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRecognition {
    /// 是否已知
    pub is_known: bool,
    /// 已知名称
    #[serde(skip_serializing_if = "Option::is_none")]
    pub known_name: Option<String>,
    /// 关系描述
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relationship: Option<String>,
}

/// 容错反序列化：LLM 可能返回 string/bool/null 而非 AgentRecognition struct
fn deserialize_recognition<'de, D>(deserializer: D) -> Result<Option<AgentRecognition>, D::Error>
where
    D: Deserializer<'de>,
{
    let value: serde_json::Value = match Option::deserialize(deserializer) {
        Ok(Some(v)) => v,
        Ok(None) => return Ok(None),
        Err(_) => return Ok(None),
    };

    match value {
        serde_json::Value::Object(_) => {
            match serde_json::from_value::<AgentRecognition>(value) {
                Ok(r) => Ok(Some(r)),
                Err(_) => Ok(None), // 缺必填字段的对象 → 降级为未知
            }
        }
        serde_json::Value::String(name) if !name.trim().is_empty() => {
            // LLM 返回 "recognition": "红娘子" → 视为已知角色
            Ok(Some(AgentRecognition {
                is_known: true,
                known_name: Some(name),
                relationship: None,
            }))
        }
        serde_json::Value::String(_) => Ok(None), // 空字符串 → 未知
        serde_json::Value::Bool(b) => Ok(Some(AgentRecognition {
            is_known: b,
            known_name: None,
            relationship: None,
        })),
        _ => Ok(None),
    }
}

/// 其他 Agent 感知
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPerception {
    /// 相对位置（如"身旁站着"）
    pub relative_position: String,
    /// 外貌描述
    pub appearance: String,
    /// 当前活动
    pub current_activity: String,
    /// 识别信息（已知角色）
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_recognition"
    )]
    pub recognition: Option<AgentRecognition>,
}

/// 上次行动结果
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActionOutcome {
    /// 结果叙事
    #[serde(default)]
    pub result_narrative: String,
    /// 是否成功
    #[serde(default)]
    pub success: bool,
    /// 副作用
    #[serde(default)]
    pub side_effects: Vec<String>,
    /// 意外事件
    #[serde(default)]
    pub unexpected_events: Vec<String>,
}

/// 叙事化感知上下文（旧路径，人魂直连 WorldState 后已旁路）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NarrativeContext {
    /// Tick ID
    pub tick_id: i64,
    /// 自身感知
    pub self_perception: SelfPerception,
    /// 环境感知
    pub environment: EnvironmentPerception,
    /// 附近 Agent 感知
    #[serde(default)]
    pub nearby_agents: Vec<AgentPerception>,
    /// 近期记忆片段
    #[serde(default)]
    pub recent_memories: Vec<String>,
    /// 上次行动结果
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_outcome: Option<ActionOutcome>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_narrative_config_builtin() {
        let config = NarrativeConfig::builtin();
        assert!(!config.attributes.is_empty());
        assert!(config.attributes.contains_key("hp"));
        assert!(config.attributes.contains_key("hunger"));
    }

    #[test]
    fn test_get_description() {
        let config = NarrativeConfig::builtin();
        let desc = config.get_description("hp", 95);
        assert!(desc.is_some());
        assert!(desc.unwrap().contains("极佳"));

        let desc = config.get_description("hp", 50);
        assert!(desc.is_some());
        assert!(desc.unwrap().contains("一般"));
    }

    #[test]
    fn test_get_display_name() {
        let config = NarrativeConfig::builtin();
        assert_eq!(config.get_display_name("hp"), Some("生命值"));
        assert_eq!(config.get_display_name("hunger"), Some("饥饿"));
    }

    #[test]
    fn test_serde_roundtrip() {
        let config = NarrativeConfig::builtin();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: NarrativeConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, parsed);
    }

    // --- AgentRecognition 容错反序列化测试 ---

    #[test]
    fn test_recognition_from_valid_object() {
        let json = r#"{"relative_position":"身旁","appearance":"青衫剑客","current_activity":"饮酒","recognition":{"is_known":true,"known_name":"红娘子","relationship":"旧识"}}"#;
        let p: AgentPerception = serde_json::from_str(json).unwrap();
        let r = p.recognition.unwrap();
        assert!(r.is_known);
        assert_eq!(r.known_name.as_deref(), Some("红娘子"));
        assert_eq!(r.relationship.as_deref(), Some("旧识"));
    }

    #[test]
    fn test_recognition_from_string() {
        let json = r#"{"relative_position":"对面","appearance":"蒙面人","current_activity":"观察","recognition":"红娘子"}"#;
        let p: AgentPerception = serde_json::from_str(json).unwrap();
        let r = p.recognition.unwrap();
        assert!(r.is_known);
        assert_eq!(r.known_name.as_deref(), Some("红娘子"));
        assert!(r.relationship.is_none());
    }

    #[test]
    fn test_recognition_from_bool() {
        let json = r#"{"relative_position":"远处","appearance":"老者","current_activity":"打坐","recognition":true}"#;
        let p: AgentPerception = serde_json::from_str(json).unwrap();
        let r = p.recognition.unwrap();
        assert!(r.is_known);
        assert!(r.known_name.is_none());
    }

    #[test]
    fn test_recognition_from_null() {
        let json = r#"{"relative_position":"角落","appearance":"陌生人","current_activity":"发呆","recognition":null}"#;
        let p: AgentPerception = serde_json::from_str(json).unwrap();
        assert!(p.recognition.is_none());
    }

    #[test]
    fn test_recognition_from_number() {
        let json = r#"{"relative_position":"门外","appearance":"路人","current_activity":"行走","recognition":42}"#;
        let p: AgentPerception = serde_json::from_str(json).unwrap();
        assert!(p.recognition.is_none());
    }

    #[test]
    fn test_recognition_missing_field() {
        let json = r#"{"relative_position":"身旁","appearance":"剑客","current_activity":"练剑"}"#;
        let p: AgentPerception = serde_json::from_str(json).unwrap();
        assert!(p.recognition.is_none());
    }

    #[test]
    fn test_recognition_from_empty_string() {
        let json = r#"{"relative_position":"身旁","appearance":"路人","current_activity":"发呆","recognition":""}"#;
        let p: AgentPerception = serde_json::from_str(json).unwrap();
        assert!(p.recognition.is_none());
    }

    #[test]
    fn test_recognition_from_whitespace_string() {
        let json = r#"{"relative_position":"身旁","appearance":"路人","current_activity":"发呆","recognition":"   "}"#;
        let p: AgentPerception = serde_json::from_str(json).unwrap();
        assert!(p.recognition.is_none());
    }

    #[test]
    fn test_recognition_from_partial_object() {
        // 缺少必填字段 is_known
        let json = r#"{"relative_position":"身旁","appearance":"路人","current_activity":"发呆","recognition":{"known_name":"红娘子"}}"#;
        let p: AgentPerception = serde_json::from_str(json).unwrap();
        assert!(p.recognition.is_none());
    }
}
