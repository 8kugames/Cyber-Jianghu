// ============================================================================
// 事件-特质映射器
// ============================================================================
//
// 将游戏事件映射为人设特质变化
//
// 核心功能:
// - 定义事件到特质变化的规则
// - 根据事件类型自动计算特质变化
// - 支持自定义映射规则
// ============================================================================

use serde::{Deserialize, Serialize};

use crate::models::{WorldEvent, WorldEventType};

use super::dynamic_persona::DynamicPersona;
use super::trait_types::TraitChange;

/// 事件类型分类
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventType {
    /// 被攻击
    Attacked,
    /// 被欺骗
    Deceived,
    /// 被帮助
    Helped,
    /// 交易成功
    TradeSuccess,
    /// 交易失败
    TradeFail,
    /// 战斗胜利
    BattleWin,
    /// 战斗失败
    BattleLose,
    /// 获取食物
    GetFood,
    /// 饥饿
    Hungry,
    /// 口渴
    Thirsty,
    /// 社交互动
    SocialInteraction,
    /// 其他事件
    Other,
}

/// 特质映射规则
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraitMappingRule {
    /// 事件类型
    pub event_type: EventType,
    /// 目标特质名称
    pub trait_name: String,
    /// 基础变化量
    pub base_delta: i16,
    /// 条件判断（可选）
    pub condition: Option<String>,
    /// 权重（影响变化幅度）
    pub weight: f32,
}

impl TraitMappingRule {
    /// 创建新的映射规则
    pub fn new(event_type: EventType, trait_name: String, base_delta: i16) -> Self {
        Self {
            event_type,
            trait_name,
            base_delta,
            condition: None,
            weight: 1.0,
        }
    }

    /// 创建带权重的规则
    pub fn with_weight(mut self, weight: f32) -> Self {
        self.weight = weight;
        self
    }

    /// 计算实际变化量
    pub fn calculate_delta(&self, _context: &EventContext) -> i16 {
        let mut delta = self.base_delta;
        // 应用权重
        delta = (delta as f32 * self.weight).round() as i16;
        delta
    }
}

/// 事件上下文
///
/// 提供事件发生时的额外信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventContext {
    /// 事件描述
    pub description: String,
    /// 涉及的其他 Agent
    pub other_agents: Vec<String>,
    /// 事件强度 (0.0 - 1.0)
    pub intensity: f32,
    /// 当前 Tick ID
    pub tick_id: i64,
}

impl EventContext {
    /// 从 WorldEvent 创建上下文
    pub fn from_world_event(event: &WorldEvent, tick_id: i64) -> Self {
        let other_agents = Self::extract_other_agents(event);

        Self {
            description: event.description.clone(),
            other_agents,
            intensity: 0.5, // 默认中等强度
            tick_id,
        }
    }

    /// 分类事件类型（静态辅助方法）
    pub fn classify_event(event: &WorldEvent) -> EventType {
        match event.event_type {
            WorldEventType::ActionResult => {
                let desc = event.description.to_lowercase();

                // 优先检查被动语态（被...攻击）
                if desc.contains("被") && desc.contains("攻击") {
                    return EventType::Attacked;
                }

                // 然后检查主动战斗
                if desc.contains("战斗") || (desc.contains("攻击") && !desc.contains("被")) {
                    if desc.contains("胜利") || desc.contains("成功") {
                        return EventType::BattleWin;
                    }
                    return EventType::BattleLose;
                }

                if desc.contains("交易") || desc.contains("买卖") {
                    if desc.contains("成功") {
                        return EventType::TradeSuccess;
                    }
                    return EventType::TradeFail;
                }
                if desc.contains("欺骗") || desc.contains("诈骗") {
                    return EventType::Deceived;
                }
                if desc.contains("帮助") {
                    return EventType::Helped;
                }
                EventType::Other
            }
            WorldEventType::EnvironmentalChange => {
                let desc = event.description.to_lowercase();
                if desc.contains("饥饿") {
                    return EventType::Hungry;
                }
                if desc.contains("口渴") {
                    return EventType::Thirsty;
                }
                EventType::Other
            }
            WorldEventType::SocialInteraction => EventType::SocialInteraction,
            _ => EventType::Other,
        }
    }

    /// 提取涉及的其他 Agent
    fn extract_other_agents(event: &WorldEvent) -> Vec<String> {
        let mut agents = Vec::new();

        // 从 metadata 中提取其他 Agent
        if let Some(obj) = event.metadata.as_object()
            && let Some(targets) = obj.get("targets").and_then(|v| v.as_array())
        {
            for target in targets {
                if let Some(name) = target.as_str() {
                    agents.push(name.to_string());
                }
            }
        }

        agents
    }
}

/// 事件-特质映射器
#[derive(Debug, Clone)]
pub struct EventTraitMapper {
    /// 映射规则列表
    rules: Vec<TraitMappingRule>,
}

impl Default for EventTraitMapper {
    fn default() -> Self {
        Self::new()
    }
}

impl EventTraitMapper {
    /// 创建新的映射器（无规则 — 生产路径应使用 `from_rules` + YAML 加载）
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    /// 从规则列表创建映射器（主要给 YAML loader 用）
    pub fn from_rules(rules: Vec<TraitMappingRule>) -> Self {
        Self { rules }
    }

    /// 获取规则列表（只读访问,主要给测试和外部观测用）
    pub fn rules(&self) -> &[TraitMappingRule] {
        &self.rules
    }

    /// 将事件映射为特质变化列表
    pub fn map_event(
        &self,
        event: &WorldEvent,
        _persona: &DynamicPersona,
        tick_id: i64,
    ) -> Vec<TraitChange> {
        let context = EventContext::from_world_event(event, tick_id);
        let event_type = EventContext::classify_event(event);

        let mut changes = Vec::new();

        for rule in &self.rules {
            if rule.event_type == event_type {
                let delta = rule.calculate_delta(&context);
                let reason = format!("{:?} 事件: {}", event_type, context.description);

                changes.push(
                    TraitChange::new(rule.trait_name.clone(), delta, reason, tick_id)
                        .with_decay(0.1),
                );
            }
        }

        changes
    }

    /// 从 JSON 更新规则（用于 WebSocket ConfigUpdate 动态下发）
    pub fn update_from_json(&mut self, json_value: serde_json::Value) -> anyhow::Result<()> {
        use anyhow::Context;
        use serde::Deserialize;

        #[derive(Deserialize)]
        struct RulesJson {
            rules: Vec<TraitMappingRule>,
        }

        let parsed: RulesJson =
            serde_json::from_value(json_value).context("解析 persona_event_rules JSON 失败")?;
        self.rules = parsed.rules;
        Ok(())
    }

    /// 将特质变化应用到人设
    pub fn apply_to_persona(&self, event: &WorldEvent, persona: &mut DynamicPersona, tick_id: i64) {
        let changes = self.map_event(event, persona, tick_id);

        for change in changes {
            persona.apply_trait_change(&change.trait_name, change.delta, change.reason, tick_id);
        }
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::WorldEvent;

    #[test]
    fn test_event_classification() {
        let mut metadata = serde_json::Map::new();
        metadata.insert("targets".to_string(), serde_json::json!(["测试者"]));

        let event = WorldEvent {
            event_type: WorldEventType::ActionResult,
            tick_id: 1,
            description: "被测试者攻击".to_string(),
            metadata: serde_json::Value::Object(metadata),
        };

        let _context = EventContext::from_world_event(&event, 1);
        let event_type = EventContext::classify_event(&event);

        assert_eq!(event_type, EventType::Attacked);
    }

    #[test]
    fn test_trait_mapping_rules() {
        let yaml_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("crates/server/config/persona_event_rules.yaml");
        let mapper = crate::component::persona::rules_loader::load_event_trait_rules(&yaml_path)
            .expect("YAML 26 规则必须可加载");
        assert_eq!(mapper.rules().len(), 26);

        // 验证被攻击规则（社交维度保留）
        let attacked_rule = mapper
            .rules()
            .iter()
            .find(|r| r.event_type == EventType::Attacked && r.trait_name == "攻击性");
        assert!(attacked_rule.is_some());
        assert_eq!(attacked_rule.unwrap().base_delta, 8);
    }

    #[test]
    fn test_map_event_to_changes() {
        let yaml_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("crates/server/config/persona_event_rules.yaml");
        let mapper = crate::component::persona::rules_loader::load_event_trait_rules(&yaml_path)
            .expect("YAML 26 规则必须可加载");
        let agent_id = uuid::Uuid::new_v4();
        let mut persona = DynamicPersona::new(agent_id, "测试角色", "基础描述");
        persona.set_trait("攻击性", 50);

        // 创建被攻击事件
        let mut metadata = serde_json::Map::new();
        metadata.insert("targets".to_string(), serde_json::json!(["攻击者"]));

        let event = WorldEvent {
            event_type: WorldEventType::ActionResult,
            tick_id: 1,
            description: "被攻击者攻击".to_string(),
            metadata: serde_json::Value::Object(metadata),
        };

        let changes = mapper.map_event(&event, &persona, 1);

        // 应该有攻击性变化
        let aggression_change = changes.iter().find(|c| c.trait_name == "攻击性");
        assert!(aggression_change.is_some());
        assert!(aggression_change.unwrap().delta > 0);
    }
}
