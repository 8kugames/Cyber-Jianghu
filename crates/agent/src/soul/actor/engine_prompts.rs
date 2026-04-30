// ============================================================================
// 认知引擎 Prompt 构建方法
// ============================================================================
//
// 从 CognitiveEngine 中拆出的 prompt 相关方法：
// - build_direct_prompt: 直连 WorldState 的 prompt 构建
// - build_world_state_section: WorldState 数据段
// - build_hardcoded_prompt: 内置硬编码 prompt（向后兼容）
// - build_action_descriptions / build_action_field_hints: 动作描述和字段提示
// ============================================================================

use cyber_jianghu_protocol::AvailableAction;

impl super::CognitiveEngine {
    /// 构建直连 WorldState 的 prompt（包含精确数据）
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

        // 从 WorldState 构建精确数据段
        let world_state_section = self.build_world_state_section(world_state);

        // 构建技能行为指令（progressive disclosure：tool-calling 时只输出索引）
        let skill_instructions =
            self.build_skill_instructions(&world_state.self_state.skills, use_tool_calling);

        // 地魂引导：tool-calling 时替换输出格式指令，消除 "严格输出 JSON" 与 "先调用工具" 的矛盾
        let tool_calling_guidance = if use_tool_calling {
            "## 决策流程\n你必须按以下顺序完成决策：\n1. 先调用 skill_view 工具查看你掌握的技能行为指引\n2. 如需了解人际关系，调用 get_relationship 或 list_relationships\n3. 如需搜索记忆，调用 search_memory\n4. 工具调用完成后，按以下 JSON 格式输出决策\n".to_string()
        } else {
            "## 输出格式\n严格输出以下 JSON（不要添加任何额外文本）：\n".to_string()
        };

        // 尝试使用模板配置
        if let Some(ref template_config) = self.prompt_template
            && let Some(tmpl) = template_config.get_template("actor_direct")
        {
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

            return tmpl.render_all(&vars);
        }

        // 模板不可用时的内置模板（向后兼容旧部署）
        self.build_hardcoded_prompt(
            &feedback_section,
            agent_name,
            persona_desc,
            &world_state_section,
            &memory_section,
            &summary_context,
            &outcome_section,
            &action_descriptions,
            &action_field_hints,
            &tool_calling_guidance,
        )
    }

    /// 构建 WorldState 数据段（共享逻辑，模板和硬编码路径共用）
    fn build_world_state_section(
        &self,
        world_state: &cyber_jianghu_protocol::WorldState,
    ) -> String {
        let content_hint_len = self
            .prompt_template
            .as_ref()
            .and_then(|t| t.templates.get("actor_direct"))
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

        // 自身属性描述（叙事化）
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

        // 背包物品（精确 item_id）
        if !world_state.self_state.inventory.is_empty() {
            ws_parts.push("\n## 背包物品".to_string());
            for item in &world_state.self_state.inventory {
                ws_parts.push(format!(
                    "- {} ({}) x{}",
                    item.item_id, item.name, item.quantity
                ));
            }
        }

        // 附近物品（精确 item_id）
        if !world_state.nearby_items.is_empty() {
            ws_parts.push("\n## 附近可见物品".to_string());
            for item in &world_state.nearby_items {
                ws_parts.push(format!(
                    "- {} ({}) x{}",
                    item.item_id, item.name, item.quantity
                ));
            }
        }

        // 附近 Agent（精确 UUID + 近期动作）
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

        // 当前位置 + 可前往地点（强化地点约束）
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

        // 可采集资源
        if !world_state.location.gatherable_items.is_empty() {
            ws_parts.push("\n## 当前位置可采集的资源".to_string());
            for item in &world_state.location.gatherable_items {
                ws_parts.push(format!("- {} ({})", item.name, item.item_id));
            }
        }

        // 事件日志
        if !world_state.events_log.is_empty() {
            ws_parts.push("\n## 近期事件".to_string());
            for event in &world_state.events_log {
                ws_parts.push(format!("- {}", event.description));
            }
        }

        ws_parts.join("\n")
    }

    /// 内置硬编码 prompt（向后兼容旧部署）
    #[allow(clippy::too_many_arguments)]
    fn build_hardcoded_prompt(
        &self,
        feedback_section: &str,
        agent_name: &str,
        persona_desc: &str,
        world_state_section: &str,
        memory_section: &str,
        summary_context: &str,
        outcome_section: &str,
        action_descriptions: &str,
        action_field_hints: &str,
        tool_calling_guidance: &str,
    ) -> String {
        format!(
            r#"{feedback_section}你是 {agent_name}。
{persona}

## 当前世界状态
{world_state_section}
{memory_section}
{summary_context}
{outcome_section}
## 任务
基于你的性格和当前状态，做出决策。你直接输出结构化 Intent，包含精确的 ID。

## 生存法则
- 当饥饿或口渴描述中出现紧迫措辞时（如"饥肠辘辘/饥饿难耐/急需"等），进食/饮水是最高优先级
- 没有食物时：先拾取地上的食物/水（pickup），再进食/饮水（eat/drink）
- 背包和地面都没有时：采集（gather）或移动到可能有资源的地点（move）
- idle（原地休息）是合法行为，不必强求每个 tick 都行动
- eat/drink 的 action_data 必须使用"背包物品"或"附近物品"中列出的精确 item_id，禁止使用物品名称或自创 ID

## 叙事限制
- 叙事只能引用"背包物品"或"附近可见物品"中确实存在的物品
- 不得描述其他角色的行为，除非"附近的人"中有该角色的近期动作记录
- 不得与不在"附近的人"列表中的角色互动
- **世界地图仅由"可前往的地点"定义。不存在其他地点。不得在 thought_process 或 environment 中提及未列出的地点**
- 不得编造未发生的事件（如劫镖、打斗、天灾），除非"近期事件"中有记录
- 如果对某事没有观察证据，thought_process 中应标注[未确认]
- **观察分级**："附近的人"中 说话/私语/大喊 的引号内容仅代表该角色说的话，不代表事实发生。他人可能在说谎、吹嘘或编造。只有非语言动作（攻击/给予/偷窃/移动/拾取 等）的执行结果才是可靠的行为观察
- thought_process 中区分"直接观察"与"听闻"：基于他人言语推断的内容必须标注[听闻]，不得当作确凿事实

## 交易规则
- 没有系统强制的交易动作。交易通过 speak（说话/私语）议价，双方各自用 give（给予）完成交割
- 交易流程：先用 speak 与对方协商价格 → 双方同意后，卖方 give 物品给买方，买方 give 银两给卖方
- **先给的人承担对方不给的风险**。这是江湖的规矩，信错人就要付出代价
- 也可以用多次 speak 建立信任后再交易，或要求对方先付定金
- 价格完全由双方自行决定，没有公定价。可以参考采集成本、稀缺程度、个人需求

## 可做之事（参考）
{action_descriptions}

{tool_calling_guidance}
{{
  "self_status": "你的状态简述 (30字以内)",
  "environment": "环境描述 (30字以内)",
  "key_observations": ["观察1", "观察2"],
  "primary_drive": "当前主要驱动力",
  "drive_intensity": 5,
  "thought_process": "完整思考过程 (200字以内)",
  "actions": [
    {{"action_type": "动作类型", "action_data": {{}}}}
  ]
}}

### actions 规则：
- actions 数组包含 1-3 个按顺序执行的动作
- 大多数情况只需要 1 个动作
- 只有需要连续操作时才输出多个（如：先 pickup 再 eat，先 move 再 gather）
- 后续动作依赖前一个动作的成功（如 pickup 失败则 eat 不会执行）
- 每个动作的 action_data 中的 ID 必须从上面的世界状态数据中直接复制，不要编造

### action_data 字段要求：
{action_field_hints}"#,
            agent_name = agent_name,
            persona = persona_desc,
            world_state_section = world_state_section,
            memory_section = memory_section,
            summary_context = summary_context,
            feedback_section = feedback_section,
            action_descriptions = action_descriptions,
            action_field_hints = action_field_hints,
            tool_calling_guidance = tool_calling_guidance,
        )
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
                            // 从 field_aliases 找中文名，否则用英文名
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

    /// 旧式 prompt 构建（不接收 WorldState，降级路径用）
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
        let action_list = cache.get_actions_list().to_string();
        drop(cache);

        format!(
            r#"{feedback_section}你是 {agent_name}。
{persona}

## 当前游戏状态 (Tick {tick_id})
{memory_section}
{summary_context}
## 任务
基于你的性格和当前状态，做出决策。

## 生存法则
- 饥饿或口渴严重时，进食/饮水是最高优先级
- 没有食物时：先拾取地上的食物/水
- idle（原地休息）是合法行为

## 可做之事（参考）
{action_list}

## 输出格式
严格输出以下 JSON：
{{
  "self_status": "你的状态简述 (30字以内)",
  "environment": "环境描述 (30字以内)",
  "key_observations": ["观察1", "观察2"],
  "primary_drive": "当前主要驱动力",
  "drive_intensity": 5,
  "thought_process": "完整思考过程 (200字以内)",
  "actions": [
    {{"action_type": "动作类型", "action_data": {{}}}}
  ]
}}
- actions 数组 1-3 个，按顺序执行，后续依赖前一个成功"#,
            tick_id = tick_id,
            agent_name = agent_name,
            persona = persona_desc,
            memory_section = memory_section,
            summary_context = summary_context,
            feedback_section = feedback_section,
            action_list = action_list,
        )
    }

    /// 从本地配置目录加载技能行为指令（带缓存）
    ///
    /// 查找路径: $CYBER_JIANGHU_CONFIG_DIR/skills/{skill_id}/SKILL.md
    /// 目录不存在时 warn + 返回空字符串，不 panic。
    /// 缓存在 self.skill_cache 中，避免每 tick 重复 IO。
    ///
    /// `index_only=true` 时输出技能索引（progressive disclosure，配合地魂 tool-calling）。
    /// `index_only=false` 时输出完整 body（非 tool-calling 降级路径）。
    fn build_skill_instructions(
        &self,
        skills: &[cyber_jianghu_protocol::types::entities::SkillInfo],
        index_only: bool,
    ) -> String {
        if skills.is_empty() {
            return String::new();
        }

        // 使用引擎统一解析的配置目录（单一数据源）
        let config_dir = self.config_dir.clone();
        if config_dir.as_os_str().is_empty() {
            tracing::warn!("配置目录为空，跳过技能指令加载");
            return String::new();
        }

        let skills_dir = config_dir.join("skills");

        // 始终填充缓存（地魂 tool-calling 需要缓存数据）
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
            // Progressive disclosure：不列出技能名，强制 LLM 调用 skill_view 获取详情
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

            // 从 ToolDefinition 动态构建工具描述（单一数据源，无硬编码）
            for tool in super::super::earth::EarthToolExecutor::tool_definitions() {
                lines.push(format!(
                    "- {}: {}",
                    tool.function.name, tool.function.description
                ));
            }
            return lines.join("\n");
        }

        // 降级路径：注入完整 body
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
            .as_ref()
            .and_then(|t| t.templates.get("actor_direct"))
            .and_then(|tmpl| tmpl.sections.get(section_name))
            .map(|s| s.trim().to_string())
    }
}
