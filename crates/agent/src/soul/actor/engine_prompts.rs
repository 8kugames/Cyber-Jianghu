// ============================================================================
// 认知引擎 Prompt 构建方法
// ============================================================================
//
// 所有 prompt 模板统一由 prompt_templates.json 定义（本地加载或 WS ConfigUpdate 下发）。
// 本文件仅负责：
// - 组装变量 → 调用模板渲染
// - 构建 WorldState 数据段（动态数据，不适合放模板）
// - 构建动作描述/字段提示
// ============================================================================

use crate::component::attention::FocusSummary;
use cyber_jianghu_protocol::{ActionEffectInfo, ActionRequirementInfo, AvailableAction};

/// Lean prompt 构建参数（避免函数签名过长）
pub(super) struct LeanPromptParams<'a> {
    pub world_state: &'a cyber_jianghu_protocol::WorldState,
    pub memory_context: &'a str,
    pub validation_feedback: Option<&'a str>,
    pub persona_desc: &'a str,
    pub agent_name: &'a str,
    pub focus_summary: Option<&'a FocusSummary>,
    pub critical_preload: Option<&'a str>,
    pub use_tool_calling: bool,
}

// ============================================================================
// Prompt Section Token 估算 (Phase 0a Instrumentation)
// ============================================================================

/// Prompt 各 section 的字符数 / 4 粗估 token 数
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
    /// 字符数 / 4 粗估 tokens
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
    /// 构建直连 WorldState 的 prompt（包含精确数据）
    ///
    /// 单一数据源：prompt_templates.json（本地加载或 WS ConfigUpdate 下发）。
    /// 模板必须包含 actor_direct，否则返回 Err。
    pub(super) fn build_direct_prompt(
        &self,
        world_state: &cyber_jianghu_protocol::WorldState,
        memory_context: &str,
        validation_feedback: Option<&str>,
        persona_desc: &str,
        agent_name: &str,
        use_tool_calling: bool,
    ) -> anyhow::Result<String> {
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

        let cache = self.prompt_cache.read().unwrap();
        let action_descriptions = cache.get_action_descriptions().to_string();
        let action_field_hints = cache.get_action_field_hints().to_string();
        drop(cache);

        let world_state_section = self.build_world_state_section(world_state);

        let skill_instructions =
            self.build_skill_instructions(&world_state.self_state.skills, use_tool_calling);

        let tool_calling_guidance = if use_tool_calling {
            "## 输出格式\n直接输出以下 JSON。工具调用是可选的——大多数情况下你不需要调用任何工具，直接根据已有信息决策即可。\n仅在确实需要查阅技能行为指引时才调用 skill_view，或需要确认人际关系/搜索记忆时才调用对应工具。\n你最多可以调用 1 次工具，调用后必须立即输出 JSON。\n".to_string()
        } else {
            "## 输出格式\n严格输出以下 JSON（不要添加任何额外文本）：\n".to_string()
        };

        let prompt_template = self.prompt_template();

        let tmpl = prompt_template.get_template("actor_direct")
            .ok_or_else(|| anyhow::anyhow!(
                "actor_direct 模板未加载 — 本地 prompt_templates.json 未找到或 WS ConfigUpdate 尚未到达"
            ))?;

        // Phase 0a: prompt section token 估算（字符数 / 4，在 move 之前计算）
        let estimate = PromptSectionEstimate {
            system: PromptSectionEstimate::estimate_tokens(tool_calling_guidance.len()),
            persona: PromptSectionEstimate::estimate_tokens(persona_desc.len()),
            world_state: PromptSectionEstimate::estimate_tokens(world_state_section.len()),
            action_descriptions: PromptSectionEstimate::estimate_tokens(
                action_descriptions.len() + action_field_hints.len(),
            ),
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
        vars.insert("dialogue_section".to_string(), String::new()); // 对话已在memory_context中，此处传空避免模板{dialogue_section}字面输出
        vars.insert("summary_context".to_string(), summary_context);
        vars.insert("action_descriptions".to_string(), action_descriptions);
        vars.insert("action_field_hints".to_string(), action_field_hints);
        vars.insert("outcome_section".to_string(), outcome_section);
        vars.insert("skill_instructions".to_string(), skill_instructions);
        vars.insert("tool_calling_guidance".to_string(), tool_calling_guidance);
        tracing::info!(
            "[prompt-section-estimate] total~{}tokens | persona={} world_state={} actions={} memory={} skills={} other={}",
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

    /// 构建 WorldState 数据段（动态数据，不适合放 YAML）
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

    /// 从动作列表构建描述文本（含 cost/effect 摘要）
    pub(super) fn build_action_descriptions(actions: &[AvailableAction]) -> String {
        if actions.is_empty() {
            return "- 休息: 休息".to_string();
        }

        actions
            .iter()
            .map(|a| {
                let display_name = if a.name.is_empty() {
                    a.action.clone()
                } else {
                    a.name.clone()
                };
                let desc = if a.description.is_empty() {
                    display_name.clone()
                } else {
                    a.description.clone()
                };
                let meta = render_action_meta(&a.requirements, &a.effects);
                if meta.is_empty() {
                    format!("- {}: {}", display_name, desc)
                } else {
                    format!("- {}: {} [{}]", display_name, desc, meta)
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 从动作列表构建字段 schema（用中文字段名显示）
    pub(super) fn build_action_field_hints(actions: &[AvailableAction]) -> String {
        if actions.is_empty() {
            return "- 休息: (无额外参数)".to_string();
        }

        actions
            .iter()
            .map(|a| {
                let display_name = if a.name.is_empty() {
                    a.action.clone()
                } else {
                    a.name.clone()
                };
                let fields_hint = if a.required_fields.is_empty() {
                    "(无额外参数)".to_string()
                } else {
                    let fields_str = a
                        .required_fields
                        .iter()
                        .map(|f| {
                            let cn = a.field_aliases.get(f).and_then(|v| v.first()).unwrap_or(f);
                            format!("\"{}\": ...", cn)
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("(动作数据: {{ {} }})", fields_str)
                };
                format!("- {}: {}", display_name, fields_hint)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 旧式 prompt 构建（Claw 模式降级路径，不接收 WorldState）
    ///
    /// 同样走模板渲染，用空 world_state_section 占位。
    pub(super) fn build_legacy_prompt(
        &self,
        tick_id: i64,
        memory_context: &str,
        validation_feedback: Option<&str>,
        persona_desc: &str,
        agent_name: &str,
    ) -> anyhow::Result<String> {
        let feedback_section = match validation_feedback {
            Some(fb) => format!("\n[验证反馈]: {}\n", fb),
            None => String::new(),
        };

        let memory_section = if memory_context.is_empty() {
            String::new()
        } else {
            format!("\n### 当前状态与感知\n{memory_context}\n")
        };

        let summary_context = self.get_summary_context();

        let cache = self.prompt_cache.read().unwrap();
        let action_descriptions = cache.get_action_descriptions().to_string();
        let action_field_hints = cache.get_action_field_hints().to_string();
        drop(cache);

        let prompt_template = self.prompt_template();
        let world_state_section = format!("- Tick: {} (旧式模式，无 WorldState)", tick_id);

        let tmpl = prompt_template.get_template("actor_direct")
            .ok_or_else(|| anyhow::anyhow!(
                "actor_direct 模板未加载 — 本地 prompt_templates.json 未找到或 WS ConfigUpdate 尚未到达"
            ))?;

        let mut vars = std::collections::HashMap::new();
        vars.insert("feedback_section".to_string(), feedback_section);
        vars.insert("agent_name".to_string(), agent_name.to_string());
        vars.insert("persona".to_string(), persona_desc.to_string());
        vars.insert("world_state_section".to_string(), world_state_section);
        vars.insert("memory_section".to_string(), memory_section);
        vars.insert("summary_context".to_string(), summary_context);
        vars.insert("action_descriptions".to_string(), action_descriptions);
        vars.insert("action_field_hints".to_string(), action_field_hints);
        vars.insert("outcome_section".to_string(), String::new());
        vars.insert("skill_instructions".to_string(), String::new());
        vars.insert(
            "tool_calling_guidance".to_string(),
            "## 输出格式\n严格输出以下 JSON（不要添加任何额外文本）：\n".to_string(),
        );

        Ok(tmpl.render_all(&vars))
    }

    /// 从 skill_cache 构建技能行为指令
    ///
    /// skill_cache 来源：
    /// 1. 启动时从 skill_cache.json 加载
    /// 2. 运行时从 Server ConfigUpdate 推送
    fn build_skill_instructions(
        &self,
        skills: &[cyber_jianghu_protocol::types::entities::SkillInfo],
        index_only: bool,
    ) -> String {
        if skills.is_empty() {
            return String::new();
        }

        if index_only {
            let header = self
                .render_template_section("skill_index_header")
                .unwrap_or_else(|| {
                    format!(
                        "## 已掌握技能（共 {} 项，可选：调用 skill_view 查看行为指引）",
                        skills.len()
                    )
                });

            let mut lines = vec![header];
            lines.push(String::new());

            let tool_header = self
                .render_template_section("tool_hints_header")
                .unwrap_or_else(|| "## 可用工具（可选，仅在需要时调用）".to_string());
            lines.push(tool_header);

            for tool in super::super::earth::EarthToolExecutor::tool_definitions() {
                lines.push(format!(
                    "- {}: {}",
                    tool.function.name, tool.function.description
                ));
            }
            return lines.join("\n");
        }

        let full_header = self
            .render_template_section("skill_full_header")
            .unwrap_or_else(|| "## 已掌握技能行为准则".to_string());

        let cache = self.skill_cache.read().unwrap();
        let mut instructions = Vec::new();
        for skill in skills {
            if let Some(body) = cache.get(&skill.skill_id) {
                instructions.push(format!("### {} ({})\n{}", skill.name, skill.skill_id, body));
            }
        }
        if instructions.is_empty() {
            return String::new();
        }
        format!("{}\n{}", full_header, instructions.join("\n\n"))
    }

    /// 渲染模板中的非 required section（progressive disclosure 用）
    fn render_template_section(&self, section_name: &str) -> Option<String> {
        self.prompt_template()
            .get_template("actor_direct")
            .and_then(|tmpl| tmpl.sections.get(section_name))
            .map(|s| s.trim().to_string())
    }

    // ========================================================================
    // Lean Prompt (Task 8: Token 优化)
    // ========================================================================

    /// 构建 Lean Prompt（Token 优化模式）
    ///
    /// Focus Summary 替代完整 WorldState，Action Index 替代完整描述，
    /// Skill Index 替代完整指令。LLM 通过 tool calling 按需获取详情。
    pub(super) fn build_lean_direct_prompt(
        &self,
        params: LeanPromptParams<'_>,
    ) -> anyhow::Result<String> {
        let LeanPromptParams {
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

        // Lean: Focus Summary 替代完整 WorldState
        let world_state_section = match focus_summary {
            Some(fs) => {
                let mut narrative = format!("### 焦点状态\n{}\n", fs.narrative);
                // Task 9: 追加 Critical preload 数据
                if let Some(preload) = critical_preload {
                    narrative.push_str(preload);
                }
                narrative
            }
            None => self.build_world_state_section(world_state), // 无 summary 时降级到完整模式
        };

        // Lean: Action Index 替代完整描述
        let actions = self.available_actions.read().unwrap();
        let action_descriptions = Self::build_action_index(&actions);
        drop(actions);
        let action_field_hints = String::new(); // Lean 模式不需要，通过 tool 查询

        // Lean: Skill Index 替代完整指令
        let skill_instructions = {
            let cache = self.skill_cache.read().unwrap();
            Self::build_skill_index_from_cache(&cache)
        };

        let tool_calling_guidance = if use_tool_calling {
            "## 输出格式\n直接输出以下 JSON。工具调用是可选的——根据焦点状态中的提示，在需要查询详细信息时调用对应工具。\n你最多可以调用 2 次工具，调用后必须立即输出 JSON。\n".to_string()
        } else {
            "## 输出格式\n严格输出以下 JSON（不要添加任何额外文本）：\n".to_string()
        };

        let prompt_template = self.prompt_template();

        let tmpl = prompt_template.get_template("actor_direct")
            .ok_or_else(|| anyhow::anyhow!(
                "actor_direct 模板未加载 — 本地 prompt_templates.json 未找到或 WS ConfigUpdate 尚未到达"
            ))?;

        // Phase 0a: prompt section token 估算
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
        vars.insert("action_field_hints".to_string(), action_field_hints);
        vars.insert("outcome_section".to_string(), outcome_section);
        vars.insert("skill_instructions".to_string(), skill_instructions);
        vars.insert("tool_calling_guidance".to_string(), tool_calling_guidance);
        tracing::info!(
            "[prompt-section-estimate][lean] total~{}tokens | persona={} world_state={} actions={} memory={} skills={} other={}",
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

    /// 构建 Action Index（名称 + 描述，无字段提示）
    ///
    /// Lean 模式下替代完整 action_descriptions + action_field_hints。
    /// LLM 通过 get_action_detail(action_name) 工具按需获取详情。
    fn build_action_index(actions: &[AvailableAction]) -> String {
        if actions.is_empty() {
            return "- 休息: 休息 (查询详情: get_action_detail(\"休息\"))\n".to_string();
        }
        let mut s = String::from("## 可用动作 (查询详情: get_action_detail(action_name))\n\n");
        for action in actions {
            let display_name = if action.name.is_empty() {
                action.action.clone()
            } else {
                action.name.clone()
            };
            let desc = if action.description.is_empty() {
                display_name.clone()
            } else {
                action.description.clone()
            };
            s.push_str(&format!("- {}: {}\n", display_name, desc));
        }
        s
    }

    /// 构建 Skill Index（仅名称，从 skill_cache）
    ///
    /// Lean 模式下替代完整 skill_instructions。
    /// LLM 通过 skill_view(skill_id) 工具按需获取行为指引。
    fn build_skill_index_from_cache(
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

// ============================================================================
// Action cost/effect 通用渲染（纯数据驱动，零硬编码）
// ============================================================================

/// 将 requirements + effects 渲染为单行摘要
///
/// 格式示例: "消耗qi 2, thirst+2"
/// 未知 requirement_type/effect_type 跳过（通用适配）
fn render_action_meta(
    requirements: &[ActionRequirementInfo],
    effects: &[ActionEffectInfo],
) -> String {
    let mut parts = Vec::new();

    for req in requirements {
        if let Some(s) = render_requirement(req) {
            parts.push(s);
        }
    }

    for eff in effects {
        if let Some(s) = render_effect(eff) {
            parts.push(s);
        }
    }

    parts.join(", ")
}

fn render_requirement(req: &ActionRequirementInfo) -> Option<String> {
    match req.requirement_type.as_str() {
        "attribute" => {
            let attr = display_attr(&req.params)?;
            let cost = req.params.get("cost").and_then(|v| v.as_i64()).unwrap_or(0);
            if cost > 0 {
                Some(format!("消耗{}{}", attr, cost))
            } else {
                None
            }
        }
        "item" => {
            let item = req.params.get("item_id")?.as_str()?;
            let qty = req
                .params
                .get("quantity")
                .and_then(|v| v.as_i64())
                .unwrap_or(1);
            Some(format!("需要{}x{}", item, qty))
        }
        _ => None,
    }
}

fn render_effect(eff: &ActionEffectInfo) -> Option<String> {
    match eff.effect_type.as_str() {
        "attribute_change" => {
            let attr = display_attr(&eff.params)?;
            let op = eff
                .params
                .get("operation")
                .and_then(|v| v.as_str())
                .unwrap_or("add");
            let val = eff.params.get("value")?.as_i64()?;
            let formatted = match op {
                "add" if val > 0 => format!("{}+{}", attr, val),
                "add" if val < 0 => format!("{}{}", attr, val),
                "add" => return None,
                "set" => format!("{}={}", attr, val),
                _ => format!("{}{}{}", attr, op, val),
            };
            Some(formatted)
        }
        "add_item" => {
            let item = eff.params.get("item_id")?.as_str()?;
            let qty = eff
                .params
                .get("quantity")
                .and_then(|v| v.as_i64())
                .unwrap_or(1);
            Some(format!("获得{}x{}", item, qty))
        }
        "remove_item" => {
            let item = eff.params.get("item_id")?.as_str()?;
            let qty = eff
                .params
                .get("quantity")
                .and_then(|v| v.as_i64())
                .unwrap_or(1);
            Some(format!("消耗{}x{}", item, qty))
        }
        _ => None,
    }
}

/// 优先使用 display_attribute（Server 注入的中文显示名），fallback 到 attribute
fn display_attr(params: &std::collections::HashMap<String, serde_json::Value>) -> Option<String> {
    params
        .get("display_attribute")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            params
                .get("attribute")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
}
