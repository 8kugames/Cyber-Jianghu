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
                    let display_name = self.action_alias_map.read().unwrap().chinese_name(&action.action_type);
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

## 可做之事（参考）
{action_descriptions}

## 输出格式
严格输出以下 JSON（不要添加任何额外文本）：
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
}
