// ============================================================================
// 特质类型定义
// ============================================================================
//
// 定义人设中可演化的性格特质

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 特质类型分类
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TraitType {
    /// 社交特质（如：信任、友善、攻击性）
    Social,
    /// 道德特质（如：诚实、贪婪、正义感）
    Moral,
    /// 能力特质（如：勇敢、智慧、机敏）
    Capability,
    /// 情绪特质（如：愤怒、恐惧、喜悦）
    Emotional,
    /// 生存特质（如：求生欲、适应力）
    Survival,
}

impl TraitType {
    /// 获取特质类型的显示名称
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

/// 特质变化记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraitChange {
    /// 特质名称
    pub trait_name: String,
    /// 变化量（正数为增加，负数为减少）
    pub delta: i16,
    /// 变化原因
    pub reason: String,
    /// 发生变化的 Tick ID
    pub tick_id: i64,
    /// 时间戳
    pub timestamp: i64,
    /// 衰减率（每 Tick 减少的值）
    pub decay_rate: f32,
}

impl TraitChange {
    /// 创建新的特质变化
    pub fn new(trait_name: String, delta: i16, reason: String, tick_id: i64) -> Self {
        Self {
            trait_name,
            delta,
            reason,
            tick_id,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
            decay_rate: 0.1, // 默认衰减率
        }
    }

    /// 创建带衰减率的特质变化
    pub fn with_decay(mut self, decay_rate: f32) -> Self {
        self.decay_rate = decay_rate;
        self
    }
}

/// 特质定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trait {
    /// 特质名称（如：信任、贪婪、勇敢）
    pub name: String,
    /// 特质类型
    pub trait_type: TraitType,
    /// 当前值 (0-100)
    pub value: u8,
    /// 变化历史
    pub history: Vec<TraitChange>,
    /// 最小值
    pub min_value: u8,
    /// 最大值
    pub max_value: u8,
}

impl Trait {
    /// 创建新的特质
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

    /// 应用变化
    pub fn apply_change(&mut self, change: TraitChange, _tick_id: i64) {
        let old_value = self.value;
        let new_value = (old_value as i16 + change.delta).clamp(self.min_value as i16, self.max_value as i16) as u8;
        self.value = new_value;
        self.history.push(change);
    }

    /// 获取当前值
    pub fn value(&self) -> u8 {
        self.value
    }

    /// 获取值的叙事描述
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

    /// 应用衰减（每 Tick 调用）
    pub fn apply_decay(&mut self) {
        // 对最近的每个变化应用衰减
        for change in &mut self.history {
            if change.delta > 0 {
                // 正向变化会衰减
                let decay = (change.delta as f32 * change.decay_rate).ceil() as i16;
                change.delta = (change.delta - decay).max(0);
            } else {
                // 负向变化也会逐渐恢复
                let recovery = (change.delta.abs() as f32 * change.decay_rate).ceil() as i16;
                change.delta = (change.delta + recovery).min(0);
            }
        }

        // 重新计算当前值
        let base_value = 50; // 基准值
        let total_delta: i16 = self.history.iter().map(|h| h.delta).sum();
        self.value = (base_value + total_delta).clamp(self.min_value as i16, self.max_value as i16) as u8;
    }
}

/// 预定义的特质集合
impl Trait {
    /// 从预设的 AgentPrompt 解析特质
    pub fn parse_from_prompt(_prompt: &str, agent_name: &str) -> HashMap<String, Trait> {
        let mut traits = HashMap::new();

        // 根据不同的 Agent 预设不同的初始特质
        match agent_name {
            "柳云娘" => {
                traits.insert("贪婪".to_string(), Trait::new("贪婪".to_string(), TraitType::Moral, 70));
                traits.insert("信誉".to_string(), Trait::new("信誉".to_string(), TraitType::Moral, 80));
                traits.insert("精明".to_string(), Trait::new("精明".to_string(), TraitType::Capability, 85));
                traits.insert("同情心".to_string(), Trait::new("同情心".to_string(), TraitType::Moral, 50));
                traits.insert("求生欲".to_string(), Trait::new("求生欲".to_string(), TraitType::Survival, 75));
            }
            "燕无归" => {
                traits.insert("沉默".to_string(), Trait::new("沉默".to_string(), TraitType::Social, 90));
                traits.insert("复仇心".to_string(), Trait::new("复仇心".to_string(), TraitType::Survival, 95));
                traits.insert("孤独".to_string(), Trait::new("孤独".to_string(), TraitType::Social, 80));
                traits.insert("正义感".to_string(), Trait::new("正义感".to_string(), TraitType::Moral, 60));
                traits.insert("求生欲".to_string(), Trait::new("求生欲".to_string(), TraitType::Survival, 90));
            }
            "方子清" => {
                traits.insert("迂腐".to_string(), Trait::new("迂腐".to_string(), TraitType::Capability, 70));
                traits.insert("书卷气".to_string(), Trait::new("书卷气".to_string(), TraitType::Capability, 80));
                traits.insert("善良".to_string(), Trait::new("善良".to_string(), TraitType::Moral, 75));
                traits.insert("天真".to_string(), Trait::new("天真".to_string(), TraitType::Social, 60));
                traits.insert("求知欲".to_string(), Trait::new("求知欲".to_string(), TraitType::Capability, 70));
            }
            "小翠" => {
                traits.insert("机灵".to_string(), Trait::new("机灵".to_string(), TraitType::Capability, 85));
                traits.insert("谨慎".to_string(), Trait::new("谨慎".to_string(), TraitType::Social, 80));
                traits.insert("戒备".to_string(), Trait::new("戒备".to_string(), TraitType::Emotional, 75));
                traits.insert("嘴甜".to_string(), Trait::new("嘴甜".to_string(), TraitType::Social, 70));
                traits.insert("求生欲".to_string(), Trait::new("求生欲".to_string(), TraitType::Survival, 85));
            }
            "钱三通" => {
                traits.insert("贪婪".to_string(), Trait::new("贪婪".to_string(), TraitType::Moral, 90));
                traits.insert("圆滑".to_string(), Trait::new("圆滑".to_string(), TraitType::Social, 85));
                traits.insert("中立".to_string(), Trait::new("中立".to_string(), TraitType::Moral, 70));
                traits.insert("精明".to_string(), Trait::new("精明".to_string(), TraitType::Capability, 80));
                traits.insert("求生欲".to_string(), Trait::new("求生欲".to_string(), TraitType::Survival, 70));
            }
            _ => {
                // 默认特质
                traits.insert("友善".to_string(), Trait::new("友善".to_string(), TraitType::Social, 50));
                traits.insert("好奇".to_string(), Trait::new("好奇".to_string(), TraitType::Capability, 50));
                traits.insert("求生欲".to_string(), Trait::new("求生欲".to_string(), TraitType::Survival, 70));
            }
        }

        traits
    }
}

// ============================================================================
// 测试
// ============================================================================

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

        // 测试上限
        let change = TraitChange::new("测试".to_string(), 200, "大量增加".to_string(), 100);
        trait_obj.apply_change(change, 100);
        assert_eq!(trait_obj.value(), 100);

        // 测试下限
        let change2 = TraitChange::new("测试".to_string(), -200, "大量减少".to_string(), 101);
        trait_obj.apply_change(change2, 101);
        assert_eq!(trait_obj.value(), 0);
    }

    #[test]
    fn test_trait_decay() {
        let mut trait_obj = Trait::new("愤怒".to_string(), TraitType::Emotional, 50);
        let change = TraitChange::new("愤怒".to_string(), 30, "被攻击".to_string(), 100);
        trait_obj.apply_change(change, 100);

        // 初始值应该是 80 (50 + 30)
        assert_eq!(trait_obj.value(), 80);

        // 应用衰减后，值应该降低
        trait_obj.apply_decay();
        assert!(trait_obj.value() < 80);
        assert!(trait_obj.value() >= 50); // 不会低于基准值
    }
}
