// ============================================================================
// 认知引擎 Prompt 构建方法
// ============================================================================
//
// 所有 prompt 模板统一由 prompt_templates.yaml 定义，本文件仅负责：
// - 组装变量 → 调用模板渲染
// - 构建 WorldState 数据段（动态数据，不适合放 YAML）
// - 构建动作描述/字段提示
// ============================================================================

use cyber_jianghu_protocol::AvailableAction;

impl super::CognitiveEngine {
    /// 构建直连 WorldState 的 prompt（包含精确数据）
    ///
    /// 单一数据源：prompt_templates.yaml。模板加载失败在启动时 panic。
    pub(super) fn build_direct_prompt(
        &self,
        world_state: &cyber_jianghu_protocol::WorldState,
        memory_context: &str,
        validation_feedback: Option<&str>,
        persona_desc: &str,
        agent_name: &str,
        use_tool_calling: bool,
    ) -> String {
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
            "## 决策流程\n你必须按以下顺序完成决策：\n1. 先调用 skill_view 工具查看你掌握的技能行为指引\n2. 如需了解人际关系，调用 get_relationship 或 list_relationships\n3. 如需搜索记忆，调用 search_memory\n4. 工具调用完成后，按以下 JSON 格式输出决策\n".to_string()
        } else {
            "## 输出格式\n严格输出以下 JSON（不要添加任何额外文本）：\n".to_string()
        };

        let tmpl = self
            .prompt_template
            .get_template("actor_direct")
            .expect("prompt_templates.yaml 必须包含 actor_direct 模板");

        let mut vars = std::collections::HashMap::new();
        vars.insert("feedback_section".to_string(), feedback_section);
        vars.insert("agent_name".to_string(), agent_name.to_string());
        vars.insert("persona".to_string(), persona_desc.to_string());
        vars.insert("world_state_section".to_string(), world_state_section);
        vars.insert("memory_section".to_string(), memory_section);
        vars.insert("summary_context".to_string(), summary_context);
        vars.insert("action_descriptions".to_string(), action_descriptions);
        vars.insert("action_field_hints".to_string(), action_field_hints);
        vars.insert("outcome_section".to_string(), outcome_section);
        vars.insert("skill_instructions".to_string(), skill_instructions);
        vars.insert("tool_calling_guidance".to_string(), tool_calling_guidance);

        tmpl.render_all(&vars)
    }

    /// 构建 WorldState 数据段（动态数据，不适合放 YAML）
    fn build_world_state_section(
        &self,
        world_state: &cyber_jianghu_protocol::WorldState,
    ) -> String {
        let content_hint_len = self
            .prompt_template
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

        ws_parts.join("\n")
    }

    /// 从动作列表构建描述文本
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
                format!("- {}: {}", display_name, desc)
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
    ) -> String {
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

        let tmpl = self
            .prompt_template
            .get_template("actor_direct")
            .expect("prompt_templates.yaml 必须包含 actor_direct 模板");

        let world_state_section = format!("- Tick: {} (旧式模式，无 WorldState)", tick_id);

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

        tmpl.render_all(&vars)
    }

    /// 从本地配置目录加载技能行为指令（带缓存）
    fn build_skill_instructions(
        &self,
        skills: &[cyber_jianghu_protocol::types::entities::SkillInfo],
        index_only: bool,
    ) -> String {
        if skills.is_empty() {
            return String::new();
        }

        let config_dir = self.config_dir.clone();
        if config_dir.as_os_str().is_empty() {
            tracing::warn!("配置目录为空，跳过技能指令加载");
            return String::new();
        }

        let skills_dir = config_dir.join("skills");

        if skills_dir.exists() {
            let mut cache = self.skill_cache.write().unwrap();
            for skill in skills {
                if cache.contains_key(&skill.skill_id) {
                    continue;
                }
                let skill_path = skills_dir.join(&skill.skill_id).join("SKILL.md");
                if let Ok(content) = std::fs::read_to_string(&skill_path) {
                    let body = super::super::earth::extract_skill_body(&content);
                    if !body.is_empty() {
                        cache.insert(skill.skill_id.clone(), body);
                    }
                }
            }
        }

        if index_only {
            let header = self
                .render_template_section("skill_index_header")
                .unwrap_or_else(|| {
                    format!(
                        "## 已掌握技能（共 {} 项，使用 skill_view 工具查看详情和行为指引）",
                        skills.len()
                    )
                });

            let mut lines = vec![header];
            lines.push(String::new());

            let tool_header = self
                .render_template_section("tool_hints_header")
                .unwrap_or_else(|| "## 可用工具\n调用以下工具获取决策所需信息：".to_string());
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
        self.prompt_template
            .get_template("actor_direct")
            .and_then(|tmpl| tmpl.sections.get(section_name))
            .map(|s| s.trim().to_string())
    }
}
