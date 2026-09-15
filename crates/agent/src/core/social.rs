// ============================================================================
// 社交事件处理
// ============================================================================
//
// 处理 WorldEvent 中的社交事件，通过 LLM 评估好感度变化并更新关系存储。
// 异步非阻塞：spawn 独立任务处理 LLM 调用。
//
// 主观认知分歧设计（三层上下文注入）：
//   1. 身份——评估主体是谁（不同角色对同一行为感受不同）
//   2. 性格——显著人设特质（如高傲者受施舍可为负 delta）
//   3. 关系现状——仅注入本批涉及对象的好感度与最近事件，使增量路径依赖
//      （宿仇者送礼与挚友送礼意义不同）
// 有向边 A→B 与 B→A 的评估输入因此结构性不同，产生真实的认知不对称；
// 注入范围严格限定为涉及对象，避免全量图谱撑爆 prompt。
// ============================================================================

use crate::component::llm::LlmClientExt;
use crate::component::persona::Trait;
use crate::component::social::{RelationshipMemory, get_relationship_level};

/// 主观评估单批事件数上限：超出则分块调用，防止事件列表过长
const SOCIAL_EVAL_CHUNK_SIZE: usize = 10;

/// 已归因的社交事件（LLM 评估输入单元：对端 + 动作 + 描述）
struct ResolvedSocialEvent {
    other_id: uuid::Uuid,
    other_name: String,
    action: String,
    description: String,
    tick_id: i64,
}

impl super::Agent {
    /// 处理社交事件并更新关系存储
    ///
    /// 从 WorldEvent 列表中过滤 SocialInteraction 类型事件，
    /// 使用 LLM 以本角色主观视角评估好感度变化（-10 到 +10），
    /// 异步更新关系存储。
    pub fn process_social_events(
        &self,
        events: &[crate::models::WorldEvent],
        entities: &[crate::models::Entity],
    ) {
        let Some(ref store) = self.relationship_store else {
            return;
        };

        // 收集所有社交事件（物品转移 + 公开说话 + 密语）
        let social_events: Vec<crate::models::WorldEvent> = events
            .iter()
            .filter(|e| {
                matches!(
                    e.event_type,
                    crate::models::WorldEventType::SocialInteraction
                        | crate::models::WorldEventType::PublicMessage
                        | crate::models::WorldEventType::PrivateDialogue
                )
            })
            .cloned()
            .collect();

        if social_events.is_empty() {
            return;
        }

        // 构建名称查找表（UUID → 名称）
        let name_map: std::collections::HashMap<String, String> = entities
            .iter()
            .map(|e| (e.id.to_string(), e.name.clone()))
            .collect();

        // 主观上下文快照：身份 + 性格（评估主体差异的来源）
        let (self_name, self_desc, traits_summary) = self.persona.read(|p| {
            (
                p.name.clone(),
                p.base_description.clone(),
                format_trait_summary(&p.traits),
            )
        });

        let container = self.actor_llm_container.clone();
        let store = store.clone();

        // 非阻塞：spawn 独立任务处理 LLM 调用和关系更新
        tokio::spawn(async move {
            // 归因前置：先解析每事件的对端，prompt 才能标注 [对方：X]
            // 并按需注入关系现状。预加载所有已知关系名字，避免循环内逐个查 DB。
            let known_names: std::collections::HashMap<String, String> =
                match store.get_all_relationships() {
                    Ok(rels) => rels
                        .into_iter()
                        .filter(|r| !r.target_name.is_empty() && r.target_name != "陌生人")
                        .map(|r| (r.target_agent_id.to_string(), r.target_name))
                        .collect(),
                    Err(_) => std::collections::HashMap::new(),
                };

            let resolved: Vec<ResolvedSocialEvent> = social_events
                .iter()
                .filter_map(|event| resolve_social_event(event, &name_map, &known_names))
                .collect();

            if resolved.is_empty() {
                return;
            }

            // 调用 LLM 评估（无 LLM 容器则跳过，不写入 delta=0 的无意义记录）
            let Some(ref container) = container else {
                return;
            };

            for chunk in resolved.chunks(SOCIAL_EVAL_CHUNK_SIZE) {
                let status_lines = build_relationship_status_lines(&store, chunk);

                let event_lines: Vec<String> = chunk
                    .iter()
                    .enumerate()
                    .map(|(i, e)| format!("{}. [对方：{}] {}", i + 1, e.other_name, e.description))
                    .collect();

                let prompt = build_subjective_eval_prompt(
                    &self_name,
                    &self_desc,
                    &traits_summary,
                    &status_lines,
                    &event_lines,
                );

                let deltas: std::collections::HashMap<usize, i32> = {
                    let llm_client = container.read().await.clone();
                    let chat_config = crate::component::llm::ChatExchangeConfig {
                        model: llm_client.model_name(),
                        temperature: llm_client.temperature(),
                        max_tokens: None,
                        enable_thinking: None,
                    };
                    match llm_client
                        .complete_json_with_config_and_retry_extracted::<Vec<serde_json::Value>>(
                            &prompt,
                            chat_config,
                            2,
                        )
                        .await
                    {
                        Ok(extracted) => extracted
                            .value
                            .into_iter()
                            .filter_map(|v| {
                                let idx = v.get("index")?.as_u64()? as usize;
                                let delta = v.get("delta")?.as_i64()? as i32;
                                Some((idx, delta.clamp(-10, 10)))
                            })
                            .collect(),
                        Err(e) => {
                            tracing::warn!("社交事件 LLM 评估失败: {}", e);
                            continue; // 单块失败不拖垮其它块
                        }
                    }
                };

                for (i, ev) in chunk.iter().enumerate() {
                    let delta = deltas.get(&(i + 1)).copied().unwrap_or(0);
                    if let Err(e) = store.record_social_event(
                        ev.other_id,
                        &ev.other_name,
                        ev.tick_id,
                        &ev.action,
                        &ev.description,
                        delta,
                    ) {
                        tracing::warn!("社交事件关系更新失败: {}", e);
                    }
                }
            }
        });
    }

    /// 每 tick 关系名册同步：初遇登记 + 名称跟随
    ///
    /// 与 process_social_events 互补：后者只在真实社交事件后由 LLM 评估建档且失败无重试，
    /// 此处保证"同处一地即相识"，并让对方改名在下一认知 tick 自愈（幂等，本地 SQLite）。
    /// 实体的出现/位置变化为 Important 级 delta 信号，认知循环必然执行，
    /// 因此本调用不会因空转跳过而漏掉初遇。
    pub fn sync_relationship_roster(&self, entities: &[crate::models::Entity], tick_id: i64) {
        let Some(ref store) = self.relationship_store else {
            return;
        };
        let roster: Vec<(uuid::Uuid, String)> =
            entities.iter().map(|e| (e.id, e.name.clone())).collect();
        if let Err(e) = store.sync_roster(&roster, tick_id) {
            tracing::warn!("关系名册同步失败: {}", e);
        }
    }
}

/// 从事件 metadata 解析对端身份与动作类型
///
/// 语义与历史实现一致：予/trade_sell 取 target，receive/trade_buy/stolen_from
/// 取 from，说话类事件取 from_agent_id。名字解析优先级：当前在线实体 →
/// 已知关系存储 → "陌生人"。解析失败返回 None（事件跳过）。
fn resolve_social_event(
    event: &crate::models::WorldEvent,
    name_map: &std::collections::HashMap<String, String>,
    known_names: &std::collections::HashMap<String, String>,
) -> Option<ResolvedSocialEvent> {
    let meta = event.metadata.as_object()?;
    let action = meta.get("action").and_then(|v| v.as_str()).unwrap_or("");
    let is_speak = meta.contains_key("from_agent_id") && !meta.contains_key("action");
    let resolved_action = if is_speak { "speak" } else { action };
    let other_id_str = match resolved_action {
        "予" | "trade_sell" => meta.get("target").and_then(|v| v.as_str()),
        "receive" | "trade_buy" | "stolen_from" => meta.get("from").and_then(|v| v.as_str()),
        // PublicMessage/PrivateDialogue: from_agent_id 标识说话者
        _ => meta.get("from_agent_id").and_then(|v| v.as_str()),
    };

    let id_str = other_id_str?;
    let other_id = uuid::Uuid::parse_str(id_str).ok()?;
    let other_name = name_map
        .get(id_str)
        .cloned()
        .or_else(|| known_names.get(id_str).cloned())
        .unwrap_or_else(|| "陌生人".to_string());

    Some(ResolvedSocialEvent {
        other_id,
        other_name,
        action: resolved_action.to_string(),
        description: event.description.clone(),
        tick_id: event.tick_id,
    })
}

/// 格式化显著人设特质（取最偏离中性的 3 个）
///
/// 策略与 NarrativeGenerator::format_persona_traits 一致：value>=60 或 <=40 视为显著。
fn format_trait_summary(traits: &std::collections::HashMap<String, Trait>) -> String {
    let items: Vec<String> = traits
        .iter()
        .filter(|(_, t)| t.value() >= 60 || t.value() <= 40)
        .take(3)
        .map(|(name, t)| format!("{}({})", name, t.narrative_description()))
        .collect();

    if items.is_empty() {
        "性格平和".to_string()
    } else {
        items.join("、")
    }
}

/// 构建本块涉及对象的关系现状行（有界注入：每对象一行，仅本块涉及者）
fn build_relationship_status_lines(
    store: &crate::component::social::RelationshipStore,
    chunk: &[ResolvedSocialEvent],
) -> Vec<String> {
    let mut seen: Vec<uuid::Uuid> = Vec::new();
    let mut lines = Vec::new();
    for ev in chunk {
        if seen.contains(&ev.other_id) {
            continue;
        }
        seen.push(ev.other_id);
        let line = match store.get_relationship(ev.other_id) {
            Ok(Some(rel)) => format_relationship_status(&rel, &ev.other_name),
            _ => format!("- 你对{}：无交往记录", ev.other_name),
        };
        lines.push(line);
    }
    lines
}

/// 单条关系现状：好感度 + 等级 + 最近 2 次关键事件
///
/// key_events 存储顺序不稳定（内存 add_event 为升序，DB 加载为 tick DESC），
/// 此处按 tick_id 取最近 2 条，不依赖存储顺序。
fn format_relationship_status(rel: &RelationshipMemory, other_name: &str) -> String {
    let (_, label) = get_relationship_level(rel.favorability);
    let mut sorted: Vec<&crate::component::social::KeyEvent> = rel.key_events.iter().collect();
    sorted.sort_by_key(|e| std::cmp::Reverse(e.tick_id));
    let recent: Vec<String> = sorted
        .into_iter()
        .take(2)
        .map(|e| {
            let delta = if e.favorability_delta > 0 {
                format!("+{}", e.favorability_delta)
            } else {
                e.favorability_delta.to_string()
            };
            format!("{}({})", e.event_type, delta)
        })
        .collect();

    if recent.is_empty() {
        format!(
            "- 你对{}：好感度 {}（{}）",
            other_name, rel.favorability, label
        )
    } else {
        format!(
            "- 你对{}：好感度 {}（{}），最近：{}",
            other_name,
            rel.favorability,
            label,
            recent.join("、")
        )
    }
}

/// 构建主观评估 prompt（三层上下文：身份 + 性格 + 关系现状）
fn build_subjective_eval_prompt(
    self_name: &str,
    self_desc: &str,
    traits_summary: &str,
    status_lines: &[String],
    event_lines: &[String],
) -> String {
    // base_description 可能承载完整 system prompt，截断防撑爆
    let desc = if self_desc.chars().count() > 40 {
        format!("{}…", self_desc.chars().take(40).collect::<String>())
    } else {
        self_desc.to_string()
    };
    let identity = if desc.is_empty() {
        self_name.to_string()
    } else {
        format!("{}，{}", self_name, desc)
    };

    format!(
        r#"你是一个武侠世界角色的内心评估器。

你的身份：{identity}
你的人设特质：{traits}
你与相关之人的现状：
{status}

以你的身份、性格和与对方的现有关系评估每件事：同一件事发生在不同关系上感受不同（如宿仇者送礼可能是伪善，挚友小恩胜过陌生人重礼）。

返回 JSON 数组，每个元素包含 {{"index": 事件编号, "delta": 好感度变化(-10到+10的整数)}}
- 正数表示好感增加（如对方帮助、送礼）
- 负数表示好感降低（如被偷窃、被骗）
- 0 表示中性事件
- delta 体现你的主观感受，而非事件的客观善恶

事件列表:
{events}

只输出 JSON 数组，不要其他文字。"#,
        identity = identity,
        traits = traits_summary,
        status = status_lines.join("\n"),
        events = event_lines.join("\n"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::persona::TraitType;
    use crate::models::{WorldEvent, WorldEventType};

    fn make_event(tick_id: i64, description: &str, metadata: serde_json::Value) -> WorldEvent {
        WorldEvent {
            event_type: WorldEventType::SocialInteraction,
            tick_id,
            description: description.to_string(),
            metadata,
        }
    }

    fn empty_maps() -> (
        std::collections::HashMap<String, String>,
        std::collections::HashMap<String, String>,
    ) {
        (
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
        )
    }

    #[test]
    fn test_resolve_give_takes_target() {
        let (names, known) = empty_maps();
        let ev = make_event(
            5,
            "你给李四转移了 2 个馒头",
            serde_json::json!({"action": "予", "target": "11111111-1111-1111-1111-111111111111"}),
        );
        let r = resolve_social_event(&ev, &names, &known).unwrap();
        assert_eq!(r.action, "予");
        assert_eq!(r.other_name, "陌生人");
        assert_eq!(
            r.other_id.to_string(),
            "11111111-1111-1111-1111-111111111111"
        );
    }

    #[test]
    fn test_resolve_receive_takes_from() {
        let (names, known) = empty_maps();
        let ev = make_event(
            6,
            "张三给你转移了 2 个馒头",
            serde_json::json!({"action": "receive", "from": "22222222-2222-2222-2222-222222222222"}),
        );
        let r = resolve_social_event(&ev, &names, &known).unwrap();
        assert_eq!(r.action, "receive");
        assert_eq!(
            r.other_id.to_string(),
            "22222222-2222-2222-2222-222222222222"
        );
    }

    #[test]
    fn test_resolve_speak_takes_from_agent_id() {
        let mut names = std::collections::HashMap::new();
        names.insert(
            "33333333-3333-3333-3333-333333333333".to_string(),
            "王五".to_string(),
        );
        let (_, known) = empty_maps();
        let ev = make_event(
            7,
            "王五说：今夜风大",
            serde_json::json!({"from_agent_id": "33333333-3333-3333-3333-333333333333"}),
        );
        let r = resolve_social_event(&ev, &names, &known).unwrap();
        assert_eq!(r.action, "speak");
        assert_eq!(r.other_name, "王五"); // 在线实体优先于"陌生人"
    }

    #[test]
    fn test_resolve_skips_invalid_uuid() {
        let (names, known) = empty_maps();
        let ev = make_event(
            8,
            "无效事件",
            serde_json::json!({"action": "予", "target": "not-a-uuid"}),
        );
        assert!(resolve_social_event(&ev, &names, &known).is_none());
    }

    #[test]
    fn test_format_trait_summary_salient_and_empty() {
        let mut traits = std::collections::HashMap::new();
        traits.insert(
            "友善".to_string(),
            Trait::new("友善".to_string(), TraitType::Social, 80),
        );
        traits.insert(
            "睚眦必报".to_string(),
            Trait::new("睚眦必报".to_string(), TraitType::Moral, 15),
        );
        let summary = format_trait_summary(&traits);
        assert!(summary.contains("友善(友善较高)"));
        assert!(summary.contains("睚眦必报(睚眦必报很低)"));

        // 全中性 → fallback
        let mut neutral = std::collections::HashMap::new();
        neutral.insert(
            "平和".to_string(),
            Trait::new("平和".to_string(), TraitType::Social, 50),
        );
        assert_eq!(format_trait_summary(&neutral), "性格平和");
    }

    #[test]
    fn test_format_relationship_status_with_events() {
        let mut rel = RelationshipMemory::new(uuid::Uuid::new_v4(), "李四");
        rel.set_favorability(-35);
        rel.add_event(crate::component::social::KeyEvent::new(
            10, "攻击", "被打", -10,
        ));
        rel.add_event(crate::component::social::KeyEvent::new(
            11, "辱骂", "被骂", -5,
        ));
        let line = format_relationship_status(&rel, "李四");
        assert!(line.contains("好感度 -35"));
        assert!(line.contains("最近："));
        // key_events 最新在前：第一条应是 tick 11 的辱骂
        let recent_part = line.split("最近：").nth(1).unwrap();
        assert!(recent_part.starts_with("辱骂(-5)"));
    }

    #[test]
    fn test_build_subjective_eval_prompt_contains_three_layers() {
        let status = vec!["- 你对李四：好感度 -35（敌视），最近：辱骂(-5)".to_string()];
        let events = vec!["1. [对方：李四] 李四给了你 2 个馒头".to_string()];
        let prompt = build_subjective_eval_prompt(
            "张三",
            "你是一名行走江湖的冷面剑客。",
            "高傲(很高)、睚眦必报(很高)",
            &status,
            &events,
        );
        assert!(prompt.contains("你的身份：张三"));
        assert!(prompt.contains("你的人设特质：高傲(很高)"));
        assert!(prompt.contains("- 你对李四：好感度 -35"));
        assert!(prompt.contains("[对方：李四]"));
        assert!(prompt.contains("主观感受"));
    }

    #[test]
    fn test_build_subjective_eval_prompt_truncates_long_description() {
        let long_desc = "很".repeat(100);
        let prompt = build_subjective_eval_prompt(
            "张三",
            &long_desc,
            "性格平和",
            &[],
            &["1. [对方：李四] 某事件".to_string()],
        );
        // 40 字截断 + 省略号，身份行总长可控
        let identity_line = prompt
            .lines()
            .find(|l| l.starts_with("你的身份："))
            .unwrap();
        assert!(identity_line.ends_with('…'));
        assert!(identity_line.chars().count() < "你的身份：".len() + 45);
    }
}
