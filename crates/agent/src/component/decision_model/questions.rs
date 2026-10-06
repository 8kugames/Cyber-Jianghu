// ============================================================================
// 决策问题构造：结构化实体候选 + act1/act2/item1/agent1/loc1 + 门控策略
// ============================================================================
//
// 问题文本与评测口径（tmp/eval_startlux_decision.py 的 build_questions /
// build_entity_questions 顺序模式）逐字一致——这是训练时的输入分布。
// 与评测的差异仅在候选来源：评测用文本正则提取，生产用 WorldState 结构化
// 实体列表（背包/附近物品/可采集/附近的人/相邻地点），覆盖更全。
//
// 门控策略（纯函数，供 runtime/decision.rs 调用）：
//   - act1 confidence >= 阈值且 act1 属于"字段可完全绑定"的动作集；
//   - 所需实体问题存在且答案可回填（缺失即整体回退 LLM 路径）；
//   - act2 仅在「无/休整/观察」时采纳（这些动作无需实体绑定；其余动作的
//     第 2 意图缺字段，构造出来必被天魂驳回，直接丢弃更稳）。

use std::collections::HashMap;

use cyber_jianghu_protocol::{AvailableAction, WorldState};

use super::prompt::{OptionSpec, QType, QuestionSpec};

/// act1 候选数上限（与评测/训练口径一致）
pub const MAX_CANDIDATES: usize = 25;

/// 「无」选项 id
pub const NONE_OPTION: &str = "无";

/// act1 字段可完全由决策问题绑定的动作集（其余动作——说话需 content、
/// 予需 recipient_id、教导/制造需 recipe_id——整体回退 LLM 路径补写）
pub const BINDABLE_ACTIONS: [&str; 8] = ["休整", "移动", "吃", "喝", "用", "取", "观察", "攻击"];

/// act2 可采纳动作集（无需实体绑定即可构造合法意图）
pub const ACT2_ADOPTABLE_ACTIONS: [&str; 3] = ["无", "休整", "观察"];

/// act1 为这些动作时问 item1（与评测 build_entity_questions 的 ITEM_ACTS 一致）
pub const ITEM_ACTS: [&str; 5] = ["取", "用", "吃", "喝", "予"];

/// act1 为这些动作时问 agent1（与评测 AGENT_ACTS 一致）
pub const AGENT_ACTS: [&str; 4] = ["攻击", "教导", "说话", "观察"];

// ---------------------------------------------------------------------------
// 动作词表
// ---------------------------------------------------------------------------

/// 训练时的动作选项顺序（actions.yaml data 键序，评测脚本同源）。
/// server 下发顺序若有出入，以此为准；新动作追加在表尾。
const ACTION_ORDER: [&str; 12] = [
    "予", "取", "用", "吃", "喝", "移动", "说话", "观察", "攻击", "休整", "制造", "教导",
];

/// 动作 → 判据（描述规范化：压缩空白 + 截断 80 字符；空描述回退动作名，
/// 与评测 load_actions 的 `desc[:80] or k` 口径一致）
pub fn action_criteria(available: &[AvailableAction]) -> Vec<(String, String)> {
    let mut map: HashMap<&str, &AvailableAction> = HashMap::new();
    for a in available {
        map.entry(a.action.as_str()).or_insert(a);
    }
    let mut ordered: Vec<(String, String)> = Vec::with_capacity(ACTION_ORDER.len());
    for name in ACTION_ORDER {
        if let Some(a) = map.remove(name) {
            ordered.push((name.to_string(), normalize_desc(&a.description, name)));
        }
    }
    // 词表之外的新动作（server 演化）：追加在表尾，保持选项顺序稳定
    let mut rest: Vec<&str> = map.keys().copied().collect();
    rest.sort_unstable();
    for name in rest {
        let a = map[name];
        ordered.push((a.action.clone(), normalize_desc(&a.description, &a.action)));
    }
    ordered
}

fn normalize_desc(desc: &str, fallback: &str) -> String {
    let squeezed = desc.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = squeezed.chars().take(80).collect();
    let cut: String = chars.into_iter().collect();
    if cut.trim().is_empty() {
        fallback.to_string()
    } else {
        cut
    }
}

// ---------------------------------------------------------------------------
// 结构化实体候选
// ---------------------------------------------------------------------------

/// 物品候选来源（取 动作绑定时推断 source_type 用）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemProvenance {
    Inventory,
    Ground,
    Resource,
}

/// 结构化实体候选（每类上限 MAX_CANDIDATES）
#[derive(Debug, Clone, Default)]
pub struct EntityCandidates {
    /// (展示串, 归一 key, 来源)
    pub items: Vec<(String, String, ItemProvenance)>,
    /// (短 id, 归一 key)——附近 Agent 的 8 位短 ID
    pub agents: Vec<(String, String)>,
    /// (地点名或 node_id, 归一 key)
    pub locs: Vec<(String, String)>,
}

impl EntityCandidates {
    pub fn item_provenance(&self, norm_key: &str) -> Option<ItemProvenance> {
        self.items
            .iter()
            .find(|(_, k, _)| k == norm_key)
            .map(|(_, _, p)| *p)
    }
}

/// 完整 uuid → 前 8 位 hex；其余 trim 原样（与评测 norm_id 一致）
pub fn norm_id(v: &str) -> String {
    let v = v.trim();
    if v.len() == 36
        && v.as_bytes()[8] == b'-'
        && v.as_bytes()[13] == b'-'
        && v.as_bytes()[18] == b'-'
        && v.as_bytes()[23] == b'-'
        && v.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
    {
        return v[..8].to_string();
    }
    v.to_string()
}

/// 从 WorldState 构建结构化候选（不用文本正则）
pub fn build_candidates(ws: &WorldState) -> EntityCandidates {
    let mut c = EntityCandidates::default();
    let mut seen = std::collections::HashSet::new();

    let mut push_item = |display: String, provenance: ItemProvenance, c: &mut EntityCandidates| {
        let key = norm_id(&display);
        if key.is_empty() || !seen.insert(key.clone()) {
            return;
        }
        if c.items.len() >= MAX_CANDIDATES {
            return;
        }
        c.items.push((display, key.clone(), provenance));
    };

    // 背包：展示串「名称[短uuid]」+ 裸名（与 layer0 接受的两种合法形态一致）
    for item in &ws.self_state.inventory {
        push_item(
            cyber_jianghu_protocol::display_item_ref(&item.name, &item.item_id),
            ItemProvenance::Inventory,
            &mut c,
        );
        push_item(item.name.clone(), ItemProvenance::Inventory, &mut c);
    }
    for item in &ws.nearby_items {
        push_item(
            cyber_jianghu_protocol::display_item_ref(&item.name, &item.item_id),
            ItemProvenance::Ground,
            &mut c,
        );
        push_item(item.name.clone(), ItemProvenance::Ground, &mut c);
    }
    for item in &ws.location.gatherable_items {
        push_item(
            cyber_jianghu_protocol::display_item_ref(&item.name, &item.item_id),
            ItemProvenance::Resource,
            &mut c,
        );
        push_item(item.name.clone(), ItemProvenance::Resource, &mut c);
    }

    // 人物：8 位短 ID（target_agent_id 的提交口径）
    let mut seen_agents = std::collections::HashSet::new();
    for e in &ws.entities {
        let short = cyber_jianghu_protocol::short_id(&e.id);
        if seen_agents.insert(short.clone()) && c.agents.len() < MAX_CANDIDATES {
            c.agents.push((short.clone(), short));
        }
    }

    // 地点：节点名 + node_id 双形态（名称经 canonicalize_move_target 归一）
    let mut seen_locs = std::collections::HashSet::new();
    for node in &ws.location.adjacent_nodes {
        for display in [node.name.as_str(), node.node_id.as_str()] {
            let display = display.trim();
            if display.is_empty() {
                continue;
            }
            let key = norm_id(display);
            if seen_locs.insert(key.clone()) && c.locs.len() < MAX_CANDIDATES {
                c.locs.push((display.to_string(), key));
            }
        }
    }
    c
}

// ---------------------------------------------------------------------------
// 问题构造（文本与评测脚本逐字一致）
// ---------------------------------------------------------------------------

fn criteria_to_options(criteria: &[(String, String)]) -> Vec<OptionSpec> {
    criteria
        .iter()
        .map(|(id, desc)| OptionSpec::with_criterion(id.clone(), desc.clone()))
        .collect()
}

fn choice_question(instructions: &str, mut options: Vec<OptionSpec>) -> QuestionSpec {
    options.shrink_to_fit();
    QuestionSpec {
        qtype: QType::Choice,
        instructions: instructions.to_string(),
        options,
    }
}

/// act1（12 选 1）
pub fn build_act1_question(criteria: &[(String, String)]) -> QuestionSpec {
    choice_question(
        "你是该角色本人。根据证据中的世界状态、记忆、上一轮结果与紧迫程度,选择你此刻最应该执行的第一个动作。",
        criteria_to_options(criteria),
    )
}

/// act2（12 动作 + 「无」）
pub fn build_act2_question(criteria: &[(String, String)]) -> QuestionSpec {
    let mut options = vec![OptionSpec::with_criterion(NONE_OPTION, "不需要第二个动作")];
    options.extend(criteria_to_options(criteria));
    choice_question(
        "选择你此刻第二个要执行的动作(可与第一个构成因果链如取后用,或独立并行);必须与第一个动作不同;若确无第二个动作,选「无」。",
        options,
    )
}

/// item1（顺序模式：act1 已知后构造；候选为空时返回 None = 不问）
pub fn build_item1_question(act1: &str, items: &[(String, String)]) -> Option<QuestionSpec> {
    if items.is_empty() || !ITEM_ACTS.contains(&act1) {
        return None;
    }
    let mut options = vec![OptionSpec::with_criterion(
        NONE_OPTION,
        "证据中找不到目标物品",
    )];
    options.extend(
        items
            .iter()
            .map(|(id, key)| OptionSpec::with_criterion(key.clone(), id.clone())),
    );
    Some(choice_question(
        &format!(
            "第一个动作已定为「{act1}」。从证据中选出该动作的目标物品的准确标识(完整ID或名称);若证据中找不到目标物品,选「无」。"
        ),
        options,
    ))
}

/// agent1（顺序模式）
pub fn build_agent1_question(act1: &str, agents: &[(String, String)]) -> Option<QuestionSpec> {
    if agents.is_empty() || !AGENT_ACTS.contains(&act1) {
        return None;
    }
    let mut options = vec![OptionSpec::with_criterion(
        NONE_OPTION,
        "该动作不需要目标人物",
    )];
    options.extend(
        agents
            .iter()
            .map(|(id, key)| OptionSpec::with_criterion(key.clone(), id.clone())),
    );
    Some(choice_question(
        &format!(
            "第一个动作已定为「{act1}」。若该动作需要目标人物,从证据中选出其准确ID;若不需要(如公开发言、环顾四周),选「无」。"
        ),
        options,
    ))
}

/// loc1（顺序模式）
pub fn build_loc1_question(act1: &str, locs: &[(String, String)]) -> Option<QuestionSpec> {
    if act1 != "移动" || locs.is_empty() {
        return None;
    }
    let mut options = vec![OptionSpec::with_criterion(NONE_OPTION, "证据中无可达地点")];
    options.extend(
        locs.iter()
            .map(|(id, key)| OptionSpec::with_criterion(key.clone(), id.clone())),
    );
    Some(choice_question(
        "第一个动作已定为「移动」。从证据中选出目标地点;若证据中无可达地点,选「无」。",
        options,
    ))
}

// ---------------------------------------------------------------------------
// 状态文本
// ---------------------------------------------------------------------------

/// 认知摘要块标题（与训练数据 prepare_decision_sft.py 逐字一致）
pub const COGNITION_TITLE: &str = "\n\n## 角色此刻的认知(内心独白与决策依据)\n";

/// 认知摘要块（与评测 B 配置 / 训练数据逐字一致的格式）
pub fn cognition_block(
    self_status: &str,
    environment: &str,
    key_observations: &[String],
    primary_drive: &str,
    drive_intensity: u8,
    thought_process: &str,
    emotion: Option<(&str, f32)>,
) -> String {
    let mut parts = vec![
        format!("自我状态: {self_status}"),
        format!("环境: {environment}"),
        format!("关键观察: {}", key_observations.join("；")),
        format!("主导驱动: {primary_drive} (强度 {drive_intensity}/10)"),
        format!("思考过程: {thought_process}"),
    ];
    if let Some((label, intensity)) = emotion {
        parts.push(format!("情绪: {label} ({intensity})"));
    }
    // 与 Python `p.strip(": ")` 一致：剔除首尾的空格与冒号后非空才保留
    let kept: Vec<String> = parts
        .drain(..)
        .filter(|p| !p.trim_matches(|c| c == ':' || c == ' ').is_empty())
        .collect();
    format!("{COGNITION_TITLE}{}", kept.join("\n"))
}

/// 决策状态文本 = 人设 system 消息 + tick 消息 + 认知摘要块
/// （与训练数据 system persona + tick user prompt + 认知摘要块 的拼装完全一致）
pub fn build_state_text(system_message: &str, tick_message: &str, cognition: &str) -> String {
    format!("{system_message}\n\n{tick_message}{cognition}")
}

// ---------------------------------------------------------------------------
// 门控与意图构造
// ---------------------------------------------------------------------------

/// act1 置信度门控：动作可绑定 且 置信度达标
pub fn act1_gate_pass(act1: &str, confidence: f64, threshold: f32) -> bool {
    BINDABLE_ACTIONS.contains(&act1) && confidence >= threshold as f64
}

/// act2 一致性：必须异于 act1 且可采纳（无需实体绑定）
pub fn act2_gate_pass(act1: &str, act2: &str) -> bool {
    act2 != act1 && ACT2_ADOPTABLE_ACTIONS.contains(&act2)
}

/// 决策答案 → Intent 集合构造所需的绑定信息
#[derive(Debug, Clone)]
pub struct ActionBinding {
    pub action_type: String,
    pub action_data: Option<serde_json::Value>,
    /// 绑定失败原因（该 tick 回退 LLM 路径）
    pub bind_error: Option<String>,
}

/// 由 act1 + 实体答案构造主意图 action_data。
///
/// 绑定失败（需要字段但候选/答案缺失，如 攻击 答了「无」、取 绑了背包物品）
/// 返回 bind_error，由调用方整体回退。
pub fn bind_act1(
    act1: &str,
    item_answer: Option<&str>,
    agent_answer: Option<&str>,
    loc_answer: Option<&str>,
    candidates: &EntityCandidates,
) -> ActionBinding {
    let mut data = serde_json::Map::new();
    match act1 {
        "休整" => {}
        "观察" => {
            if let Some(agent) = agent_answer
                && agent != NONE_OPTION
            {
                data.insert(
                    "target_agent_id".into(),
                    serde_json::Value::String(agent.into()),
                );
            }
        }
        "移动" => {
            return match loc_answer {
                Some(loc) if loc != NONE_OPTION => ActionBinding {
                    action_type: act1.into(),
                    action_data: Some(serde_json::json!({ "target_location": loc })),
                    bind_error: None,
                },
                _ => ActionBinding {
                    action_type: act1.into(),
                    action_data: None,
                    bind_error: Some("移动 但无目标地点绑定".into()),
                },
            };
        }
        "吃" | "喝" | "用" => {
            return match item_answer {
                Some(item) if item != NONE_OPTION => ActionBinding {
                    action_type: act1.into(),
                    action_data: Some(serde_json::json!({ "item_id": item })),
                    bind_error: None,
                },
                _ => ActionBinding {
                    action_type: act1.into(),
                    action_data: None,
                    bind_error: Some(format!("{act1} 但无目标物品绑定")),
                },
            };
        }
        "取" => {
            return match item_answer {
                Some(item) if item != NONE_OPTION => {
                    let provenance = candidates.item_provenance(&norm_id(item));
                    match provenance {
                        Some(ItemProvenance::Ground) | Some(ItemProvenance::Resource) => {
                            data.insert("item_id".into(), serde_json::Value::String(item.into()));
                            data.insert(
                                "source_type".into(),
                                serde_json::Value::String(
                                    if provenance == Some(ItemProvenance::Resource) {
                                        "resource"
                                    } else {
                                        "ground"
                                    }
                                    .into(),
                                ),
                            );
                            ActionBinding {
                                action_type: act1.into(),
                                action_data: Some(serde_json::Value::Object(data)),
                                bind_error: None,
                            }
                        }
                        _ => ActionBinding {
                            action_type: act1.into(),
                            action_data: None,
                            bind_error: Some("取 但候选不在地面/资源点（背包物品不可取）".into()),
                        },
                    }
                }
                _ => ActionBinding {
                    action_type: act1.into(),
                    action_data: None,
                    bind_error: Some("取 但无目标物品绑定".into()),
                },
            };
        }
        "攻击" => {
            return match agent_answer {
                Some(agent) if agent != NONE_OPTION => ActionBinding {
                    action_type: act1.into(),
                    action_data: Some(serde_json::json!({ "target_agent_id": agent })),
                    bind_error: None,
                },
                _ => ActionBinding {
                    action_type: act1.into(),
                    action_data: None,
                    bind_error: Some("攻击 但无目标人物绑定".into()),
                },
            };
        }
        other => {
            return ActionBinding {
                action_type: other.into(),
                action_data: None,
                bind_error: Some(format!("动作 {other} 不在决策模型可绑定集")),
            };
        }
    }
    ActionBinding {
        action_type: act1.into(),
        action_data: if data.is_empty() {
            None
        } else {
            Some(serde_json::Value::Object(data))
        },
        bind_error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_actions() -> Vec<AvailableAction> {
        ACTION_ORDER
            .iter()
            .map(|name| AvailableAction {
                action: name.to_string(),
                name: String::new(),
                description: format!("{name} 的描述。完整句子保持简洁。"),
                category: String::new(),
                valid_targets: None,
                required_fields: vec![],
                optional_fields: vec![],
                ooc_risk: cyber_jianghu_protocol::OocRisk::Low,
                requirements: vec![],
                effects: vec![],
            })
            .collect()
    }

    #[test]
    fn action_criteria_order_and_truncation() {
        let criteria = action_criteria(&sample_actions());
        assert_eq!(criteria.len(), 12);
        assert_eq!(criteria[0].0, "予");
        assert_eq!(criteria[11].0, "教导");
        // 描述压缩空白并截断 80 字符
        assert!(criteria[0].1.chars().count() <= 80);
        // 乱序输入不影响顺序
        let mut shuffled = sample_actions();
        shuffled.reverse();
        assert_eq!(action_criteria(&shuffled)[0].0, "予");
    }

    #[test]
    fn norm_id_full_uuid_and_passthrough() {
        assert_eq!(norm_id("a65df604-1234-5678-9abc-def012345678"), "a65df604");
        assert_eq!(norm_id("馒头[a65df604]"), "馒头[a65df604]");
        assert_eq!(norm_id("  馒头 "), "馒头");
    }

    #[test]
    fn candidates_from_world_state_structured() {
        use cyber_jianghu_protocol::types::entities::{Entity, InventoryItem, SceneItem};
        use cyber_jianghu_protocol::types::locations::{AdjacentNode, Location};
        let uuid1 = uuid::Uuid::parse_str("a65df604-1234-5678-9abc-def012345678").expect("uuid");
        let uuid2 = uuid::Uuid::parse_str("b75ef605-1234-5678-9abc-def012345678").expect("uuid");
        let ws = WorldState {
            event_type: "world_state".into(),
            tick_id: 1,
            agent_id: None,
            world_time: cyber_jianghu_protocol::WorldTime {
                year: 1,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
                weather: "晴".into(),
            },
            location: Location {
                node_id: "inn".into(),
                name: "客栈".into(),
                node_type: String::new(),
                adjacent_nodes: vec![
                    AdjacentNode {
                        node_id: "gate_street".into(),
                        name: "城门口".into(),
                        travel_cost: 2,
                    },
                    AdjacentNode {
                        node_id: "back_alley".into(),
                        name: "后巷".into(),
                        travel_cost: 1,
                    },
                ],
                gatherable_items: vec![],
                parent_chain: vec![],
            },
            self_state: cyber_jianghu_protocol::AgentSelfState {
                inventory: vec![InventoryItem {
                    item_id: uuid1.to_string(),
                    name: "馒头".into(),
                    quantity: 3,
                    is_equipped: false,
                    item_type: "consumable".into(),
                }],
                attributes: Default::default(),
                derived_attributes: Default::default(),
                attribute_descriptions: Default::default(),
                survival_drives: vec![],
                status_effects: vec![],
                skills: vec![],
                recipe_details: vec![],
                age_years: None,
                max_age: None,
            },
            entities: vec![Entity {
                id: uuid2,
                name: "温九辞".into(),
                distance: 0,
                state: "存活".into(),
                hostile: false,
                recent_actions: vec![],
            }],
            nearby_items: vec![SceneItem {
                item_id: uuid::Uuid::new_v4().to_string(),
                name: "水壶".into(),
                quantity: 1,
                item_type: "consumable".into(),
            }],
            events_log: vec![],
            private_dialogue_log: vec![],
            last_execution_summary: None,
        };
        let c = build_candidates(&ws);
        // 背包：ref 形态 + 裸名
        assert!(
            c.items
                .iter()
                .any(|(d, _, p)| d.contains("馒头") && *p == ItemProvenance::Inventory)
        );
        // 附近物品
        assert!(
            c.items
                .iter()
                .any(|(d, _, p)| d.contains("水壶") && *p == ItemProvenance::Ground)
        );
        // 人物短 id
        assert!(c.agents.iter().any(|(d, _)| d == "b75ef605"));
        // 地点双形态
        assert!(c.locs.iter().any(|(d, _)| d == "城门口"));
        assert!(c.locs.iter().any(|(d, _)| d == "gate_street"));
    }

    #[test]
    fn question_texts_match_eval_wording() {
        let criteria = action_criteria(&sample_actions());
        let q1 = build_act1_question(&criteria);
        assert_eq!(
            q1.instructions,
            "你是该角色本人。根据证据中的世界状态、记忆、上一轮结果与紧迫程度,选择你此刻最应该执行的第一个动作。"
        );
        assert_eq!(q1.options.len(), 12);
        let q2 = build_act2_question(&criteria);
        assert_eq!(q2.options.len(), 13);
        assert_eq!(q2.options[0].id, "无");
        let items = vec![("馒头[a65df604]".to_string(), "馒头[a65df604]".to_string())];
        let qi = build_item1_question("吃", &items).expect("吃 应问 item1");
        assert!(qi.options.len() == 2 && qi.options[0].id == "无");
        assert!(qi.instructions.starts_with("第一个动作已定为「吃」。"));
        assert!(
            build_item1_question("移动", &items).is_none(),
            "移动 不问物品"
        );
        assert!(build_loc1_question("移动", &[("城门口".into(), "城门口".into())]).is_some());
        assert!(build_loc1_question("吃", &[("城门口".into(), "城门口".into())]).is_none());
    }

    #[test]
    fn entity_question_requires_candidates() {
        assert!(build_item1_question("吃", &[]).is_none(), "空候选不问");
        assert!(build_agent1_question("攻击", &[]).is_none());
    }

    #[test]
    fn cognition_block_exact_format() {
        let block = cognition_block(
            "饥饿难耐",
            "大堂中三两客人",
            &["桌上有半壶酒".into(), "角落里坐着一位陌生人".into()],
            "饥饿驱使觅食",
            7,
            "先找些吃的",
            Some(("忐忑", 0.5)),
        );
        assert_eq!(
            block,
            "\n\n## 角色此刻的认知(内心独白与决策依据)\n自我状态: 饥饿难耐\n环境: 大堂中三两客人\n关键观察: 桌上有半壶酒；角落里坐着一位陌生人\n主导驱动: 饥饿驱使觅食 (强度 7/10)\n思考过程: 先找些吃的\n情绪: 忐忑 (0.5)"
        );
        // 空字段保留标签行（与 Python strip 过滤行为一致）
        let block = cognition_block("", "", &[], "", 0, "思考", None);
        assert!(block.contains("自我状态: "), "空值行保留标签");
    }

    #[test]
    fn state_text_concatenation() {
        let s = build_state_text("SYS", "TICK", "\n\n## 认知\nX");
        assert_eq!(s, "SYS\n\nTICK\n\n## 认知\nX");
    }

    #[test]
    fn gate_boundaries() {
        // 阈值边界：等于阈值通过
        assert!(act1_gate_pass("吃", 0.70, 0.70));
        assert!(!act1_gate_pass("吃", 0.6999, 0.70));
        // 不可绑定动作即使高置信也回退
        assert!(!act1_gate_pass("说话", 0.99, 0.70));
        assert!(!act1_gate_pass("制造", 0.99, 0.70));
        assert!(!act1_gate_pass("予", 0.99, 0.70));
        assert!(!act1_gate_pass("教导", 0.99, 0.70));
        // act2 一致性
        assert!(act2_gate_pass("吃", "无"));
        assert!(act2_gate_pass("吃", "休整"));
        assert!(act2_gate_pass("吃", "观察"));
        assert!(!act2_gate_pass("吃", "取"), "需绑定字段的 act2 不采纳");
        assert!(!act2_gate_pass("吃", "吃"), "act2 必须异于 act1");
    }

    #[test]
    fn bind_act1_paths() {
        let c = EntityCandidates {
            items: vec![
                (
                    "水壶[b75ef605]".into(),
                    "水壶[b75ef605]".into(),
                    ItemProvenance::Inventory,
                ),
                (
                    "野果[aabbccdd]".into(),
                    "野果[aabbccdd]".into(),
                    ItemProvenance::Ground,
                ),
            ],
            agents: Vec::new(),
            locs: Vec::new(),
        };
        // 吃：绑定物品
        let b = bind_act1("吃", Some("水壶[b75ef605]"), None, None, &c);
        assert!(b.bind_error.is_none());
        assert_eq!(b.action_data.unwrap()["item_id"], "水壶[b75ef605]");
        // 吃答「无」→ 回退
        assert!(
            bind_act1("吃", Some("无"), None, None, &c)
                .bind_error
                .is_some()
        );
        assert!(bind_act1("吃", None, None, None, &c).bind_error.is_some());
        // 取：地面来源注入 source_type=ground
        let b = bind_act1("取", Some("野果[aabbccdd]"), None, None, &c);
        let d = b.action_data.expect("取 绑定成功");
        assert_eq!(d["source_type"], "ground");
        // 取：背包物品 → 回退
        assert!(
            bind_act1("取", Some("水壶[b75ef605]"), None, None, &c)
                .bind_error
                .is_some()
        );
        // 移动：绑定地点
        let c2 = EntityCandidates {
            items: Vec::new(),
            agents: Vec::new(),
            locs: vec![("城门口".into(), "城门口".into())],
        };
        let b = bind_act1("移动", None, None, Some("城门口"), &c2);
        assert_eq!(b.action_data.unwrap()["target_location"], "城门口");
        // 攻击答「无」→ 回退（攻击必须有目标）
        assert!(
            bind_act1("攻击", None, Some("无"), None, &c)
                .bind_error
                .is_some()
        );
        // 观察：无目标 → 合法空数据（环顾四周）
        let b = bind_act1("观察", None, Some("无"), None, &c);
        assert!(b.bind_error.is_none() && b.action_data.is_none());
        // 休整：无数据
        let b = bind_act1("休整", None, None, None, &c);
        assert!(b.bind_error.is_none() && b.action_data.is_none());
    }
}
