// ============================================================================
// Context Generation - 格式化上下文生成
// ============================================================================
//
// 设计原则：
// - 属性使用叙事化描述，不暴露原始数值
// - 叙事化规则从 narrative_config.json 加载
// - 外部系统（OpenClaw）通过 /api/v1/attributes 获取"梦中一瞥"数值

use crate::component::social::RelationshipStore;
use crate::infra::api::cognitive_context::load_available_actions_from_file;
use cyber_jianghu_protocol::WorldState;
use serde::Serialize;
use std::collections::HashMap;

/// Context response
#[derive(Serialize)]
pub struct ContextResponse {
    /// 格式化的 Markdown 上下文
    pub context: String,
    /// 当前 Tick ID
    pub tick_id: i64,
    /// Agent ID
    pub agent_id: String,
    /// 决策上下文 enrichment（CognitiveEngine 内部数据）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enrichment: Option<ContextEnrichment>,
}

/// 决策上下文补充数据（与 CognitiveEngine prompt 对齐）
#[derive(Serialize, Clone)]
pub struct ContextEnrichment {
    /// 完整 memory_context（三层记忆 + 生存/理智/延迟对话/托梦）
    pub memory_context: String,
    /// 行动历史滑窗
    pub summary_context: String,
    /// 行动结果学习
    pub outcome_section: String,
    /// 动作描述列表
    pub action_descriptions: String,
    /// 动作字段 schema
    pub action_field_hints: String,
    /// 上次执行结果
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_execution_result: Option<super::ExecutionSummary>,
}

/// "梦中一瞥"属性响应
#[derive(Serialize)]
pub struct AttributesGlimpse {
    /// 当前 Tick ID
    pub tick_id: i64,
    /// 属性列表（格式化显示）
    pub attributes: Vec<FormattedAttribute>,
    /// 原始属性数值（供程序使用）
    pub raw: HashMap<String, i32>,
    /// 派生属性原始数值（浮点数）
    pub derived_raw: HashMap<String, f32>,
    /// 警告：此数据不可记忆
    pub warning: String,
}

/// 格式化的属性
#[derive(Serialize)]
pub struct FormattedAttribute {
    /// 属性名称（原始键）
    pub name: String,
    /// 显示名称（中文）
    pub display_name: String,
    /// 格式化的值字符串
    pub value_str: String,
    /// 属性类别，由 attribute_categories 配置定义
    pub category: String,
}

/// 创建"梦中一瞥"属性响应
///
/// 格式说明：
/// - 显示格式：{display_name}: {value_str}
///
/// 数据驱动：属性分类来自 NarrativeConfig.attribute_categories（YAML 配置），
/// 显示名来自 attribute_descriptions（server 端 build_attribute_descriptions）。
/// 禁止在此方法内硬编码任何属性名或类别。
pub fn create_attributes_glimpse(
    state: &WorldState,
    narrative_config: Option<&cyber_jianghu_protocol::NarrativeConfig>,
) -> AttributesGlimpse {
    let raw: HashMap<String, i32> = state.self_state.attributes.clone();
    let derived_raw: HashMap<String, f32> = state.self_state.derived_attributes.clone();
    let descriptions = &state.self_state.attribute_descriptions;

    let attributes = if let Some(config) = narrative_config {
        config
            .build_attribute_views(&raw, &derived_raw, descriptions)
            .into_iter()
            .map(|v| FormattedAttribute {
                name: v.name,
                display_name: v.display_name,
                value_str: v.value_str,
                category: v.category,
            })
            .collect()
    } else {
        // 无 NarrativeConfig 时降级：全部标记为 unknown
        let mut formatted: Vec<FormattedAttribute> = raw
            .iter()
            .map(|(name, &value)| FormattedAttribute {
                name: name.clone(),
                display_name: descriptions
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| name.clone()),
                value_str: format!("{}", value),
                category: "unknown".to_string(),
            })
            .collect();
        for (name, &value) in &derived_raw {
            formatted.push(FormattedAttribute {
                name: name.clone(),
                display_name: descriptions
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| name.clone()),
                value_str: format!("{:.3}", value),
                category: "unknown".to_string(),
            });
        }
        formatted
    };

    AttributesGlimpse {
        tick_id: state.tick_id,
        attributes,
        raw,
        derived_raw,
        warning: "此数据为梦中一瞥，仅限当前决策周期使用。禁止存储到记忆系统。".to_string(),
    }
}

/// 从 AgentSelfState 中查找属性值
///
/// 同时检查 attributes（i32 基础属性）和 derived_attributes（f32 派生属性）。
/// 因为 attribute_descriptions 由 build_attribute_descriptions 注入，
/// 其中包含派生属性条目，迭代 descriptions 时 raw value 可能来自任一张 map。
fn lookup_attr_value(attr: &str, state: &cyber_jianghu_protocol::types::entities::AgentSelfState) -> String {
    state
        .attributes
        .get(attr)
        .map(|v| format!(" [当前值: {}]", v))
        .or_else(|| {
            state
                .derived_attributes
                .get(attr)
                .map(|v| format!(" [当前值: {:.3}]", v))
        })
        .unwrap_or_default()
}

/// 生成叙事化上下文
pub fn generate_context_markdown(
    state: &WorldState,
    relationship_store: &RelationshipStore,
    dream_thought: Option<&str>,
) -> String {
    generate_impl(state, Some(relationship_store), dream_thought)
}

/// 生成无关系存储的简化上下文
pub fn generate_context_markdown_no_relationship(
    state: &WorldState,
    dream_thought: Option<&str>,
) -> String {
    generate_impl(state, None, dream_thought)
}

/// 内部实现
///
/// 属性叙事描述使用 WorldState.attribute_descriptions（server 数据驱动）。
fn generate_impl(
    state: &WorldState,
    relationship_store: Option<&RelationshipStore>,
    dream_thought: Option<&str>,
) -> String {
    let mut sections: Vec<String> = Vec::new();

    // Header
    sections.push("# 游戏状态上下文".to_string());
    sections.push("".to_string());
    sections.push(format!("> 生成时间: Tick {}", state.tick_id));
    sections.push("".to_string());

    // 托梦
    if let Some(thought) = dream_thought {
        sections.push("## 托梦".to_string());
        sections.push("> 此念头在心中萦绕，挥之不去...".to_string());
        sections.push("".to_string());
        sections.push(format!("**{}**", thought));
        sections.push("".to_string());
    }

    // Tick & Agent
    sections.push("## 当前状态".to_string());
    sections.push(format!("- **Tick**: {}", state.tick_id));
    if let Some(agent_id) = &state.agent_id {
        sections.push(format!("- **Agent**: {}", agent_id));
    }

    // 位置
    sections.push("".to_string());
    sections.push("## 位置".to_string());
    sections.push(format!(
        "- **{}** ({})",
        state.location.name, state.location.node_type
    ));

    // 自身状态 - 使用 server 提供的 attribute_descriptions（数据驱动）
    sections.push("".to_string());
    sections.push("## 自身状态".to_string());

    let descriptions = &state.self_state.attribute_descriptions;
    let vital_attrs = ["hp", "hunger", "thirst", "stamina"];

    // 核心状态属性优先展示（选择哪些属性优先是展示层决策，不做数据驱动）
    for attr in &vital_attrs {
        if let Some(desc) = descriptions.get(*attr) {
            let raw = lookup_attr_value(attr, &state.self_state);
            sections.push(format!("- {}: {}{}", attr, desc, raw));
        }
    }

    // 其他属性（包括派生属性）—— 统一使用 attribute_descriptions + 双 map 查值
    for (name, desc) in descriptions {
        if vital_attrs.contains(&name.as_str()) {
            continue;
        }
        let raw = lookup_attr_value(name, &state.self_state);
        sections.push(format!("- {}: {}{}", name, desc, raw));
    }

    // 状态效果
    if !state.self_state.status_effects.is_empty() {
        sections.push("".to_string());
        sections.push(format!(
            "**特殊状态**: {}",
            state.self_state.status_effects.join("、")
        ));
    }

    // 背包
    if !state.self_state.inventory.is_empty() {
        sections.push("".to_string());
        sections.push("## 背包".to_string());
        for item in &state.self_state.inventory {
            let eq = if item.is_equipped { " [已装备]" } else { "" };
            sections.push(format!(
                "- {} [{}] x{}{}",
                item.name, item.item_id, item.quantity, eq
            ));
        }
    }

    // 附近实体
    sections.push("".to_string());
    sections.push("## 附近实体".to_string());
    if !state.entities.is_empty() {
        for entity in &state.entities {
            let dist = if entity.distance > 0 {
                format!(" ({}m)", entity.distance)
            } else {
                String::new()
            };

            let rel = match relationship_store {
                Some(store) => store
                    .get_relationship(entity.id)
                    .ok()
                    .flatten()
                    .map(|mem| {
                        if mem.self_description.is_empty() {
                            format!(" [好感度 {}]", mem.favorability)
                        } else {
                            format!(" [{}]", mem.self_description)
                        }
                    })
                    .unwrap_or_default(),
                None => String::new(),
            };
            sections.push(format!(
                "- **{}** `{}`{}{}",
                entity.name, entity.id, dist, rel
            ));
        }
    } else {
        sections.push("无".to_string());
    }

    // 地面物品
    sections.push("".to_string());
    sections.push("## 地面物品".to_string());
    if !state.nearby_items.is_empty() {
        for item in &state.nearby_items {
            sections.push(format!(
                "- {} [{}] x{}",
                item.name, item.item_id, item.quantity
            ));
        }
    } else {
        sections.push("无".to_string());
    }

    // 最近事件
    if !state.events_log.is_empty() {
        sections.push("".to_string());
        sections.push("## 最近事件".to_string());
        for event in state.events_log.iter().rev().take(20).rev() {
            sections.push(format!("- {}", event.description));
        }
    }

    // 可用动作（从本地文件加载）
    let available_actions = load_available_actions_from_file();
    if !available_actions.is_empty() {
        sections.push("".to_string());
        sections.push("## 可用动作".to_string());
        let actions: Vec<String> = available_actions
            .iter()
            .map(|a| format!("`{}`", a.action))
            .collect();
        sections.push(actions.join(", "));
    }

    sections.join("\n")
}
