use super::super::{ActionExecutionResult, StateChange};
use super::super::{
    CraftData, MoveData, ObserveData, QuData, SpeakData, TeachData, YongData, YuData,
};
use crate::game_data::registry_or_error;
use crate::models::Intent;

pub(super) struct BasicActionExecutor;

impl BasicActionExecutor {
    /// 予：物品从 actor 向外流动
    /// recipient_type = "agent" → ItemTransferred
    /// recipient_type = "ground" → ItemDisposed
    /// 予是纯物理输出，不携带赠予/丢弃的社会语义
    pub(super) fn execute_yu(
        intent: &Intent,
        data: &YuData,
        current_location: &str,
    ) -> ActionExecutionResult {
        let rtype = data.recipient_type.as_str();

        match rtype {
            "agent" => {
                let target_id = match &data.recipient_id {
                    Some(id) => match uuid::Uuid::parse_str(id) {
                        Ok(uid) => uid,
                        Err(_) => {
                            return ActionExecutionResult::failure(
                                format!("无效的目标 ID: {}", id),
                                intent.action_type.to_string(),
                                Some(intent.intent_id),
                            );
                        }
                    },
                    None => {
                        return ActionExecutionResult::failure(
                            "予(agent) 需要 recipient_id".to_string(),
                            intent.action_type.to_string(),
                            Some(intent.intent_id),
                        );
                    }
                };

                let mut result = ActionExecutionResult::success(
                    format!("将 {} 个 {} 给予目标", data.quantity, data.item_id),
                    intent.action_type.to_string(),
                    Some(intent.intent_id),
                );
                result.add_change(StateChange::ItemTransferred {
                    from: intent.agent_id,
                    to: target_id,
                    item_id: data.item_id.clone(),
                    quantity: data.quantity,
                });
                result
            }
            "ground" => {
                let mut result = ActionExecutionResult::success(
                    format!("将 {} 个 {} 丢弃到地面", data.quantity, data.item_id),
                    intent.action_type.to_string(),
                    Some(intent.intent_id),
                );
                result.add_change(StateChange::ItemDisposed {
                    agent_id: intent.agent_id,
                    item_id: data.item_id.clone(),
                    quantity: data.quantity,
                    location: current_location.to_string(),
                });
                result
            }
            _ => ActionExecutionResult::failure(
                format!("无效的 recipient_type: {}（必须是 agent 或 ground）", rtype),
                intent.action_type.to_string(),
                Some(intent.intent_id),
            ),
        }
    }

    /// 取：物品从外部流入 actor
    /// source_type = "ground" → ItemAcquired(source=ground)
    /// source_type = "agent" → ItemTransferred (需授权判定)
    /// source_type = "resource" → ItemAcquired(source=resource)
    pub(super) fn execute_qu(
        intent: &Intent,
        data: &QuData,
        current_location: &str,
    ) -> ActionExecutionResult {
        let stype = data.source_type.as_str();

        match stype {
            "ground" => {
                let mut result = ActionExecutionResult::success(
                    format!("从地面拾取 {} 个 {}", data.quantity, data.item_id),
                    intent.action_type.to_string(),
                    Some(intent.intent_id),
                );
                result.add_change(StateChange::ItemAcquired {
                    agent_id: intent.agent_id,
                    item_id: data.item_id.clone(),
                    quantity: data.quantity,
                    source: "ground".to_string(),
                });
                result
            }
            "agent" => {
                let source_id = match &data.source_id {
                    Some(id) => match uuid::Uuid::parse_str(id) {
                        Ok(uid) => uid,
                        Err(_) => {
                            return ActionExecutionResult::failure(
                                format!("无效的来源 ID: {}", id),
                                intent.action_type.to_string(),
                                Some(intent.intent_id),
                            );
                        }
                    },
                    None => {
                        return ActionExecutionResult::failure(
                            "取(agent) 需要 source_id".to_string(),
                            intent.action_type.to_string(),
                            Some(intent.intent_id),
                        );
                    }
                };

                let mut result = ActionExecutionResult::success(
                    format!("从目标获取 {} 个 {}", data.quantity, data.item_id),
                    intent.action_type.to_string(),
                    Some(intent.intent_id),
                );
                result.add_change(StateChange::ItemTransferred {
                    from: source_id,
                    to: intent.agent_id,
                    item_id: data.item_id.clone(),
                    quantity: data.quantity,
                });
                result
            }
            "resource" => {
                let resource_id = data.source_id.as_deref().unwrap_or(&data.item_id);

                let registry = match registry_or_error() {
                    Ok(r) => r,
                    Err(e) => {
                        return ActionExecutionResult::failure(
                            format!("注册表未初始化: {}", e),
                            intent.action_type.to_string(),
                            Some(intent.intent_id),
                        );
                    }
                };
                let location_registry = registry.location_registry.read().expect("rwlock poisoned");

                let can_gather = location_registry
                    .get_node(current_location)
                    .map(|node| node.gatherable_items.contains(&resource_id.to_string()))
                    .unwrap_or(false);

                if !can_gather {
                    return ActionExecutionResult::failure(
                        format!("当前位置无法采集 {}", resource_id),
                        intent.action_type.to_string(),
                        Some(intent.intent_id),
                    );
                }

                let mut quantity = data.quantity;
                if let Some(season) =
                    crate::game_data::registry::TimeRegistry::get_current_season(intent.tick_id)
                {
                    quantity = (quantity as f32 * season.resource_growth_rate).floor() as i32;
                    if quantity < 1 {
                        quantity = 1;
                    }
                }

                let mut result = ActionExecutionResult::success(
                    format!("从 {} 采集了 {} 个", resource_id, quantity),
                    intent.action_type.to_string(),
                    Some(intent.intent_id),
                );
                result.add_change(StateChange::ItemAcquired {
                    agent_id: intent.agent_id,
                    item_id: data.item_id.clone(),
                    quantity,
                    source: "resource".to_string(),
                });
                result
            }
            _ => ActionExecutionResult::failure(
                format!(
                    "无效的 source_type: {}（必须是 ground/agent/resource）",
                    stype
                ),
                intent.action_type.to_string(),
                Some(intent.intent_id),
            ),
        }
    }

    /// 用：消耗或激活物品
    /// 不做语义过滤——物品效果由 item 定义中的 effects 决定
    pub(super) fn execute_yong(intent: &Intent, data: &YongData) -> ActionExecutionResult {
        let item = match crate::items::get_item_definition(&data.item_id) {
            Some(item) => item,
            None => {
                return ActionExecutionResult::failure(
                    format!("物品不存在: {}", data.item_id),
                    intent.action_type.to_string(),
                    Some(intent.intent_id),
                );
            }
        };

        if !item.is_usable() {
            return ActionExecutionResult::failure(
                format!("{} 不可使用", item.name),
                intent.action_type.to_string(),
                Some(intent.intent_id),
            );
        }

        let effects: Vec<super::super::ItemEffect> = item
            .effects
            .iter()
            .filter_map(|effect| {
                effect.value_as_i32().map(|v| super::super::ItemEffect {
                    attribute: effect.attribute.clone(),
                    operation: effect.operation,
                    value: v,
                })
            })
            .collect();

        let mut result = ActionExecutionResult::success(
            format!("使用了 {}", item.name),
            intent.action_type.to_string(),
            Some(intent.intent_id),
        );

        result.add_change(StateChange::ItemUsed {
            agent_id: intent.agent_id,
            item_id: data.item_id.clone(),
            effects,
        });

        result
    }

    /// 说话：统一通信
    /// channel = "public" → 本地广播（默认）
    /// channel = "private" → 私密会话（Dialogue Session）
    /// channel = "broadcast" → 大范围广播
    pub(super) fn execute_speak(intent: &Intent, data: &SpeakData) -> ActionExecutionResult {
        let channel = data.channel.as_str();

        if channel == "private" && data.target_agent_id.is_none() {
            return ActionExecutionResult::failure(
                "channel=private 时需要指定 target_agent_id".to_string(),
                intent.action_type.to_string(),
                Some(intent.intent_id),
            );
        }

        let mut result = ActionExecutionResult::success(
            format!("{}: {}", intent.agent_id, data.content),
            intent.action_type.to_string(),
            Some(intent.intent_id),
        );

        result.add_change(StateChange::MessageSpoken {
            agent_id: intent.agent_id,
            content: data.content.clone(),
            channel: data.channel.clone(),
            target_agent_id: data.target_agent_id,
            already_broadcast: intent.already_broadcast,
        });

        result
    }

    /// 移动
    pub(super) fn execute_move(
        intent: &Intent,
        data: &MoveData,
        current_location: &str,
    ) -> ActionExecutionResult {
        let registry = match registry_or_error() {
            Ok(r) => r,
            Err(e) => {
                return ActionExecutionResult::failure(
                    format!("注册表未初始化: {}", e),
                    intent.action_type.to_string(),
                    Some(intent.intent_id),
                );
            }
        };

        let location_registry = registry.location_registry.read().expect("rwlock poisoned");

        if !location_registry.node_exists(&data.target_location) {
            return ActionExecutionResult::failure(
                format!("目标位置不存在: {}", data.target_location),
                intent.action_type.to_string(),
                Some(intent.intent_id),
            );
        }

        if !location_registry.is_connected(current_location, &data.target_location) {
            return ActionExecutionResult::failure(
                format!(
                    "无法从 {} 移动到 {}（位置不相邻）",
                    current_location, data.target_location
                ),
                intent.action_type.to_string(),
                Some(intent.intent_id),
            );
        }

        let travel_cost = location_registry
            .get_travel_cost(current_location, &data.target_location)
            .unwrap_or(1);
        let stamina_multiplier = registry
            .get()
            .game_rules
            .data
            .agent_state
            .location
            .travel_stamina_multiplier;
        let stamina_cost = travel_cost as i32 * stamina_multiplier;

        let mut result = ActionExecutionResult::success(
            format!(
                "从 {} 移动到 {}，消耗 {} 体力",
                current_location, data.target_location, stamina_cost
            ),
            intent.action_type.to_string(),
            Some(intent.intent_id),
        );

        result.add_change(StateChange::StaminaChanged {
            agent_id: intent.agent_id,
            delta: -stamina_cost,
        });

        result.add_change(StateChange::LocationChanged {
            agent_id: intent.agent_id,
            old_location: current_location.to_string(),
            new_location: data.target_location.clone(),
        });

        result
    }

    /// 观察
    pub(super) fn execute_observe(
        intent: &Intent,
        data: &ObserveData,
        all_states: &[crate::models::AgentState],
    ) -> ActionExecutionResult {
        match &data.target_agent_id {
            Some(target_str) => {
                let target_id = match target_str.parse::<uuid::Uuid>() {
                    Ok(id) => id,
                    Err(_) => {
                        return ActionExecutionResult::failure(
                            format!("无效的目标 ID: {}", target_str),
                            intent.action_type.to_string(),
                            Some(intent.intent_id),
                        );
                    }
                };

                let target_state = all_states.iter().find(|s| s.agent_id == target_id);
                match target_state {
                    Some(state) => {
                        let formula_context = state.get_formula_context();
                        let hp_formula = state
                            .status
                            .collection
                            .attributes
                            .get("hp")
                            .map(|a| &a.metadata.max_value_formula);
                        let hp_max = match hp_formula {
                            Some(formula) => crate::game_data::types::status_component::StatusComponent::evaluate_max_value(
                                formula,
                                crate::game_data::types::status_component::DEFAULT_STATUS_MAX_VALUE,
                                &formula_context,
                            ) as i32,
                            None => crate::game_data::types::status_component::DEFAULT_STATUS_MAX_VALUE as i32,
                        } + state.status.max_modifiers.get("hp").copied().unwrap_or(0);
                        let hp_pct =
                            state.status.get("hp").unwrap_or(0) as f64 / hp_max.max(1) as f64;
                        let alive_status = if state.is_alive {
                            "存活"
                        } else {
                            "已死亡"
                        };
                        let hp_desc = if !state.is_alive {
                            "气息全无".to_string()
                        } else if hp_pct > 0.8 {
                            "神完气足".to_string()
                        } else if hp_pct > 0.5 {
                            "状态尚可".to_string()
                        } else if hp_pct > 0.3 {
                            "面色不佳".to_string()
                        } else if hp_pct > 0.1 {
                            "伤痕累累".to_string()
                        } else {
                            "奄奄一息".to_string()
                        };

                        let mut desc = format!(
                            "你注视着{}，见其{}，{}。",
                            state.name, hp_desc, alive_status
                        );

                        let observer_has_stealth = all_states
                            .iter()
                            .find(|s| s.agent_id == intent.agent_id)
                            .map(|observer| {
                                observer
                                    .skills
                                    .iter()
                                    .any(|s| s.contains("stealth") || s.contains("潜行"))
                            })
                            .unwrap_or(false);
                        let detected = !observer_has_stealth;

                        if detected {
                            desc.push_str(&format!(" {}似乎察觉到了你的目光。", state.name));
                        } else {
                            desc.push_str(&format!(" {}对此毫无察觉。", state.name));
                        }

                        let mut result = ActionExecutionResult::success(
                            desc.clone(),
                            intent.action_type.to_string(),
                            Some(intent.intent_id),
                        );
                        result.add_change(StateChange::Observation {
                            observer_id: intent.agent_id,
                            target_id: Some(target_id),
                            description: desc,
                            detected,
                        });
                        result
                    }
                    None => ActionExecutionResult::failure(
                        format!("目标 {} 不存在或不在当前区域", target_str),
                        intent.action_type.to_string(),
                        Some(intent.intent_id),
                    ),
                }
            }
            None => {
                let observer_node = all_states
                    .iter()
                    .find(|me| me.agent_id == intent.agent_id)
                    .map(|me| me.node_id.as_str())
                    .unwrap_or("");

                let same_node_agents: Vec<&str> = all_states
                    .iter()
                    .filter(|s| {
                        s.agent_id != intent.agent_id && s.node_id == observer_node && s.is_alive
                    })
                    .map(|s| s.name.as_str())
                    .collect();

                let (gatherable_info, location_name, hazard_info) = match registry_or_error() {
                    Ok(reg) => {
                        let loc_reg = reg.location_registry.read().expect("rwlock poisoned");
                        match loc_reg.get_node(observer_node) {
                            Some(node) => {
                                let items = if node.gatherable_items.is_empty() {
                                    String::new()
                                } else {
                                    format!("自然资源有：{}", node.gatherable_items.join("、"))
                                };
                                let hazard = node
                                    .environmental_damage
                                    .filter(|d| *d > 0)
                                    .map(|d| format!("此地环境恶劣，持续造成 {} 点伤害", d))
                                    .unwrap_or_default();
                                (items, node.name.clone(), hazard)
                            }
                            None => (String::new(), String::new(), String::new()),
                        }
                    }
                    Err(_) => (String::new(), String::new(), String::new()),
                };

                let mut env_parts = Vec::new();
                if !location_name.is_empty() {
                    env_parts.push(format!("你位于{}", location_name));
                }
                if same_node_agents.is_empty() {
                    env_parts.push("此地似乎只有你一人".to_string());
                } else {
                    let names = same_node_agents.join("、");
                    env_parts.push(format!("在场的有 {}", names));
                }
                if !gatherable_info.is_empty() {
                    env_parts.push(gatherable_info);
                }
                if !hazard_info.is_empty() {
                    env_parts.push(hazard_info);
                }

                let env_desc = env_parts.join("。") + "。";

                let mut result = ActionExecutionResult::success(
                    env_desc.clone(),
                    intent.action_type.to_string(),
                    Some(intent.intent_id),
                );
                result.add_change(StateChange::Observation {
                    observer_id: intent.agent_id,
                    target_id: None,
                    description: env_desc,
                    detected: false,
                });
                result
            }
        }
    }

    /// 休整：时间流逝 + 效果恢复
    pub(super) fn execute_halt(intent: &Intent) -> ActionExecutionResult {
        ActionExecutionResult::success(
            format!("Agent {} 静心休整", intent.agent_id),
            intent.action_type.to_string(),
            Some(intent.intent_id),
        )
    }

    /// 制造
    pub(super) fn execute_craft(intent: &Intent, data: &CraftData) -> ActionExecutionResult {
        let recipe = match crate::game_data::registry::RecipeRegistry::get(&data.recipe_id) {
            Some(r) => r,
            None => {
                return ActionExecutionResult::failure(
                    format!("配方不存在: {}", data.recipe_id),
                    intent.action_type.to_string(),
                    Some(intent.intent_id),
                );
            }
        };

        let mut result = ActionExecutionResult::success(
            format!("制造了 {}", recipe.name),
            intent.action_type.to_string(),
            Some(intent.intent_id),
        );

        result.add_change(StateChange::ItemCrafted {
            agent_id: intent.agent_id,
            item_id: recipe.result_item.clone(),
            quantity: recipe.result_quantity,
        });

        result
    }

    /// 教导
    pub(super) fn execute_teach(intent: &Intent, data: &TeachData) -> ActionExecutionResult {
        let student_id = match uuid::Uuid::parse_str(&data.target_agent_id) {
            Ok(id) => id,
            Err(_) => {
                return ActionExecutionResult::failure(
                    format!("无效的目标 ID: {}", data.target_agent_id),
                    intent.action_type.to_string(),
                    Some(intent.intent_id),
                );
            }
        };

        let recipe_name = crate::game_data::registry::RecipeRegistry::get(&data.recipe_id)
            .map(|r| r.name)
            .unwrap_or_else(|| data.recipe_id.clone());

        let mut result = ActionExecutionResult::success(
            format!("传授配方「{}」", recipe_name),
            intent.action_type.to_string(),
            Some(intent.intent_id),
        );

        result.add_change(StateChange::RecipeLearned {
            agent_id: student_id,
            recipe_id: data.recipe_id.clone(),
            source: "taught".to_string(),
        });

        let stamina_cost = crate::game_data::registry()
            .map(|cache| {
                cache
                    .get()
                    .game_rules
                    .data
                    .recipe_learning
                    .teach_stamina_cost
            })
            .unwrap_or_else(|| {
                crate::game_data::types::unified_config::RecipeLearningConfig::default()
                    .teach_stamina_cost
            });
        result.add_change(StateChange::StaminaChanged {
            agent_id: intent.agent_id,
            delta: -stamina_cost,
        });

        result
    }
}
