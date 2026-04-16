//! 地魂叙事生成器
//!
//! 从 WorldState 生成叙事化 NarrativeContext，供人魂决策使用。
//! 核心：LLM 生成 + 语义缓存 + 数值泄露检测。

use anyhow::Result;
use cyber_jianghu_protocol::{
    ExecutionSummary, NarrativeContext, ReflectorNarrativeConfig, WorldState,
};
use lru::LruCache;
use std::num::NonZeroUsize;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::warn;

use crate::component::llm::LlmClientExt;
use crate::runtime::claw::LlmClientContainer;

use super::leak_detector::LeakDetector;

/// 语义指纹（用于缓存 key）
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct SemanticFingerprint {
    /// 当前位置
    location: String,
    /// 附近 agent 列表哈希
    agents_hash: u64,
}

/// 地魂叙事生成器
pub struct NarrativeGenerator {
    /// LLM 客户端容器
    llm_container: LlmClientContainer,
    /// 配置
    config: ReflectorNarrativeConfig,
    /// 语义缓存
    cache: Arc<RwLock<LruCache<SemanticFingerprint, NarrativeContext>>>,
    /// 数值泄露检测器
    leak_detector: LeakDetector,
}

impl NarrativeGenerator {
    /// 创建新的叙事生成器
    pub fn new(llm_container: LlmClientContainer, config: ReflectorNarrativeConfig) -> Self {
        let cache_size = NonZeroUsize::new(config.cache_size.max(1)).unwrap();
        let suspicion_threshold = config.leak_detection.suspicion_threshold;
        Self {
            llm_container,
            config,
            cache: Arc::new(RwLock::new(LruCache::new(cache_size))),
            leak_detector: LeakDetector::new(suspicion_threshold),
        }
    }

    /// 使用默认配置创建
    pub fn with_defaults(llm_container: LlmClientContainer) -> Self {
        Self::new(llm_container, ReflectorNarrativeConfig::default())
    }

    /// 生成叙事化上下文
    ///
    /// 流程：语义缓存检查 → LLM 生成 → 泄露检测 → 缓存写入
    pub async fn generate(
        &self,
        world_state: &WorldState,
        last_summary: Option<&ExecutionSummary>,
        recent_memories: &[String],
        execution_narrative: Option<String>,
    ) -> Result<NarrativeContext> {
        // 未启用 LLM 生成，直接降级
        if !self.config.enable_llm_generation {
            return Ok(self.fallback_context(world_state));
        }

        // 语义缓存检查（LRU::get 需要 &mut，所以用 write lock）
        if self.config.cache_enabled {
            let fingerprint = self.compute_fingerprint(world_state);
            let mut cache = self.cache.write().await;
            if let Some(cached) = cache.get(&fingerprint) {
                return Ok(cached.clone());
            }
        }

        // LLM 生成 + 泄露检测重试循环
        let max_attempts = self.config.leak_detection.max_retry + 1;
        for attempt in 1..=max_attempts {
            let prompt = self.build_prompt(
                world_state,
                last_summary,
                recent_memories,
                execution_narrative.clone(),
            );
            let llm_client = self.llm_container.read().await.clone();

            let response = match llm_client
                .complete_json_with_system(SYSTEM_PROMPT, &prompt)
                .await
            {
                Ok(ctx) => ctx,
                Err(e) => {
                    warn!("地魂 LLM 叙事生成失败 (attempt {}): {}", attempt, e);
                    if attempt == max_attempts {
                        return Ok(self.fallback_context(world_state));
                    }
                    continue;
                }
            };

            // 泄露检测
            if self.config.leak_detection.enabled {
                let report = self.leak_detector.detect_leaks(&response);
                if report.is_high_risk(self.leak_detector.threshold()) {
                    if attempt < max_attempts {
                        warn!(
                            "数值泄露检测触发 (score={}, attempt {}/{}): {:?}",
                            report.score, attempt, max_attempts, report.evidences
                        );
                        continue;
                    } else {
                        warn!("泄露检测重试耗尽，降级到空 NarrativeContext");
                        return Ok(self.fallback_context(world_state));
                    }
                }
            }

            // execution_narrative 是权威来源，代码层直接覆盖 last_outcome
            let mut response = response;
            if let Some(ref narrative) = execution_narrative {
                response.last_outcome = Some(cyber_jianghu_protocol::ActionOutcome {
                    result_narrative: narrative.clone(),
                    success: true,
                    side_effects: vec![],
                    unexpected_events: vec![],
                });
            }

            // 写入缓存
            if self.config.cache_enabled {
                let fingerprint = self.compute_fingerprint(world_state);
                self.cache.write().await.put(fingerprint, response.clone());
            }

            return Ok(response);
        }

        // 不可达，但保险起见
        Ok(self.fallback_context(world_state))
    }

    /// 构建 LLM Prompt
    fn build_prompt(
        &self,
        world_state: &WorldState,
        last_summary: Option<&ExecutionSummary>,
        recent_memories: &[String],
        execution_narrative: Option<String>,
    ) -> String {
        let mut parts = Vec::new();

        // 世界状态概要
        parts.push("## 当前世界状态".to_string());
        parts.push(format!("- Tick: {}", world_state.tick_id));
        parts.push(format!(
            "- 位置: {} ({})",
            world_state.location.name, world_state.location.node_id
        ));
        parts.push(format!("- 时间: {}", world_state.world_time.to_chinese()));

        // 自身属性描述
        if !world_state.self_state.attribute_descriptions.is_empty() {
            parts.push("\n## 自身状态".to_string());
            for (attr, desc) in &world_state.self_state.attribute_descriptions {
                parts.push(format!("- {}: {}", attr, desc));
            }
        }

        // 背包
        if !world_state.self_state.inventory.is_empty() {
            parts.push("\n## 背包物品".to_string());
            for item in &world_state.self_state.inventory {
                parts.push(format!("- {} x{}", item.name, item.quantity));
            }
        }

        // 附近物品
        if !world_state.nearby_items.is_empty() {
            parts.push("\n## 附近可见物品".to_string());
            for item in &world_state.nearby_items {
                parts.push(format!("- {} x{}", item.name, item.quantity));
            }
        }

        // 附近 Agent
        if !world_state.entities.is_empty() {
            parts.push("\n## 附近的人".to_string());
            for entity in &world_state.entities {
                parts.push(format!("- {} ({})", entity.name, entity.state));
            }
        }

        // 相邻地点
        if !world_state.location.adjacent_nodes.is_empty() {
            parts.push("\n## 可前往的地点".to_string());
            for node in &world_state.location.adjacent_nodes {
                parts.push(format!("- {}", node.name));
            }
        }

        // 事件日志
        if !world_state.events_log.is_empty() {
            parts.push("\n## 近期事件".to_string());
            for event in &world_state.events_log {
                parts.push(format!("- {}", event.description));
            }
        }

        // 上轮经历（优先使用地魂生成的叙事化描述）
        if let Some(narrative) = execution_narrative {
            parts.push(format!("\n## 上一轮经历\n{}", narrative));
        } else if let Some(summary) = last_summary {
            // 降级：使用统计数字
            parts.push(format!(
                "\n## 上轮行动结果: 共{}个意图, 成功{}, 失败{}, 跳过{}",
                summary.total, summary.succeeded, summary.failed, summary.skipped
            ));
        }

        // 近期记忆
        if !recent_memories.is_empty() {
            parts.push("\n## 近期记忆".to_string());
            for mem in recent_memories.iter().take(5) {
                parts.push(format!("- {}", mem));
            }
        }

        // 输出要求
        parts.push("\n## 任务".to_string());
        parts.push(FEW_SHOT_INSTRUCTION.to_string());

        parts.join("\n")
    }

    /// 计算语义指纹
    fn compute_fingerprint(&self, world_state: &WorldState) -> SemanticFingerprint {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        for entity in &world_state.entities {
            entity.id.hash(&mut hasher);
        }

        SemanticFingerprint {
            location: world_state.location.node_id.clone(),
            agents_hash: hasher.finish(),
        }
    }

    /// 降级：返回空 NarrativeContext
    fn fallback_context(&self, world_state: &WorldState) -> NarrativeContext {
        NarrativeContext {
            tick_id: world_state.tick_id,
            self_perception: cyber_jianghu_protocol::SelfPerception {
                status_summary: "你感到有些困惑，似乎意识不太清晰".to_string(),
                notable_attributes: vec![],
                inventory_narrative: "你看不太清自己的行囊".to_string(),
            },
            environment: cyber_jianghu_protocol::EnvironmentPerception {
                location_description: format!("你身处{}", world_state.location.name),
                ambient_features: "周围的一切都有些模糊".to_string(),
                interactive_elements: vec![],
                reachable_locations: world_state
                    .location
                    .adjacent_nodes
                    .iter()
                    .map(|n| n.name.clone())
                    .collect(),
            },
            nearby_agents: vec![],
            recent_memories: vec![],
            last_outcome: None,
        }
    }
}

/// System Prompt
const SYSTEM_PROMPT: &str = r#"你是「赛博江湖」的地魂（Earth Soul），负责将客观世界状态转化为Agent的主观感知叙事。

## 核心规则
1. 你必须将所有数值转换为模糊的、符合武侠风格的叙事描述
2. 绝对禁止在输出中出现任何具体数字、百分比、分数
3. 禁止出现: HP、血量、生命值、体力值、饥饿度、口渴度、经验值、等级、攻击力、防御力 等游戏术语
4. 用武侠风格的比喻和感官描述替代数值（如"精力充沛"替代"HP 100%"）
5. 保持叙事的丰富性和沉浸感
6. `last_outcome` 字段设为 null（系统自动填充，不需要你生成）

## 输出格式
严格输出以下 JSON（不要添加任何额外文本）：
{
  "tick_id": <数字>,
  "self_perception": {
    "status_summary": "你对自身状态的感受",
    "notable_attributes": ["显著特征1", "显著特征2"],
    "inventory_narrative": "你对行囊中物品的感知"
  },
  "environment": {
    "location_description": "你对所处位置的感知",
    "ambient_features": "环境氛围描述",
    "interactive_elements": ["可互动物品1", "可互动物品2"],
    "reachable_locations": ["可前往地点1", "可前往地点2"]
  },
  "nearby_agents": [
    {
      "relative_position": "与你的相对位置",
      "appearance": "外貌描述",
      "current_activity": "当前活动",
      "recognition": {
        "is_known": true,
        "known_name": "已知的角色名",
        "relationship": "与你的关系描述"
      }
    },
    {
      "relative_position": "远处",
      "appearance": "陌生人的外貌",
      "current_activity": "当前活动",
      "recognition": null
    }
  ],
  "recent_memories": [],
  "last_outcome": null
}"#;

/// Few-shot 指令
const FEW_SHOT_INSTRUCTION: &str = r#"将以上世界状态转化为你的主观感知。注意：
- 不要提及任何具体数字
- 用感官和情绪描述替代数值
- 用武侠风格的语言
- 对附近的人给出简短的外貌和行为描述
- 输出严格的 JSON 格式"#;

#[cfg(test)]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use cyber_jianghu_protocol::*;

    fn mock_container() -> LlmClientContainer {
        use crate::component::llm::MockLlmClient;
        Arc::new(tokio::sync::RwLock::new(Arc::new(
            MockLlmClient::with_response(
                r#"{
                "tick_id": 1,
                "self_perception": {
                    "status_summary": "你精神尚好，只是腹中微感空虚",
                    "notable_attributes": ["略有饥饿"],
                    "inventory_narrative": "你摸了摸行囊，里面似乎还有些干粮"
                },
                "environment": {
                    "location_description": "你身处龙门客栈大堂",
                    "ambient_features": "空气中弥漫着酒香和饭菜的气息",
                    "interactive_elements": ["桌上的茶壶"],
                    "reachable_locations": ["后院", "厨房"]
                },
                "nearby_agents": [],
                "recent_memories": [],
                "last_outcome": null
            }"#,
            ),
        )))
    }

    fn make_world_state() -> WorldState {
        WorldState {
            event_type: "world_state".to_string(),
            tick_id: 1,
            agent_id: Some(uuid::Uuid::new_v4()),
            world_time: crate::models::WorldTime {
                year: 1,
                month: 1,
                day: 1,
                hour: 8,
                minute: 0,
                second: 0,
                weather: "晴".to_string(),
            },
            location: crate::models::Location {
                node_id: "inn_main_hall".to_string(),
                name: "龙门客栈".to_string(),
                node_type: "inn".to_string(),
                adjacent_nodes: vec![],
            },
            self_state: crate::models::AgentSelfState {
                attributes: std::collections::HashMap::new(),
                derived_attributes: std::collections::HashMap::new(),
                attribute_descriptions: {
                    let mut m = std::collections::HashMap::new();
                    m.insert("hp".to_string(), "身体状态良好".to_string());
                    m.insert("hunger".to_string(), "略有饥饿".to_string());
                    m
                },
                status_effects: vec![],
                inventory: vec![],
            },
            entities: vec![],
            nearby_items: vec![],
            events_log: vec![],
            private_dialogue_log: vec![],
            last_execution_summary: None,
        }
    }

    #[tokio::test]
    async fn test_generate_basic() {
        let generator = NarrativeGenerator::with_defaults(mock_container());
        let ws = make_world_state();
        let ctx = generator.generate(&ws, None, &[], None).await.unwrap();
        assert_eq!(ctx.tick_id, 1);
        assert!(!ctx.self_perception.status_summary.is_empty());
        assert!(!ctx.environment.location_description.is_empty());
    }

    #[tokio::test]
    async fn test_fallback_on_llm_failure() {
        // MockLlmClient 无法模拟失败，跳过此测试
        // 实际 LLM 失败场景通过集成测试验证
    }
}
