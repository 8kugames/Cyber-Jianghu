// ============================================================================
// Context Generation - 格式化上下文生成
// ============================================================================
//
// 设计原则：
// - 属性使用叙事化描述，不暴露原始数值
// - 叙事化规则从 narrative_config.json 加载
// - 外部系统（OpenClaw）通过 /api/v1/attributes 获取"梦中一瞥"数值

use crate::ai::cognitive::narrative::{NarrativeConfig, NarrativeEngine};
use crate::ai::relationship::RelationshipStore;
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
    /// 属性类别：primary（先天）, status（状态）, derived（派生）
    pub category: String,
}

/// 创建"梦中一瞥"属性响应
///
/// 格式说明：
/// - 显示格式：{display_name}: {value_str}
/// - 先天属性（growable）：{当前} ({上限})
/// - 状态值：{当前}/{最大}
/// - 派生属性：{计算值}
pub fn create_attributes_glimpse(
    state: &WorldState,
    engine: &NarrativeEngine,
) -> AttributesGlimpse {
    let mut formatted = Vec::new();
    let raw: HashMap<String, i32> = state.self_state.attributes.clone();

    // 定义属性类别（基于名称前缀或具体名称）
    let status_attrs = [
        "hp",
        "stamina",
        "hunger",
        "thirst",
        "qi",
        "sanity",
        "reputation",
    ];
    let primary_attrs = [
        "strength",
        "agility",
        "constitution",
        "intelligence",
        "charisma",
        "luck",
    ];
    // 派生属性当前不在 WorldState 中，这里列出已知名称
    let derived_attrs = [
        "max_carry_weight",
        "physical_damage",
        "dodge_rate",
        "critical_rate",
        "skill_learning_speed",
        "exp_bonus",
        "stamina_regen",
        "qi_regen",
        "perception",
        "social_interaction",
        "event_probability",
    ];

    for (name, &value) in &raw {
        let display_name = engine.get_display_name(name).unwrap_or(name).to_string();
        let category = if status_attrs.contains(&name.as_str()) {
            "status"
        } else if primary_attrs.contains(&name.as_str()) {
            "primary"
        } else if derived_attrs.contains(&name.as_str()) {
            "derived"
        } else {
            "unknown"
        };

        // 格式化值
        let value_str = if category == "primary" {
            // 先天属性 - 显示当前值，带成长标记
            // 注意：先天属性通常有 birth_range，但没有 max_value 概念
            // 这里简化处理，只显示当前值
            format!("{}", value)
        } else if category == "status" {
            // 状态值 - 显示当前值
            format!("{}", value)
        } else {
            // 派生属性或其他
            format!("{}", value)
        };

        formatted.push(FormattedAttribute {
            name: name.clone(),
            display_name,
            value_str,
            category: category.to_string(),
        });
    }

    // 按类别排序：primary -> status -> derived -> unknown
    formatted.sort_by(|a, b| {
        let order = |c: &str| match c {
            "primary" => 0,
            "status" => 1,
            "derived" => 2,
            _ => 3,
        };
        order(&a.category).cmp(&order(&b.category))
    });

    AttributesGlimpse {
        tick_id: state.tick_id,
        attributes: formatted,
        raw,
        warning: "此数据为梦中一瞥，仅限当前决策周期使用。禁止存储到记忆系统。".to_string(),
    }
}

/// 生成叙事化上下文
pub fn generate_context_markdown(
    state: &WorldState,
    relationship_store: &RelationshipStore,
    engine: &NarrativeEngine,
) -> String {
    generate_impl(state, Some(relationship_store), engine)
}

/// 生成无关系存储的简化上下文
pub fn generate_context_markdown_no_relationship(
    state: &WorldState,
    engine: &NarrativeEngine,
) -> String {
    generate_impl(state, None, engine)
}

/// 内部实现
fn generate_impl(
    state: &WorldState,
    relationship_store: Option<&RelationshipStore>,
    engine: &NarrativeEngine,
) -> String {
    let mut sections: Vec<String> = Vec::new();

    // Header
    sections.push("# 游戏状态上下文".to_string());
    sections.push("".to_string());
    sections.push(format!("> 生成时间: Tick {}", state.tick_id));
    sections.push("".to_string());

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

    // 自身状态 - 叙事化描述（不暴露数值）
    sections.push("".to_string());
    sections.push("## 自身状态".to_string());

    let attrs: HashMap<String, i32> = state
        .self_state
        .attributes
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect();

    let narrative = engine.generate_narrative(&attrs, &state.self_state.status_effects);

    sections.push(format!("- 身体: {}", narrative.body_status));
    sections.push(format!("- 饥饿: {}", narrative.hunger_status));
    sections.push(format!("- 口渴: {}", narrative.thirst_status));
    sections.push(format!("- 体力: {}", narrative.stamina_status));

    // 非标准属性
    let standard = ["hp", "hunger", "thirst", "stamina"];
    for (name, value) in &attrs {
        if !standard.contains(&name.as_str()) {
            sections.push(format!(
                "- {}: {}",
                name,
                engine.describe_attribute(name, *value)
            ));
        }
    }

    // 状态效果
    if !narrative.status_effects.is_empty() {
        sections.push("".to_string());
        sections.push(format!(
            "**特殊状态**: {}",
            narrative.status_effects.join("、")
        ));
    }

    // 背包
    if !state.self_state.inventory.is_empty() {
        sections.push("".to_string());
        sections.push("## 背包".to_string());
        for item in &state.self_state.inventory {
            let eq = if item.is_equipped { " [已装备]" } else { "" };
            sections.push(format!("- {} x{}{}", item.name, item.quantity, eq));
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
            sections.push(format!("- **{}**{}{}", entity.name, dist, rel));
        }
    } else {
        sections.push("无".to_string());
    }

    // 地面物品
    sections.push("".to_string());
    sections.push("## 地面物品".to_string());
    if !state.nearby_items.is_empty() {
        for item in &state.nearby_items {
            sections.push(format!("- {} x{}", item.name, item.quantity));
        }
    } else {
        sections.push("无".to_string());
    }

    // 最近事件
    if !state.events_log.is_empty() {
        sections.push("".to_string());
        sections.push("## 最近事件".to_string());
        for event in state.events_log.iter().rev().take(5).rev() {
            sections.push(format!("- {}", event.description));
        }
    }

    // 可用动作
    if !state.available_actions.is_empty() {
        sections.push("".to_string());
        sections.push("## 可用动作".to_string());
        let actions: Vec<String> = state
            .available_actions
            .iter()
            .map(|a| format!("`{}`", a.action))
            .collect();
        sections.push(actions.join(", "));
    }

    sections.join("\n")
}

/// 创建叙事引擎
///
/// 配置加载优先级：
/// 1. Agent 数据目录: ~/.cyber-jianghu/config/narrative_config.json
/// 2. 内置配置（硬编码在二进制中）
///
/// 注意：Agent 不能直接访问 Server 的开发环境文件
pub fn create_narrative_engine() -> NarrativeEngine {
    // 尝试从 Agent 自己的数据目录加载
    if let Some(home) = dirs::home_dir() {
        let config_path = home
            .join(".cyber-jianghu")
            .join("config")
            .join("narrative_config.json");

        if config_path.exists() {
            match NarrativeConfig::from_file(&config_path) {
                Ok(config) => {
                    tracing::info!(
                        "[context] Loaded narrative config from {}",
                        config_path.display()
                    );
                    return NarrativeEngine::new(config);
                }
                Err(e) => tracing::warn!("[context] Failed to load narrative config: {}", e),
            }
        }
    }

    // 使用内置配置
    tracing::info!("[context] Using builtin narrative config");
    NarrativeEngine::with_builtin_config()
}
