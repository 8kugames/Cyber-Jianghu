// ============================================================================
// 认知引擎 Prompt 构建
// ============================================================================
//
// 单一入口: build_prompt()
// 策略: FocusSummary 替代完整 WorldState, name-only Action Index, Skill Index。
// LLM 通过地魂 tool calling (get_action_detail / query_world / skill_view) 按需获取详情。
// 模板由 prompt_templates.json 定义（本地加载或 WS ConfigUpdate 下发）。

use crate::component::attention::FocusSummary;
use cyber_jianghu_protocol::AvailableAction;

/// 构造空 WorldState（降级路径用，如 legacy think_with_memory_and_feedback）
///
/// WorldState 无 Default derive，且未来字段增减时此函数会编译报错（结构性保证）。
pub(super) fn empty_world_state() -> cyber_jianghu_protocol::WorldState {
    use cyber_jianghu_protocol::*;
    WorldState {
        event_type: "world_state".to_string(),
        tick_id: 0,
        agent_id: None,
        world_time: WorldTime {
            year: 1, month: 1, day: 1, hour: 0, minute: 0, second: 0,
            weather: "晴".to_string(),
        },
        location: Location {
            node_id: String::new(),
            name: "未知".to_string(),
            node_type: String::new(),
            adjacent_nodes: vec![],
            gatherable_items: vec![],
        },
        self_state: AgentSelfState {
            attributes: Default::default(),
            derived_attributes: Default::default(),
            attribute_descriptions: Default::default(),
            status_effects: vec![],
            inventory: vec![],
            skills: vec![],
            recipe_details: vec![],
            age_years: None,
            max_age: None,
        },
        entities: vec![],
        nearby_items: vec![],
        events_log: vec![],
        private_dialogue_log: vec![],
        last_execution_summary: None,
        lessons_learned: vec![],
    }
}

/// Prompt 构建参数
pub(super) struct PromptParams<'a> {
    pub world_state: &'a cyber_jianghu_protocol::WorldState,
    pub memory_context: &'a str,
    pub validation_feedback: Option<&'a str>,
    pub persona_desc: &'a str,
    pub agent_name: &'a str,
    pub focus_summary: Option<&'a FocusSummary>,
    pub critical_preload: Option<&'a str>,
    pub use_tool_calling: bool,
}

/// Prompt 各 section token 估算
#[derive(Debug, Clone, Default)]
pub struct PromptSectionEstimate {
    pub system: usize,
    pub persona: usize,
    pub world_state: usize,
    pub action_descriptions: usize,
    pub memory: usize,
    pub skill_instructions: usize,
    pub other: usize,
}

impl PromptSectionEstimate {
    fn estimate_tokens(chars: usize) -> usize {
        chars / 4
    }

    fn total_tokens(&self) -> usize {
        self.system
            + self.persona
            + self.world_state
            + self.action_descriptions
            + self.memory
            + self.skill_instructions
            + self.other
    }
}

impl super::CognitiveEngine {
    /// 唯一 prompt 构建入口
    pub(super) fn build_prompt(&self, params: PromptParams<'_>) -> anyhow::Result<String> {
        let PromptParams {
            world_state,
            memory_context,
            validation_feedback,
            persona_desc,
            agent_name,
            focus_summary,
            critical_preload,
            use_tool_calling,
        } = params;

        let feedback_section = match validation_feedback {
            Some(fb) => format!("\n[验证反馈]: {}\n", fb),
            None => String::new(),
        };

        let memory_section = if memory_context.is_empty() {
            String::new()
        } else {
            format!("\n### 记忆上下文\n{memory_context}\n")
        };

        let summary_context = self.get_summary_context();
        let outcome_section = self.get_outcome_context();

        // FocusSummary 替代完整 WorldState，无 summary 时降级到完整模式
        let world_state_section = match focus_summary {
            Some(fs) => {
                let mut narrative = format!("### 焦点状态\n{}\n", fs.narrative);
                if let Some(preload) = critical_preload {
                    narrative.push_str(preload);
                }
                narrative
            }
            None => self.build_world_state_section(world_state),
        };

        // Action Index: name-only，详情通过 get_action_detail 按需查询
        let actions = self.available_actions.read().unwrap();
        let action_descriptions = Self::build_action_index_pub(&actions);
        drop(actions);

        // Skill Index: name-only，详情通过 skill_view 按需查询
        let skill_instructions = {
            let cache = self.skill_cache.read().unwrap();
            Self::build_skill_index(&cache)
        };

        let tool_calling_guidance = if use_tool_calling {
            // prompt 声明次数 < max_tool_rounds（留出 Warn→Terminate 余量）
            format!(
                "## 输出格式\n\
                直接输出以下 JSON。工具调用是可选的——根据焦点状态中的提示，在需要查询详细信息时调用对应工具。\n\
                你最多可以调用 2 次工具，调用后必须立即输出 JSON。\n\n\
                重要：工具（query_world/get_action_detail/list_skills/skill_view）是查询信息的手段，不是动作。\
                action_type 只能填\"可用动作\"列表中的名称（说话、移动、进食等），绝对不能填工具名称。\n"
            )
        } else {
            "## 输出格式\n严格输出以下 JSON（不要添加任何额外文本）：\n".to_string()
        };

        let prompt_template = self.prompt_template();
        let tmpl = prompt_template.get_template("actor_direct")
            .ok_or_else(|| anyhow::anyhow!(
                "actor_direct 模板未加载 — 本地 prompt_templates.json 未找到或 WS ConfigUpdate 尚未到达"
            ))?;

        let estimate = PromptSectionEstimate {
            system: PromptSectionEstimate::estimate_tokens(tool_calling_guidance.len()),
            persona: PromptSectionEstimate::estimate_tokens(persona_desc.len()),
            world_state: PromptSectionEstimate::estimate_tokens(world_state_section.len()),
            action_descriptions: PromptSectionEstimate::estimate_tokens(action_descriptions.len()),
            memory: PromptSectionEstimate::estimate_tokens(memory_section.len()),
            skill_instructions: PromptSectionEstimate::estimate_tokens(skill_instructions.len()),
            other: PromptSectionEstimate::estimate_tokens(
                feedback_section.len()
                    + summary_context.len()
                    + outcome_section.len()
                    + agent_name.len(),
            ),
        };

        let mut vars = std::collections::HashMap::new();
        vars.insert("feedback_section".to_string(), feedback_section);
        vars.insert("agent_name".to_string(), agent_name.to_string());
        vars.insert("persona".to_string(), persona_desc.to_string());
        vars.insert("world_state_section".to_string(), world_state_section);
        vars.insert("memory_section".to_string(), memory_section);
        vars.insert("dialogue_section".to_string(), String::new());
        vars.insert("summary_context".to_string(), summary_context);
        vars.insert("action_descriptions".to_string(), action_descriptions);
        vars.insert("action_field_hints".to_string(), String::new());
        vars.insert("outcome_section".to_string(), outcome_section);
        vars.insert("skill_instructions".to_string(), skill_instructions);
        vars.insert("tool_calling_guidance".to_string(), tool_calling_guidance);

        tracing::info!(
            "[prompt-estimate] total~{}tokens | persona={} world_state={} actions={} memory={} skills={} other={}",
            estimate.total_tokens(),
            estimate.persona,
            estimate.world_state,
            estimate.action_descriptions,
            estimate.memory,
            estimate.skill_instructions,
            estimate.other
        );

        Ok(tmpl.render_all(&vars))
    }

    /// 构建 WorldState 完整数据段（无 FocusSummary 时的降级路径）
    fn build_world_state_section(
        &self,
        world_state: &cyber_jianghu_protocol::WorldState,
    ) -> String {
        let content_hint_len = self
            .prompt_template()
            .get_template("actor_direct")
            .and_then(|t| t.truncation.get("content_hint"))
            .copied()
            .unwrap_or(30);

        let mut ws_parts = Vec::new();

        ws_parts.push(format!("- Tick: {}", world_state.tick_id));
        ws_parts.push(format!(
            "- 位置: {} ({})",
            world_state.location.name, world_state.location.node_id
        ));
        ws_parts.push(format!("- 时间: {}", world_state.world_time.to_chinese()));

        if !world_state.self_state.attribute_descriptions.is_empty() {
            ws_parts.push("\n## 自身状态".to_string());
            for (attr, desc) in &world_state.self_state.attribute_descriptions {
                let raw = world_state
                    .self_state
                    .attributes
                    .get(attr)
                    .map(|v| format!(" [当前值: {}]", v))
                    .unwrap_or_default();
                ws_parts.push(format!("- {}: {}{}", attr, desc, raw));
            }
        }

        if !world_state.self_state.inventory.is_empty() {
            ws_parts.push("\n## 背包物品".to_string());
            for item in &world_state.self_state.inventory {
                ws_parts.push(format!(
                    "- {} ({}) x{}",
                    item.item_id, item.name, item.quantity
                ));
            }
        }

        if !world_state.nearby_items.is_empty() {
            ws_parts.push("\n## 附近可见物品".to_string());
            for item in &world_state.nearby_items {
                ws_parts.push(format!(
                    "- {} ({}) x{}",
                    item.item_id, item.name, item.quantity
                ));
            }
        }

        if !world_state.entities.is_empty() {
            ws_parts.push("\n## 附近的人".to_string());
            for entity in &world_state.entities {
                ws_parts.push(format!("- {} (UUID: {})", entity.name, entity.id));
                for action in &entity.recent_actions {
                    let content_hint = action
                        .content
                        .as_ref()
                        .map(|c| {
                            let truncated: String = c.chars().take(content_hint_len).collect();
                            format!("「{}」", truncated)
                        })
                        .unwrap_or_default();
                    let display_name = &action.action_type;
                    ws_parts.push(format!(
                        "  [Tick {}] {} {}{}",
                        action.tick_id, display_name, action.result, content_hint
                    ));
                }
            }
        }

        ws_parts.push(format!(
            "\n## 当前位置：{} ({})",
            world_state.location.name, world_state.location.node_id
        ));
        if !world_state.location.adjacent_nodes.is_empty() {
            ws_parts.push("## 可前往的地点（仅这些地点存在）".to_string());
            for node in &world_state.location.adjacent_nodes {
                ws_parts.push(format!(
                    "- {} ({})，移动消耗：{} tick",
                    node.name, node.node_id, node.travel_cost
                ));
            }
        }

        if !world_state.location.gatherable_items.is_empty() {
            ws_parts.push("\n## 当前位置可采集的资源".to_string());
            for item in &world_state.location.gatherable_items {
                ws_parts.push(format!("- {} ({})", item.name, item.item_id));
            }
        }

        if !world_state.events_log.is_empty() {
            ws_parts.push("\n## 近期事件".to_string());
            for event in &world_state.events_log {
                ws_parts.push(format!("- {}", event.description));
            }
        }

        if !world_state.self_state.recipe_details.is_empty() {
            ws_parts.push("\n## 已知配方".to_string());
            for recipe in &world_state.self_state.recipe_details {
                ws_parts.push(format!(
                    "- {}（ID: {}）→ {}x{}",
                    recipe.name, recipe.recipe_id, recipe.result_item_name, recipe.result_quantity
                ));
            }
            ws_parts.push("使用 view_recipe_detail 工具查看配方详细材料要求。".to_string());
        }

        ws_parts.join("\n")
    }

    /// Action Index: 仅名称，通过 get_action_detail 按需查询详情
    pub(super) fn build_action_index_pub(actions: &[AvailableAction]) -> String {
        if actions.is_empty() {
            return "- 休息 (查询详情: get_action_detail(\"休息\"))\n".to_string();
        }
        let mut s = String::from(
            "## 可用动作 (查询详情: get_action_detail(action_name))\n\
             以下是你能执行的动作。action_type 必须是下面的名称，不能是工具名。\n\
             需要了解某动作的具体字段和效果时，调用 get_action_detail。\n\n",
        );
        for action in actions {
            let display_name = if action.name.is_empty() {
                action.action.clone()
            } else {
                action.name.clone()
            };
            s.push_str(&format!("- {}\n", display_name));
        }
        s
    }

    /// Skill Index: 仅名称，通过 skill_view 按需查询
    fn build_skill_index(
        skill_cache: &std::collections::HashMap<String, String>,
    ) -> String {
        if skill_cache.is_empty() {
            return String::new();
        }
        let mut s = String::from("## 已掌握技能 (查询详情: skill_view(skill_id))\n\n");
        for id in skill_cache.keys() {
            s.push_str(&format!("- {}\n", id));
        }
        s
    }
}
