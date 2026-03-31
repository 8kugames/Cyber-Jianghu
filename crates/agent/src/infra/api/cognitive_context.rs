// ============================================================================
// 认知上下文构建器 - 为 OpenClaw 生成结构化四阶段认知上下文
// ============================================================================
//
// 设计原则：
// - 将 WorldState 转换为叙事化的四阶段认知上下文
// - 引导 OpenClaw 的 LLM 按 Perception → Motivation → Planning → Decision 顺序推理
// - 不内置 LLM 调用，仅提供上下文数据

use crate::component::persona::dynamic_persona::DynamicPersona;
use crate::component::social::RelationshipStore;
use crate::soul::actor::narrative::{NarrativeEngine, PerceptionNarrative};
use cyber_jianghu_protocol::WorldState;
use serde::{Deserialize, Serialize};

// ============================================================================
// 认知上下文数据结构
// ============================================================================

/// 完整的认知上下文（四阶段推理）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CognitiveContext {
    /// 感知阶段上下文
    pub perception: PerceptionContext,
    /// 动机阶段上下文
    pub motivation: MotivationContext,
    /// 规划阶段上下文
    pub planning: PlanningContext,
    /// 决策阶段上下文
    pub decision: DecisionContext,
}

/// 感知阶段上下文
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerceptionContext {
    /// 自身状态叙事化描述
    pub self_status: String,
    /// 环境观察
    pub environment: String,
    /// 关键观察列表
    pub key_observations: Vec<String>,
}

/// 动机阶段上下文
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MotivationContext {
    /// 当前活跃驱动力列表
    pub active_drives: Vec<Drive>,
    /// 当前主导驱动力
    pub dominant_drive: String,
}

/// 驱动力
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Drive {
    /// 驱动力名称
    pub drive: String,
    /// 强度 (1-10)
    pub intensity: u8,
    /// 原因
    pub reason: String,
}

/// 规划阶段上下文
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningContext {
    /// 当前目标列表
    pub current_goals: Vec<String>,
    /// 可用动作列表
    pub available_actions: Vec<AvailableActionInfo>,
}

/// 可用动作信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvailableActionInfo {
    /// 动作名称
    pub action: String,
    /// 目标（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// 描述
    pub description: String,
}

/// 决策阶段上下文
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionContext {
    /// 是否需要推理
    pub requires_reasoning: bool,
    /// 思考提示
    pub thinking_prompt: String,
}

// ============================================================================
// 默认实现
// ============================================================================

impl Default for CognitiveContext {
    fn default() -> Self {
        Self {
            perception: PerceptionContext {
                self_status: "状态正常".to_string(),
                environment: "周围环境平静".to_string(),
                key_observations: vec![],
            },
            motivation: MotivationContext {
                active_drives: vec![],
                dominant_drive: "保持现状".to_string(),
            },
            planning: PlanningContext {
                current_goals: vec![],
                available_actions: vec![],
            },
            decision: DecisionContext {
                requires_reasoning: true,
                thinking_prompt:
                    "基于你的感知、动机和可用动作，决定下一步行动。请先用一段话描述你的推理过程。"
                        .to_string(),
            },
        }
    }
}

// ============================================================================
// 认知上下文构建器配置
// ============================================================================

/// 认知上下文构建器配置
#[derive(Debug, Clone)]
pub struct CognitiveContextConfig {
    /// 是否包含关系信息
    pub include_relationships: bool,
    /// 最大观察数量
    pub max_observations: usize,
    /// 最大目标数量
    pub max_goals: usize,
}

impl Default for CognitiveContextConfig {
    fn default() -> Self {
        Self {
            include_relationships: true,
            max_observations: 5,
            max_goals: 3,
        }
    }
}

// ============================================================================
// 认知上下文构建器
// ============================================================================

/// 认知上下文构建器
///
/// 从 WorldState 生成结构化的四阶段认知上下文
pub struct CognitiveContextBuilder {
    /// 叙事引擎
    narrative_engine: NarrativeEngine,
    /// 配置
    config: CognitiveContextConfig,
}

impl Default for CognitiveContextBuilder {
    fn default() -> Self {
        Self::new(
            NarrativeEngine::default(),
            CognitiveContextConfig::default(),
        )
    }
}

impl CognitiveContextBuilder {
    /// 创建新的构建器
    pub fn new(narrative_engine: NarrativeEngine, config: CognitiveContextConfig) -> Self {
        Self {
            narrative_engine,
            config,
        }
    }

    /// 使用默认配置创建
    pub fn with_narrative_engine(narrative_engine: NarrativeEngine) -> Self {
        Self::new(narrative_engine, CognitiveContextConfig::default())
    }

    /// 从 WorldState 构建认知上下文
    pub fn build(&self, world_state: &WorldState) -> CognitiveContext {
        self.build_with_persona(world_state, None, None)
    }

    /// 从 WorldState 和人设构建认知上下文
    pub fn build_with_persona(
        &self,
        world_state: &WorldState,
        persona: Option<&DynamicPersona>,
        relationship_store: Option<&RelationshipStore>,
    ) -> CognitiveContext {
        let perception = self.build_perception(world_state, relationship_store);
        let motivation = self.build_motivation(world_state, persona);
        let planning = self.build_planning(world_state);
        let decision = self.build_decision();

        CognitiveContext {
            perception,
            motivation,
            planning,
            decision,
        }
    }

    /// 构建感知上下文
    fn build_perception(
        &self,
        world_state: &WorldState,
        relationship_store: Option<&RelationshipStore>,
    ) -> PerceptionContext {
        let self_state = &world_state.self_state;

        let narrative: PerceptionNarrative = self
            .narrative_engine
            .generate_narrative(&self_state.attributes, &self_state.status_effects);

        let self_status = format!(
            "{}, {}, {}, {}",
            narrative.body_status,
            narrative.hunger_status,
            narrative.thirst_status,
            narrative.stamina_status
        );

        let environment = format!(
            "你正位于{}({})，天气{}",
            world_state.location.name,
            world_state.location.node_type,
            world_state.world_time.weather
        );

        let mut observations = Vec::new();

        for entity in &world_state.entities {
            let rel_info = relationship_store
                .and_then(|store| store.get_relationship(entity.id).ok().flatten())
                .map(|mem| format!("[{}]", mem.self_description))
                .unwrap_or_default();

            observations.push(format!(
                "附近有{}{}，状态: {}",
                entity.name, rel_info, entity.state
            ));
        }

        for item in &world_state.nearby_items {
            observations.push(format!("地上有{} x{}", item.name, item.quantity));
        }

        for event in world_state.events_log.iter().rev().take(3) {
            observations.push(event.description.clone());
        }

        PerceptionContext {
            self_status,
            environment,
            key_observations: observations
                .into_iter()
                .take(self.config.max_observations)
                .collect(),
        }
    }

    /// 构建动机上下文
    fn build_motivation(
        &self,
        world_state: &WorldState,
        persona: Option<&DynamicPersona>,
    ) -> MotivationContext {
        let mut drives = Vec::new();
        let attrs = &world_state.self_state.attributes;

        let hunger = attrs.get("hunger").copied().unwrap_or(50);
        if hunger < 40 {
            drives.push(Drive {
                drive: "寻找食物".to_string(),
                intensity: ((50 - hunger) / 5).min(10) as u8,
                reason: "肚子饿了，需要进食".to_string(),
            });
        }

        let thirst = attrs.get("thirst").copied().unwrap_or(50);
        if thirst < 40 {
            drives.push(Drive {
                drive: "寻找水源".to_string(),
                intensity: ((50 - thirst) / 5).min(10) as u8,
                reason: "口渴了，需要喝水".to_string(),
            });
        }

        let stamina = attrs.get("stamina").copied().unwrap_or(100);
        if stamina < 30 {
            drives.push(Drive {
                drive: "休息恢复".to_string(),
                intensity: ((100 - stamina) / 10).min(10) as u8,
                reason: "体力不足，需要休息".to_string(),
            });
        }

        let hp = attrs.get("hp").copied().unwrap_or(100);
        if hp < 50 {
            drives.push(Drive {
                drive: "治疗伤势".to_string(),
                intensity: ((100 - hp) / 5).min(10) as u8,
                reason: "身体受伤，需要治疗".to_string(),
            });
        }

        if let Some(p) = persona {
            for (trait_name, trait_obj) in p.traits.iter().take(2) {
                drives.push(Drive {
                    drive: trait_name.clone(),
                    intensity: (trait_obj.value() / 2).max(3),
                    reason: format!("基于{}的性格倾向", p.name),
                });
            }
        }

        if drives.is_empty() {
            drives.push(Drive {
                drive: "保持现状".to_string(),
                intensity: 3,
                reason: "一切正常，继续当前活动".to_string(),
            });
        }

        drives.sort_by(|a, b| b.intensity.cmp(&a.intensity));

        let dominant_drive = drives
            .first()
            .map(|d| d.drive.clone())
            .unwrap_or_else(|| "保持现状".to_string());

        MotivationContext {
            active_drives: drives,
            dominant_drive,
        }
    }

    /// 构建规划上下文
    fn build_planning(&self, world_state: &WorldState) -> PlanningContext {
        let mut goals = Vec::new();
        let attrs = &world_state.self_state.attributes;

        if attrs.get("hunger").copied().unwrap_or(50) < 40 {
            goals.push("寻找食物充饥".to_string());
        }
        if attrs.get("thirst").copied().unwrap_or(50) < 40 {
            goals.push("寻找水源解渴".to_string());
        }
        if attrs.get("hp").copied().unwrap_or(100) < 50 {
            goals.push("寻找方法治疗伤势".to_string());
        }

        if goals.is_empty() {
            goals.push("继续当前活动".to_string());
        }

        let available_actions: Vec<AvailableActionInfo> = world_state
            .available_actions
            .iter()
            .map(|action| AvailableActionInfo {
                action: action.action.clone(),
                target: None,
                description: action.description.clone(),
            })
            .collect();

        PlanningContext {
            current_goals: goals.into_iter().take(self.config.max_goals).collect(),
            available_actions,
        }
    }

    /// 构建决策上下文
    fn build_decision(&self) -> DecisionContext {
        DecisionContext {
            requires_reasoning: true,
            thinking_prompt:
                "基于你的感知、动机和可用动作，决定下一步行动。请先用一段话描述你的推理过程。"
                    .to_string(),
        }
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cognitive_context_default() {
        let ctx = CognitiveContext::default();
        assert!(!ctx.perception.self_status.is_empty());
        assert!(!ctx.decision.thinking_prompt.is_empty());
    }

    #[test]
    fn test_perception_context_serialization() {
        let ctx = PerceptionContext {
            self_status: "身体状态良好".to_string(),
            environment: "长安城东市".to_string(),
            key_observations: vec!["附近有商人".to_string()],
        };

        let json = serde_json::to_string(&ctx).unwrap();
        assert!(json.contains("self_status"));
        assert!(json.contains("key_observations"));
    }

    #[test]
    fn test_drive_serialization() {
        let drive = Drive {
            drive: "寻找食物".to_string(),
            intensity: 8,
            reason: "肚子饿了".to_string(),
        };

        let json = serde_json::to_string(&drive).unwrap();
        assert!(json.contains("drive"));
        assert!(json.contains("intensity"));
        assert!(json.contains("reason"));
    }

    #[test]
    fn test_drive_sorting() {
        let mut drives = [
            Drive {
                drive: "low".to_string(),
                intensity: 3,
                reason: "".to_string(),
            },
            Drive {
                drive: "high".to_string(),
                intensity: 8,
                reason: "".to_string(),
            },
            Drive {
                drive: "mid".to_string(),
                intensity: 5,
                reason: "".to_string(),
            },
        ];

        drives.sort_by(|a, b| b.intensity.cmp(&a.intensity));
        assert_eq!(drives[0].drive, "high");
        assert_eq!(drives[1].drive, "mid");
        assert_eq!(drives[2].drive, "low");
    }

    #[test]
    fn test_cognitive_context_json_structure() {
        let ctx = CognitiveContext::default();
        let json = serde_json::to_string_pretty(&ctx).unwrap();

        assert!(json.contains("perception"));
        assert!(json.contains("motivation"));
        assert!(json.contains("planning"));
        assert!(json.contains("decision"));
    }
}
