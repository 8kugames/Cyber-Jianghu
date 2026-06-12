pub mod config;
pub mod encoding;
pub mod outcome;
pub mod retrieval;
pub mod sensation;

use config::{AffectAttributeRule, AffectEventRule, BaselineTraitRule, CoreAffectConfig};
use std::collections::HashMap;

/// 核心情感：效价 x 唤醒度的连续空间
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CoreAffect {
    pub valence: f32,
    pub arousal: f32,
    pub baseline_valence: f32,
    pub baseline_arousal: f32,
    pub last_tick: i64,
}

impl CoreAffect {
    pub fn new(config: &CoreAffectConfig) -> Self {
        Self {
            valence: config.default_baseline_valence,
            arousal: config.default_baseline_arousal,
            baseline_valence: config.default_baseline_valence,
            baseline_arousal: config.default_baseline_arousal,
            last_tick: 0,
        }
    }

    /// 每 tick 更新：生理偏离 + 事件冲击 → 累积 → 自然衰减回归
    ///
    /// 语义：physiological_deltas 和 event_deltas **累加**到当前 valence/arousal。
    pub fn update(
        &mut self,
        tick_id: i64,
        physiological_deltas: (f32, f32),
        event_deltas: (f32, f32),
        config: &CoreAffectConfig,
    ) {
        // 阶段 1：生理偏离 + 事件冲击累加
        self.valence = (self.valence + physiological_deltas.0 + event_deltas.0)
            .clamp(config.valence_range[0], config.valence_range[1]);
        self.arousal = (self.arousal + physiological_deltas.1 + event_deltas.1)
            .clamp(config.arousal_range[0], config.arousal_range[1]);
        // 阶段 2：自然衰减回归基线
        self.valence += (self.baseline_valence - self.valence) * config.decay_rate;
        self.arousal += (self.baseline_arousal - self.arousal) * config.decay_rate;
        self.last_tick = tick_id;
    }

    /// 从属性偏离计算生理效价/唤醒度
    /// over_arousal_damping: 超过舒适区上限时唤醒度增幅的衰减系数
    pub fn compute_physiological_affect(
        attributes: &HashMap<String, i32>,
        rules: &[AffectAttributeRule],
        over_arousal_damping: f32,
    ) -> (f32, f32) {
        let mut vd = 0.0_f32;
        let mut ad = 0.0_f32;
        for rule in rules {
            if let Some(&current) = attributes.get(&rule.attribute) {
                let cf = current as f32;
                let [lo, hi] = rule.comfort_zone;
                let scale = (lo - rule.absolute_floor).max(1.0);
                if cf < lo {
                    let deviation = (lo - cf) / scale;
                    vd -= rule.valence_sensitivity * deviation;
                    ad += rule.arousal_sensitivity * deviation;
                } else if cf > hi {
                    let deviation = (cf - hi) / scale;
                    vd -= rule.valence_sensitivity * deviation;
                    ad += rule.arousal_sensitivity * deviation * over_arousal_damping;
                }
            }
        }
        (vd, ad)
    }

    /// 从事件计算效价/唤醒度（含 negativity bias）
    pub fn compute_event_affect(
        event_category: &str,
        outcome: Option<&str>,
        importance: f32,
        rules: &[AffectEventRule],
    ) -> (f32, f32) {
        let outcome_str = outcome.unwrap_or("neutral");
        rules
            .iter()
            .filter(|r| r.event_category == event_category && r.outcome == outcome_str)
            .fold((0.0_f32, 0.0_f32), |(v, a), r| {
                let effective_valence = if r.valence_delta < 0.0 {
                    r.valence_delta * r.negativity_multiplier
                } else {
                    r.valence_delta
                };
                (
                    v + effective_valence * importance * r.importance_weight,
                    a + r.arousal_delta * importance * r.importance_weight,
                )
            })
    }

    /// 从特质计算个性基线
    pub fn compute_baseline(
        traits: &HashMap<String, crate::component::persona::Trait>,
        rules: &[BaselineTraitRule],
        defaults: (f32, f32),
    ) -> (f32, f32) {
        rules.iter().fold(defaults, |(v, a), rule| {
            let norm = traits
                .get(&rule.trait_name)
                .map(|t| {
                    let range = (t.max_value() - t.min_value()) as f32;
                    if range == 0.0 {
                        0.0
                    } else {
                        ((t.value - t.min_value()) as f32 / range) * 2.0 - 1.0
                    }
                })
                .unwrap_or(0.0);
            (
                v + norm * rule.valence_weight,
                a + norm * rule.arousal_weight,
            )
        })
    }

    /// 更新个性基线（当特质变化时调用）
    pub fn update_baseline(
        &mut self,
        traits: &HashMap<String, crate::component::persona::Trait>,
        rules: &[BaselineTraitRule],
        config: &CoreAffectConfig,
    ) {
        let (bv, ba) = Self::compute_baseline(
            traits,
            rules,
            (
                config.default_baseline_valence,
                config.default_baseline_arousal,
            ),
        );
        self.baseline_valence = bv;
        self.baseline_arousal = ba;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::*;

    fn make_attribute_rules() -> Vec<AffectAttributeRule> {
        vec![
            AffectAttributeRule {
                attribute: "satiation".into(),
                comfort_zone: [40.0, 100.0],
                absolute_floor: 0.0,
                valence_sensitivity: 0.3,
                arousal_sensitivity: 0.2,
            },
            AffectAttributeRule {
                attribute: "hp".into(),
                comfort_zone: [30.0, 100.0],
                absolute_floor: 0.0,
                valence_sensitivity: 0.4,
                arousal_sensitivity: 0.3,
            },
        ]
    }

    #[test]
    fn test_physiological_below_lower_bound() {
        let attrs = HashMap::from([("satiation".into(), 20), ("hp".into(), 80)]);
        let (v, a) = CoreAffect::compute_physiological_affect(&attrs, &make_attribute_rules(), 0.5);
        assert!(
            v < 0.0,
            "satiation 低于舒适区应产生负效价，got valence={}",
            v
        );
        assert!(
            a > 0.0,
            "satiation 低于舒适区应产生正唤醒度，got arousal={}",
            a
        );
    }

    #[test]
    fn test_physiological_in_comfort_zone() {
        let attrs = HashMap::from([("satiation".into(), 60), ("hp".into(), 80)]);
        let (v, a) = CoreAffect::compute_physiological_affect(&attrs, &make_attribute_rules(), 0.5);
        assert!(
            (v - 0.0).abs() < 0.001,
            "舒适区内应无偏离，got valence={}",
            v
        );
        assert!(
            (a - 0.0).abs() < 0.001,
            "舒适区内应无偏离，got arousal={}",
            a
        );
    }

    #[test]
    fn test_physiological_above_upper_bound() {
        let attrs = HashMap::from([("satiation".into(), 60), ("hp".into(), 120)]);
        let (v, a) = CoreAffect::compute_physiological_affect(&attrs, &make_attribute_rules(), 0.5);
        assert!(v < 0.0, "超过上限应产生负效价，got valence={}", v);
        assert!(a > 0.0, "超过上限应产生正唤醒度，got arousal={}", a);
    }

    #[test]
    fn test_over_arousal_damping_configurable() {
        let attrs = HashMap::from([("satiation".into(), 60), ("hp".into(), 120)]);
        let (_, a_half) =
            CoreAffect::compute_physiological_affect(&attrs, &make_attribute_rules(), 0.5);
        let (_, a_full) =
            CoreAffect::compute_physiological_affect(&attrs, &make_attribute_rules(), 1.0);
        assert!(a_full > a_half, "damping=1.0 应比 damping=0.5 唤醒度更高");
    }

    #[test]
    fn test_physiological_deviation_scale_consistency() {
        let attrs_satiation = HashMap::from([("satiation".into(), 20), ("hp".into(), 80)]);
        let attrs_hp = HashMap::from([("satiation".into(), 60), ("hp".into(), 15)]);
        let (v1, _) = CoreAffect::compute_physiological_affect(
            &attrs_satiation,
            &make_attribute_rules(),
            0.5,
        );
        let (v2, _) =
            CoreAffect::compute_physiological_affect(&attrs_hp, &make_attribute_rules(), 0.5);
        assert!(
            (v1 - (-0.15)).abs() < 0.001,
            "satiation 偏离 0.5 * sens 0.3 = -0.15，got {}",
            v1
        );
        assert!(
            (v2 - (-0.20)).abs() < 0.001,
            "hp 偏离 0.5 * sens 0.4 = -0.20，got {}",
            v2
        );
    }

    #[test]
    fn test_event_affect_negativity_bias() {
        let rules = vec![
            AffectEventRule {
                event_category: "action_result".into(),
                outcome: "success".into(),
                valence_delta: 0.1,
                arousal_delta: 0.05,
                importance_weight: 1.0,
                negativity_multiplier: 1.0,
            },
            AffectEventRule {
                event_category: "action_result".into(),
                outcome: "failure".into(),
                valence_delta: -0.1,
                arousal_delta: 0.15,
                importance_weight: 1.0,
                negativity_multiplier: 2.5,
            },
        ];
        let (v_success, _) =
            CoreAffect::compute_event_affect("action_result", Some("success"), 1.0, &rules);
        let (v_failure, _) =
            CoreAffect::compute_event_affect("action_result", Some("failure"), 1.0, &rules);
        assert!((v_success - 0.1).abs() < 0.001);
        assert!(
            (v_failure - (-0.25)).abs() < 0.001,
            "failure: -0.1 * 2.5 = -0.25，got {}",
            v_failure
        );
    }

    #[test]
    fn test_decay_to_baseline() {
        let config = CoreAffectConfig::default();
        let mut affect = CoreAffect::new(&config);
        affect.valence = -0.5;
        affect.arousal = 0.8;
        affect.update(1, (0.0, 0.0), (0.0, 0.0), &config);
        assert!(
            (affect.valence - (-0.475)).abs() < 0.001,
            "got {}",
            affect.valence
        );
        assert!(
            (affect.arousal - 0.775).abs() < 0.001,
            "got {}",
            affect.arousal
        );
    }
}
