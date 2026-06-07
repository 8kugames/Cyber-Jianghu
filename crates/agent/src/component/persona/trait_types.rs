// ============================================================================
// 特质类型定义
// ============================================================================
//
// 定义人设中可演化的性格特质

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TraitType {
    Social,
    Moral,
    Capability,
    Emotional,
    Survival,
}

impl TraitType {
    pub fn display_name(&self) -> &str {
        match self {
            TraitType::Social => "社交",
            TraitType::Moral => "道德",
            TraitType::Capability => "能力",
            TraitType::Emotional => "情绪",
            TraitType::Survival => "生存",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraitChange {
    pub trait_name: String,
    pub delta: i16,
    pub reason: String,
    pub tick_id: i64,
    pub timestamp: i64,
    pub decay_rate: f32,
}

impl TraitChange {
    pub fn new(trait_name: String, delta: i16, reason: String, tick_id: i64) -> Self {
        Self {
            trait_name,
            delta,
            reason,
            tick_id,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time is always after UNIX_EPOCH")
                .as_secs() as i64,
            decay_rate: 0.1,
        }
    }

    pub fn with_decay(mut self, decay_rate: f32) -> Self {
        self.decay_rate = decay_rate;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trait {
    pub name: String,
    pub trait_type: TraitType,
    pub value: u8,
    pub(crate) history: Vec<TraitChange>,
    pub(crate) min_value: u8,
    pub(crate) max_value: u8,
}

impl Trait {
    pub fn new(name: String, trait_type: TraitType, value: u8) -> Self {
        Self {
            name,
            trait_type,
            value: value.clamp(0, 100),
            history: Vec::new(),
            min_value: 0,
            max_value: 100,
        }
    }

    pub fn min_value(&self) -> u8 {
        self.min_value
    }

    pub fn max_value(&self) -> u8 {
        self.max_value
    }

    pub fn apply_change(&mut self, change: TraitChange, _tick_id: i64) {
        let old_value = self.value;
        let new_value = (old_value as i16 + change.delta)
            .clamp(self.min_value as i16, self.max_value as i16) as u8;
        self.value = new_value;
        self.history.push(change);
    }

    pub fn value(&self) -> u8 {
        self.value
    }

    pub fn narrative_description(&self) -> String {
        let level_desc = match self.value {
            0..=20 => "很低",
            21..=40 => "较低",
            41..=60 => "中等",
            61..=80 => "较高",
            81..=100 => "很高",
            _ => "未知",
        };
        format!("{}{}", self.name, level_desc)
    }

    pub fn apply_decay(&mut self) {
        for change in &mut self.history {
            if change.delta > 0 {
                let decay = (change.delta as f32 * change.decay_rate).ceil() as i16;
                change.delta = (change.delta - decay).max(0);
            } else {
                let recovery = (change.delta.abs() as f32 * change.decay_rate).ceil() as i16;
                change.delta = (change.delta + recovery).min(0);
            }
        }

        let base_value = 50;
        let total_delta: i16 = self.history.iter().map(|h| h.delta).sum();
        self.value =
            (base_value + total_delta).clamp(self.min_value as i16, self.max_value as i16) as u8;
    }
}

pub(crate) fn default_traits() -> HashMap<String, Trait> {
    HashMap::from_iter([
        (
            "友善".to_string(),
            Trait::new("友善".to_string(), TraitType::Social, 50),
        ),
        (
            "好奇".to_string(),
            Trait::new("好奇".to_string(), TraitType::Capability, 50),
        ),
        (
            "求生欲".to_string(),
            Trait::new("求生欲".to_string(), TraitType::Survival, 70),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trait_creation() {
        let trait_obj = Trait::new("勇敢".to_string(), TraitType::Capability, 75);
        assert_eq!(trait_obj.name, "勇敢");
        assert_eq!(trait_obj.value(), 75);
        assert_eq!(trait_obj.narrative_description(), "勇敢较高");
    }

    #[test]
    fn test_trait_change() {
        let mut trait_obj = Trait::new("信任".to_string(), TraitType::Social, 50);
        let change = TraitChange::new("信任".to_string(), -10, "被骗了".to_string(), 100);
        trait_obj.apply_change(change, 100);
        assert_eq!(trait_obj.value(), 40);
    }

    #[test]
    fn test_trait_value_clamp() {
        let mut trait_obj = Trait::new("测试".to_string(), TraitType::Social, 50);
        let change = TraitChange::new("测试".to_string(), 200, "大量增加".to_string(), 100);
        trait_obj.apply_change(change, 100);
        assert_eq!(trait_obj.value(), 100);

        let change2 = TraitChange::new("测试".to_string(), -200, "大量减少".to_string(), 101);
        trait_obj.apply_change(change2, 101);
        assert_eq!(trait_obj.value(), 0);
    }

    #[test]
    fn test_trait_decay() {
        let mut trait_obj = Trait::new("愤怒".to_string(), TraitType::Emotional, 50);
        let change = TraitChange::new("愤怒".to_string(), 30, "被攻击".to_string(), 100);
        trait_obj.apply_change(change, 100);

        assert_eq!(trait_obj.value(), 80);

        trait_obj.apply_decay();
        assert!(trait_obj.value() < 80);
        assert!(trait_obj.value() >= 50);
    }
}
