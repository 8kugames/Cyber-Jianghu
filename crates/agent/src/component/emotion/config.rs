use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreAffectConfig {
    #[serde(default = "default_decay_rate")]
    pub decay_rate: f32,
    #[serde(default)]
    pub default_baseline_valence: f32,
    #[serde(default = "default_baseline_arousal")]
    pub default_baseline_arousal: f32,
    #[serde(default = "default_valence_range")]
    pub valence_range: [f32; 2],
    #[serde(default = "default_arousal_range")]
    pub arousal_range: [f32; 2],
    #[serde(default)]
    pub attributes: Vec<AffectAttributeRule>,
    #[serde(default)]
    pub events: Vec<AffectEventRule>,
    #[serde(default)]
    pub baseline_traits: Vec<BaselineTraitRule>,
    #[serde(default = "default_trait_intensity_scale")]
    pub trait_intensity_scale: f32,
    #[serde(default = "default_over_arousal_damping")]
    pub over_arousal_damping: f32,
}

fn default_trait_intensity_scale() -> f32 {
    15.0
}
fn default_over_arousal_damping() -> f32 {
    0.5
}
fn default_decay_rate() -> f32 {
    0.05
}
fn default_baseline_arousal() -> f32 {
    0.3
}
fn default_valence_range() -> [f32; 2] {
    [-1.0, 1.0]
}
fn default_arousal_range() -> [f32; 2] {
    [0.0, 1.0]
}

impl Default for CoreAffectConfig {
    fn default() -> Self {
        Self {
            decay_rate: 0.05,
            default_baseline_valence: 0.0,
            default_baseline_arousal: 0.3,
            valence_range: [-1.0, 1.0],
            arousal_range: [0.0, 1.0],
            attributes: Vec::new(),
            events: Vec::new(),
            baseline_traits: Vec::new(),
            trait_intensity_scale: 15.0,
            over_arousal_damping: 0.5,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AffectAttributeRule {
    pub attribute: String,
    pub comfort_zone: [f32; 2],
    #[serde(default)]
    pub absolute_floor: f32,
    #[serde(default)]
    pub valence_sensitivity: f32,
    #[serde(default)]
    pub arousal_sensitivity: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AffectEventRule {
    pub event_category: String,
    pub outcome: String,
    #[serde(default)]
    pub valence_delta: f32,
    #[serde(default)]
    pub arousal_delta: f32,
    #[serde(default = "default_one")]
    pub importance_weight: f32,
    #[serde(default = "default_one")]
    pub negativity_multiplier: f32,
}

fn default_one() -> f32 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaselineTraitRule {
    pub trait_name: String,
    #[serde(default)]
    pub valence_weight: f32,
    #[serde(default)]
    pub arousal_weight: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncodingConfig {
    #[serde(default = "default_encoding_function")]
    pub function: String,
    #[serde(default = "default_one")]
    pub intercept: f32,
    #[serde(default = "default_half")]
    pub slope: f32,
    #[serde(default = "default_one_point_five")]
    pub exponent: f32,
    #[serde(default)]
    pub flashbulb: FlashbulbConfig,
    #[serde(default = "default_output_range")]
    pub output_range: [f32; 2],
    #[serde(default = "default_fallback_warn")]
    pub unknown_function_fallback: String,
}

fn default_encoding_function() -> String {
    "linear".to_string()
}
fn default_half() -> f32 {
    0.5
}
fn default_one_point_five() -> f32 {
    1.5
}
fn default_output_range() -> [f32; 2] {
    [0.0, 1.0]
}
fn default_fallback_warn() -> String {
    "warn".to_string()
}

impl Default for EncodingConfig {
    fn default() -> Self {
        Self {
            function: "linear".to_string(),
            intercept: 1.0,
            slope: 0.5,
            exponent: 1.5,
            flashbulb: FlashbulbConfig::default(),
            output_range: [0.0, 1.0],
            unknown_function_fallback: "warn".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlashbulbConfig {
    #[serde(default = "default_eight_tenths")]
    pub threshold: f32,
    #[serde(default = "default_one_point_five")]
    pub multiplier: f32,
}

fn default_eight_tenths() -> f32 {
    0.8
}

impl Default for FlashbulbConfig {
    fn default() -> Self {
        Self {
            threshold: 0.8,
            multiplier: 1.5,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalConfig {
    #[serde(default = "default_three_tenths")]
    pub valence_bias_weight: f32,
    #[serde(default = "default_two")]
    pub valence_range: f32,
    #[serde(default)]
    pub null_encoding_bonus: f32,
}

fn default_two() -> f32 {
    2.0
}
fn default_three_tenths() -> f32 {
    0.3
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            valence_bias_weight: 0.3,
            valence_range: 2.0,
            null_encoding_bonus: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensationLabel {
    pub lo: f32,
    pub hi: f32,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensationConfig {
    #[serde(default = "default_sensation_template")]
    pub template: String,
    #[serde(default = "default_distress_template")]
    pub distress_template: String,
    #[serde(default = "default_distress_threshold")]
    pub distress_threshold: i32,
    #[serde(default = "default_valence_labels")]
    pub valence_labels: Vec<SensationLabel>,
    #[serde(default = "default_arousal_labels")]
    pub arousal_labels: Vec<SensationLabel>,
    #[serde(default = "default_fallback_label")]
    pub fallback_label: String,
}

fn default_distress_threshold() -> i32 {
    30
}

pub fn default_fallback_label() -> String {
    "未知".to_string()
}

impl Default for SensationConfig {
    fn default() -> Self {
        Self {
            template: default_sensation_template(),
            distress_template: default_distress_template(),
            distress_threshold: 30,
            valence_labels: default_valence_labels(),
            arousal_labels: default_arousal_labels(),
            fallback_label: default_fallback_label(),
        }
    }
}

fn default_sensation_template() -> String {
    "## 内在感受\n你的身体状态此刻传递给你这样的感觉：\n- 体感愉悦度：{valence_label}（{valence:.2}）\n- 激动程度：{arousal_label}（{arousal:.2}）\n{distress_hint}".to_string()
}

fn default_distress_template() -> String {
    "你的{attribute_name}正在侵蚀你的平静".to_string()
}

fn default_valence_labels() -> Vec<SensationLabel> {
    vec![
        SensationLabel {
            lo: -1.0,
            hi: -0.6,
            label: "强烈的不适与痛苦".into(),
        },
        SensationLabel {
            lo: -0.6,
            hi: -0.3,
            label: "明显的不安与不悦".into(),
        },
        SensationLabel {
            lo: -0.3,
            hi: -0.1,
            label: "轻微的低落".into(),
        },
        SensationLabel {
            lo: -0.1,
            hi: 0.1,
            label: "平静无波".into(),
        },
        SensationLabel {
            lo: 0.1,
            hi: 0.3,
            label: "轻微的舒适".into(),
        },
        SensationLabel {
            lo: 0.3,
            hi: 0.6,
            label: "明显的愉悦".into(),
        },
        SensationLabel {
            lo: 0.6,
            hi: 1.0,
            label: "强烈的欣喜与满足".into(),
        },
    ]
}

fn default_arousal_labels() -> Vec<SensationLabel> {
    vec![
        SensationLabel {
            lo: 0.0,
            hi: 0.2,
            label: "昏沉欲睡".into(),
        },
        SensationLabel {
            lo: 0.2,
            hi: 0.4,
            label: "平静清醒".into(),
        },
        SensationLabel {
            lo: 0.4,
            hi: 0.6,
            label: "精神集中".into(),
        },
        SensationLabel {
            lo: 0.6,
            hi: 0.8,
            label: "紧张兴奋".into(),
        },
        SensationLabel {
            lo: 0.8,
            hi: 1.0,
            label: "极度亢奋".into(),
        },
    ]
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EmotionConfig {
    #[serde(default)]
    pub core_affect: CoreAffectConfig,
    #[serde(default)]
    pub encoding: EncodingConfig,
    #[serde(default)]
    pub retrieval: RetrievalConfig,
    #[serde(default)]
    pub sensation: SensationConfig,
    #[serde(default)]
    pub outcome_mapping: HashMap<String, String>,
}
