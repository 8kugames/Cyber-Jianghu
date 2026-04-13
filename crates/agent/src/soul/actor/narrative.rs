// ============================================================================
// 叙事化状态描述模块 (数据驱动架构)
// ============================================================================
//
// [DEPRECATED] 此模块的属性映射功能计划由地魂 LLM 叙事管道替代。
// NarrativeGenerator (reflector/narrative_generator.rs) 通过 LLM 将 WorldState
// 数值转换为叙事描述，注入人魂 prompt 的 memory_context。
//
// 当前状态：NarrativeEngine 仍在以下路径中作为 primary 使用：
//   - engine.rs: CognitiveEngine Stage 1 prompt 构建 (PerceptionNarrative)
//   - claw/state.rs: Claw 模式上下文生成
//   - infra/api/: HTTP API 端点 (/api/v1/context, /api/v1/attributes)
// 两者（NarrativeEngine + NarrativeGenerator）是互补关系，不是替代关系。
// 待后续重构将 Stage 1 也迁移到 LLM 叙事管道后，此模块可移除。
//
// 架构说明:
// - NarrativeConfig: 配置数据结构（可从 JSON 加载）
// - NarrativeEngine: 核心引擎，通过组合持有 Config
// - PerceptionNarrative: 输出结构，生成叙事化描述
// ============================================================================

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ============================================================================
// 配置数据结构
// ============================================================================

/// 单个阈值配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NarrativeThreshold {
    /// 最小值（包含）
    pub min: i32,
    /// 最大值（包含）
    pub max: i32,
    /// 叙事描述
    pub description: String,
}

/// 单个属性的叙事配置
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusEffectConfig {
    /// 效果描述
    pub description: String,
}

/// 完整的叙事配置
#[derive(Debug, Clone, Serialize, Deserialize)]
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
                        description: "肚子很饿，饥肠辘辘，需要尽快进食".to_string(),
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
                        description: "完全不渴，身体水分充足".to_string(),
                    },
                    NarrativeThreshold {
                        min: 60,
                        max: 79,
                        description: "略有口渴，暂不需要喝水".to_string(),
                    },
                    NarrativeThreshold {
                        min: 40,
                        max: 59,
                        description: "口渴明显，嗓子发干".to_string(),
                    },
                    NarrativeThreshold {
                        min: 20,
                        max: 39,
                        description: "非常口渴，嘴唇干裂".to_string(),
                    },
                    NarrativeThreshold {
                        min: 0,
                        max: 19,
                        description: "渴得难以忍受，出现脱水症状".to_string(),
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
                        description: "体力充沛，精神饱满".to_string(),
                    },
                    NarrativeThreshold {
                        min: 60,
                        max: 79,
                        description: "体力尚可，有些疲惫但还能活动".to_string(),
                    },
                    NarrativeThreshold {
                        min: 40,
                        max: 59,
                        description: "体力有些不支，不宜剧烈活动".to_string(),
                    },
                    NarrativeThreshold {
                        min: 20,
                        max: 39,
                        description: "精疲力竭，浑身乏力".to_string(),
                    },
                    NarrativeThreshold {
                        min: 0,
                        max: 19,
                        description: "体力耗尽，没有力气行动".to_string(),
                    },
                ],
                note: None,
            },
        );

        let mut status_effects = HashMap::new();
        status_effects.insert(
            "poisoned".to_string(),
            StatusEffectConfig {
                description: "身中剧毒，浑身难受".to_string(),
            },
        );
        status_effects.insert(
            "bleeding".to_string(),
            StatusEffectConfig {
                description: "正在流血，伤口疼痛".to_string(),
            },
        );
        status_effects.insert(
            "diseased".to_string(),
            StatusEffectConfig {
                description: "身患疾病，身体不适".to_string(),
            },
        );
        status_effects.insert(
            "exhausted".to_string(),
            StatusEffectConfig {
                description: "精疲力尽，需要休息".to_string(),
            },
        );
        status_effects.insert(
            "stunned".to_string(),
            StatusEffectConfig {
                description: "神志不清，头晕目眩".to_string(),
            },
        );

        // 先天属性叙事配置
        attributes.insert(
            "strength".to_string(),
            NarrativeAttributeConfig {
                name: "strength".to_string(),
                display_name: "力量".to_string(),
                thresholds: vec![],
                note: None,
            },
        );
        attributes.insert(
            "agility".to_string(),
            NarrativeAttributeConfig {
                name: "agility".to_string(),
                display_name: "敏捷".to_string(),
                thresholds: vec![],
                note: None,
            },
        );
        attributes.insert(
            "constitution".to_string(),
            NarrativeAttributeConfig {
                name: "constitution".to_string(),
                display_name: "根骨".to_string(),
                thresholds: vec![],
                note: None,
            },
        );
        attributes.insert(
            "intelligence".to_string(),
            NarrativeAttributeConfig {
                name: "intelligence".to_string(),
                display_name: "悟性".to_string(),
                thresholds: vec![],
                note: None,
            },
        );
        attributes.insert(
            "charisma".to_string(),
            NarrativeAttributeConfig {
                name: "charisma".to_string(),
                display_name: "魅力".to_string(),
                thresholds: vec![],
                note: None,
            },
        );
        attributes.insert(
            "luck".to_string(),
            NarrativeAttributeConfig {
                name: "luck".to_string(),
                display_name: "福缘".to_string(),
                thresholds: vec![],
                note: None,
            },
        );

        // 属性分类映射
        let mut attribute_categories = HashMap::new();
        attribute_categories.insert(
            "primary".to_string(),
            vec![
                "strength".to_string(),
                "agility".to_string(),
                "constitution".to_string(),
                "intelligence".to_string(),
                "charisma".to_string(),
                "luck".to_string(),
            ],
        );
        attribute_categories.insert(
            "status".to_string(),
            vec![
                "hp".to_string(),
                "stamina".to_string(),
                "hunger".to_string(),
                "thirst".to_string(),
            ],
        );
        attribute_categories.insert(
            "derived".to_string(),
            vec![
                "attack_power".to_string(),
                "defense".to_string(),
                "speed".to_string(),
            ],
        );

        Self {
            version: "0.0.1-builtin".to_string(),
            description: "内置默认叙事配置".to_string(),
            attribute_categories,
            attributes,
            status_effects,
        }
    }

    /// 从 JSON 文件加载配置
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// 从文件路径加载配置
    pub fn from_file(path: &std::path::Path) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        Self::from_json(&content).map_err(|e| e.into())
    }
}

// ============================================================================
// 叙事引擎 (COI 架构核心)
// ============================================================================

/// 叙事引擎 - 使用组合持有配置
///
/// 这是 COI 架构的核心：通过组合而非继承来实现功能扩展
/// 引擎持有 NarrativeConfig 的引用，而不是继承配置
pub struct NarrativeEngine {
    /// 配置（通过组合持有）
    config: NarrativeConfig,
}

impl Default for NarrativeEngine {
    fn default() -> Self {
        Self::new(NarrativeConfig::default())
    }
}

impl NarrativeEngine {
    /// 创建新的叙事引擎
    pub fn new(config: NarrativeConfig) -> Self {
        Self { config }
    }

    /// 使用内置配置创建引擎
    pub fn with_builtin_config() -> Self {
        Self::new(NarrativeConfig::builtin())
    }

    /// 从 JSON 文件加载配置并创建引擎
    pub fn from_file(path: &std::path::Path) -> Result<Self, Box<dyn std::error::Error>> {
        let config = NarrativeConfig::from_file(path)?;
        Ok(Self::new(config))
    }

    /// 更新配置（支持热更新）
    pub fn update_config(&mut self, config: NarrativeConfig) {
        self.config = config;
    }

    /// 获取当前配置
    pub fn config(&self) -> &NarrativeConfig {
        &self.config
    }

    /// 获取属性的显示名称
    ///
    /// 如果配置中没有定义，返回原始名称
    pub fn get_display_name(&self, attr_name: &str) -> Option<&str> {
        self.config
            .attributes
            .get(attr_name)
            .map(|c| c.display_name.as_str())
    }

    /// 将属性值转换为叙事描述
    ///
    /// 根据配置中的阈值找到匹配的描述
    pub fn describe_attribute(&self, attr_name: &str, value: i32) -> String {
        if let Some(attr_config) = self.config.attributes.get(attr_name) {
            for threshold in &attr_config.thresholds {
                if value >= threshold.min && value <= threshold.max {
                    return threshold.description.clone();
                }
            }
        }
        // 回退到默认描述
        format!("{}: {}", attr_name, value)
    }

    /// 将状态效果列表转换为叙事描述
    pub fn describe_status_effects(&self, effects: &[String]) -> Vec<String> {
        effects
            .iter()
            .filter_map(|effect| {
                self.config
                    .status_effects
                    .get(effect)
                    .map(|cfg| cfg.description.clone())
                    .or_else(|| Some(format!("状态效果: {}", effect)))
            })
            .collect()
    }

    /// 生成完整的感知叙事
    pub fn generate_narrative(
        &self,
        attributes: &HashMap<String, i32>,
        status_effects: &[String],
    ) -> PerceptionNarrative {
        PerceptionNarrative {
            body_status: self
                .describe_attribute("hp", attributes.get("hp").copied().unwrap_or(100)),
            hunger_status: self
                .describe_attribute("hunger", attributes.get("hunger").copied().unwrap_or(50)),
            thirst_status: self
                .describe_attribute("thirst", attributes.get("thirst").copied().unwrap_or(50)),
            stamina_status: self
                .describe_attribute("stamina", attributes.get("stamina").copied().unwrap_or(100)),
            status_effects: self.describe_status_effects(status_effects),
        }
    }
}

// ============================================================================
// 感知叙事输出结构
// ============================================================================

/// 感知叙事输出
///
/// 包含所有状态的叙事化描述，用于构建 Prompt
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerceptionNarrative {
    /// 身体状态（HP）
    pub body_status: String,
    /// 饥饿状态
    pub hunger_status: String,
    /// 口渴状态
    pub thirst_status: String,
    /// 体力状态
    pub stamina_status: String,
    /// 状态效果描述
    #[serde(default)]
    pub status_effects: Vec<String>,
}

impl Default for PerceptionNarrative {
    fn default() -> Self {
        Self {
            body_status: "身体状况正常".to_string(),
            hunger_status: "肚子不饿".to_string(),
            thirst_status: "不渴".to_string(),
            stamina_status: "体力充沛".to_string(),
            status_effects: Vec::new(),
        }
    }
}

impl PerceptionNarrative {
    /// 使用默认引擎从属性生成叙事
    pub fn from_attributes(attributes: &HashMap<String, i32>, status_effects: &[String]) -> Self {
        let engine = NarrativeEngine::default();
        engine.generate_narrative(attributes, status_effects)
    }

    /// 使用指定引擎生成叙事
    pub fn from_attributes_with_engine(
        engine: &NarrativeEngine,
        attributes: &HashMap<String, i32>,
        status_effects: &[String],
    ) -> Self {
        engine.generate_narrative(attributes, status_effects)
    }

    /// 格式化为 Prompt 中的状态描述
    pub fn to_prompt_section(&self) -> String {
        let mut section = String::new();
        section.push_str("### 自身状态\n");
        section.push_str(&format!("- 身体: {}\n", self.body_status));
        section.push_str(&format!("- 饥饿: {}\n", self.hunger_status));
        section.push_str(&format!("- 口渴: {}\n", self.thirst_status));
        section.push_str(&format!("- 体力: {}\n", self.stamina_status));

        if !self.status_effects.is_empty() {
            section.push_str("- 特殊状态: ");
            section.push_str(&self.status_effects.join("、"));
            section.push('\n');
        }

        section
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe_attribute() {
        let engine = NarrativeEngine::default();

        // 测试 HP 描述
        let hp_desc = engine.describe_attribute("hp", 95);
        assert!(hp_desc.contains("极佳") || hp_desc.contains("充沛"));

        let hp_desc_low = engine.describe_attribute("hp", 20);
        assert!(hp_desc_low.contains("重伤") || hp_desc_low.contains("意识模糊"));

        // 测试饥饿描述
        let hunger_desc = engine.describe_attribute("hunger", 30);
        assert!(hunger_desc.contains("饿"));
    }

    #[test]
    fn test_perception_narrative() {
        let mut attrs = HashMap::new();
        attrs.insert("hp".to_string(), 80);
        attrs.insert("hunger".to_string(), 40);
        attrs.insert("thirst".to_string(), 60);
        attrs.insert("stamina".to_string(), 90);

        let narrative = PerceptionNarrative::from_attributes(&attrs, &[]);
        assert!(!narrative.body_status.is_empty());
        assert!(!narrative.hunger_status.is_empty());
    }

    #[test]
    fn test_status_effects() {
        let engine = NarrativeEngine::default();
        let effects = vec!["poisoned".to_string(), "bleeding".to_string()];
        let descriptions = engine.describe_status_effects(&effects);
        assert_eq!(descriptions.len(), 2);
        assert!(descriptions[0].contains("毒"));
    }

    #[test]
    fn test_to_prompt_section() {
        let narrative = PerceptionNarrative {
            body_status: "身体状况良好".to_string(),
            hunger_status: "有些饿了".to_string(),
            thirst_status: "口渴明显".to_string(),
            stamina_status: "体力充沛".to_string(),
            status_effects: vec!["身中剧毒".to_string()],
        };

        let section = narrative.to_prompt_section();
        assert!(section.contains("自身状态"));
        assert!(section.contains("身体状况良好"));
        assert!(section.contains("身中剧毒"));
    }
}
