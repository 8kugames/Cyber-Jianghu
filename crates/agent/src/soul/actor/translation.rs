// ============================================================================
// 中文 LLM 边界翻译层
// ============================================================================
//
// 薄翻译层：LLM 输出的中文 action_type 别名 → canonical 中文名
// 翻译硬边界：必须在 ReflectorSoul 之前完成
// 数据驱动：映射来自 AvailableAction 的 aliases/field_aliases，零硬编码
// ============================================================================

use cyber_jianghu_protocol::{AvailableAction, WorldState};
use std::collections::HashMap;

// ============================================================================
// 通用实体别名映射
// ============================================================================

/// 通用实体别名映射: alias (lowercase) → canonical ID
///
/// 用于 location、item 等实体的别名归一化。
/// LLM 输出的中文/英文别名 → canonical 中文主键。
pub struct EntityAliasMap {
    /// alias (lowercase) → canonical ID
    forward: HashMap<String, String>,
}

impl EntityAliasMap {
    /// 从 (canonical_id, aliases) 对列表构建
    pub fn from_entries(entries: Vec<(String, Vec<String>)>) -> Self {
        let mut forward = HashMap::new();
        for (canonical, aliases) in entries {
            // canonical → self
            forward
                .entry(canonical.to_lowercase())
                .or_insert_with(|| canonical.clone());
            // aliases → canonical
            for alias in aliases {
                forward.insert(alias.to_lowercase(), canonical.clone());
            }
        }
        Self { forward }
    }

    /// 翻译别名 → canonical ID
    pub fn translate(&self, input: &str) -> Option<String> {
        self.forward.get(&input.to_lowercase()).cloned()
    }

    /// 空映射
    pub fn empty() -> Self {
        Self {
            forward: HashMap::new(),
        }
    }

    /// 从 WorldState 的 adjacent_nodes 构建位置别名映射
    pub fn from_world_state_locations(world_state: &WorldState) -> Self {
        let entries: Vec<(String, Vec<String>)> = world_state
            .location
            .adjacent_nodes
            .iter()
            .map(|n| (n.node_id.clone(), n.aliases.clone()))
            .collect();
        // 同时包含当前位置自身
        let mut entries = entries;
        entries.push((world_state.location.node_id.clone(), vec![]));
        Self::from_entries(entries)
    }

    /// 从 WorldState 的 inventory + nearby_items + gatherable_items 合并构建 item 别名映射
    ///
    /// 覆盖所有 LLM 可见的物品来源，确保拾取/进食/饮水都能翻译
    pub fn from_world_state_all_items(world_state: &WorldState) -> Self {
        let mut entries: Vec<(String, Vec<String>)> = Vec::new();

        // 背包物品
        for item in &world_state.self_state.inventory {
            entries.push((item.item_id.clone(), item.aliases.clone()));
        }

        // 附近地面物品
        for item in &world_state.nearby_items {
            entries.push((item.item_id.clone(), item.aliases.clone()));
        }

        // 可采集物品
        for item in &world_state.location.gatherable_items {
            entries.push((item.item_id.clone(), item.aliases.clone()));
        }

        Self::from_entries(entries)
    }

    /// 从 WorldState 的 entities 构建 agent name → UUID 别名映射
    ///
    /// LLM 输出 "小鹿" → 自动翻译为 UUID
    pub fn from_world_state_entities(world_state: &WorldState) -> Self {
        let entries: Vec<(String, Vec<String>)> = world_state
            .entities
            .iter()
            .map(|e| (e.id.to_string(), vec![e.name.clone()]))
            .collect();
        Self::from_entries(entries)
    }
}

// ============================================================================
// 实体翻译注册表 (数据驱动)
// ============================================================================

/// 实体翻译注册表 — 数据驱动的字段值翻译
///
/// 注册 field_name → EntityAliasMap 映射，`translate()` 自动遍历所有已注册字段。
/// 新增实体类型只需在 `from_world_state()` 中加一行注册。
///
/// ```text
/// // 注册示例:
/// ("target_location", EntityAliasMap::from_world_state_locations(ws)),
/// ("item_id",         EntityAliasMap::from_world_state_all_items(ws)),
/// ("target_agent_id", EntityAliasMap::from_world_state_entities(ws)),
/// ```
pub struct EntityTranslationRegistry {
    /// (field_name, EntityAliasMap) — 按注册顺序翻译
    field_maps: Vec<(String, EntityAliasMap)>,
}

impl EntityTranslationRegistry {
    /// 从 WorldState 自动构建所有已注册字段的别名映射
    ///
    /// 当前注册字段:
    /// - `target_location` — 相邻地点别名
    /// - `item_id` — 背包+地面+可采集物品别名
    /// - `target_agent_id` — 附近角色 name→UUID
    ///
    /// 新增实体类型只需在此加一行
    pub fn from_world_state(ws: &WorldState) -> Self {
        Self {
            field_maps: vec![
                (
                    "target_location".to_string(),
                    EntityAliasMap::from_world_state_locations(ws),
                ),
                (
                    "item_id".to_string(),
                    EntityAliasMap::from_world_state_all_items(ws),
                ),
                (
                    "target_agent_id".to_string(),
                    EntityAliasMap::from_world_state_entities(ws),
                ),
            ],
        }
    }

    /// 翻译 action_data 中所有已注册字段的值
    ///
    /// 遍历 field_maps，对每个已注册字段做 alias→canonical 翻译。
    /// 未注册的字段不受影响。
    pub fn translate(&self, data: &mut serde_json::Value) {
        let Some(obj) = data.as_object_mut() else {
            return;
        };

        for (field_name, alias_map) in &self.field_maps {
            if let Some(val) = obj.get_mut(field_name)
                && let Some(s) = val.as_str()
                && let Some(translated) = alias_map.translate(s)
            {
                *val = serde_json::Value::String(translated);
            }
        }
    }
}

// ============================================================================
// action_type 别名映射
// ============================================================================

/// action_type 别名映射: alias (lowercase) → canonical chinese name
///
/// action_type 全链路已为中文，此映射仅做别名归一化
pub struct ActionAliasMap {
    /// alias (lowercase) → canonical chinese name
    forward: HashMap<String, String>,
}

/// action_data 字段别名映射: (action_type, field_alias) → canonical field
///
/// 按 action_type 隔离，仅翻译该 action 的 required_fields 对应的别名
pub struct FieldAliasMap(HashMap<String, HashMap<String, String>>);

impl ActionAliasMap {
    /// 从 AvailableAction list 构建
    ///
    /// 构建 alias → canonical 映射：
    /// - 每个动作的 `action` (canonical chinese) 映射到自身
    /// - 每个动作的 `name` (中文名) 映射到 canonical
    /// - 每个别名映射到 canonical
    /// - 所有 key 统一转小写以支持大小写不敏感匹配
    pub fn from_actions(actions: &[AvailableAction]) -> Self {
        let mut forward = HashMap::new();
        for a in actions {
            let canonical = a.action.clone();
            // canonical → self
            forward
                .entry(canonical.to_lowercase())
                .or_insert_with(|| canonical.clone());
            // name → canonical (name 和 action 可能相同，也可能不同)
            if !a.name.is_empty() {
                forward.insert(a.name.to_lowercase(), canonical.clone());
            }
            // aliases → canonical
            for alias in &a.aliases {
                forward.insert(alias.to_lowercase(), canonical.clone());
            }
        }
        Self { forward }
    }

    /// 翻译 action_type（别名 → canonical chinese）
    ///
    /// 查找顺序:
    /// 1. 精确匹配（lowercase）
    /// 2. 未匹配时返回 None（fail-fast，由调用方决定处理）
    pub fn translate(&self, input: &str) -> Option<String> {
        self.forward.get(&input.to_lowercase()).cloned()
    }
}

impl FieldAliasMap {
    /// 从 AvailableAction list 构建
    ///
    /// 构建 per-action-type 白名单：
    /// - key: action_type (lowercase)
    /// - value: { field_alias (lowercase) → canonical_field }
    /// - 仅包含该 action 的 required_fields 的别名
    pub fn from_actions(actions: &[AvailableAction]) -> Self {
        let mut map = HashMap::new();
        for a in actions {
            let action_key = a.action.to_lowercase();
            let mut field_map = HashMap::new();

            // 只为 required_fields 构建别名映射
            for field in &a.required_fields {
                // canonical field → self
                field_map.insert(field.to_lowercase(), field.clone());
                // 该 field 的别名 → canonical
                if let Some(aliases) = a.field_aliases.get(field) {
                    for alias in aliases {
                        field_map.insert(alias.to_lowercase(), field.clone());
                    }
                }
            }

            if !field_map.is_empty() {
                map.insert(action_key, field_map);
            }
        }
        Self(map)
    }

    /// 翻译 action_data 的 key（中文/别名 → canonical）
    ///
    /// 白名单模式：仅翻译该 action_type 的 required_fields 对应的 key
    /// 未识别的 key 原样保留（不误翻译内容值）
    pub fn translate_data(&self, action_type: &str, data: &mut serde_json::Value) {
        let Some(obj) = data.as_object_mut() else {
            return;
        };

        let Some(field_map) = self.0.get(&action_type.to_lowercase()) else {
            return;
        };

        // 收集需要重命名的 key（避免迭代中修改）
        let renames: Vec<(String, String)> = obj
            .keys()
            .filter_map(|k| {
                field_map
                    .get(&k.to_lowercase())
                    .filter(|canonical| *canonical != k)
                    .map(|canonical| (k.clone(), canonical.clone()))
            })
            .collect();

        // 执行重命名
        for (old_key, new_key) in renames {
            if let Some(value) = obj.remove(&old_key) {
                obj.insert(new_key, value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_test_actions() -> Vec<AvailableAction> {
        let mut fa_speak = HashMap::new();
        fa_speak.insert(
            "content".to_string(),
            vec![
                "内容".to_string(),
                "消息".to_string(),
                "话语".to_string(),
                "message".to_string(),
                "text".to_string(),
            ],
        );

        let mut fa_move = HashMap::new();
        fa_move.insert(
            "target_location".to_string(),
            vec![
                "目标地点".to_string(),
                "目的地".to_string(),
                "destination".to_string(),
                "target".to_string(),
            ],
        );

        let mut fa_eat = HashMap::new();
        fa_eat.insert(
            "item_id".to_string(),
            vec!["物品ID".to_string(), "物品".to_string()],
        );

        vec![
            AvailableAction {
                action: "休息".to_string(),
                name: "休息".to_string(),
                description: String::new(),
                category: String::new(),
                valid_targets: None,
                required_fields: vec![],
                ooc_risk: "low".to_string(),
                aliases: vec!["静修".to_string(), "原地等待".to_string()],
                field_aliases: HashMap::new(),
            },
            AvailableAction {
                action: "说话".to_string(),
                name: "说话".to_string(),
                description: String::new(),
                category: String::new(),
                valid_targets: None,
                required_fields: vec!["content".to_string()],
                ooc_risk: "low".to_string(),
                aliases: vec![
                    "交谈".to_string(),
                    "说".to_string(),
                    "讲话".to_string(),
                    "say".to_string(),
                ],
                field_aliases: fa_speak,
            },
            AvailableAction {
                action: "移动".to_string(),
                name: "移动".to_string(),
                description: String::new(),
                category: String::new(),
                valid_targets: None,
                required_fields: vec!["target_location".to_string()],
                ooc_risk: "low".to_string(),
                aliases: vec![
                    "行走".to_string(),
                    "前往".to_string(),
                    "destination".to_string(),
                ],
                field_aliases: fa_move,
            },
            AvailableAction {
                action: "进食".to_string(),
                name: "进食".to_string(),
                description: String::new(),
                category: String::new(),
                valid_targets: None,
                required_fields: vec!["item_id".to_string()],
                ooc_risk: "low".to_string(),
                aliases: vec!["吃".to_string(), "食用".to_string()],
                field_aliases: fa_eat,
            },
        ]
    }

    #[test]
    fn test_action_alias_canonical_chinese() {
        let actions = make_test_actions();
        let map = ActionAliasMap::from_actions(&actions);
        assert_eq!(map.translate("说话"), Some("说话".to_string()));
        assert_eq!(map.translate("休息"), Some("休息".to_string()));
        assert_eq!(map.translate("移动"), Some("移动".to_string()));
        assert_eq!(map.translate("进食"), Some("进食".to_string()));
    }

    #[test]
    fn test_action_alias_chinese_variant() {
        let actions = make_test_actions();
        let map = ActionAliasMap::from_actions(&actions);
        assert_eq!(map.translate("交谈"), Some("说话".to_string()));
        assert_eq!(map.translate("静修"), Some("休息".to_string()));
        assert_eq!(map.translate("行走"), Some("移动".to_string()));
        assert_eq!(map.translate("吃"), Some("进食".to_string()));
    }

    #[test]
    fn test_action_alias_case_insensitive() {
        let actions = make_test_actions();
        let map = ActionAliasMap::from_actions(&actions);
        // 大小写不敏感（对英文别名有效）
        assert_eq!(map.translate("Say"), Some("说话".to_string()));
        assert_eq!(map.translate("SAY"), Some("说话".to_string()));
    }

    #[test]
    fn test_action_alias_canonical_pass_through() {
        let actions = make_test_actions();
        let map = ActionAliasMap::from_actions(&actions);
        // canonical 中文名也应查到自身
        assert_eq!(map.translate("说话"), Some("说话".to_string()));
        assert_eq!(map.translate("移动"), Some("移动".to_string()));
        assert_eq!(map.translate("休息"), Some("休息".to_string()));
    }

    #[test]
    fn test_action_alias_unknown_returns_none() {
        let actions = make_test_actions();
        let map = ActionAliasMap::from_actions(&actions);
        assert_eq!(map.translate("飞"), None);
        assert_eq!(map.translate("dance"), None);
    }

    #[test]
    fn test_field_alias_chinese_to_english() {
        let actions = make_test_actions();
        let map = FieldAliasMap::from_actions(&actions);

        let mut data = serde_json::json!({"内容": "各位好汉，在下有礼了。"});
        map.translate_data("说话", &mut data);
        assert_eq!(data["content"], "各位好汉，在下有礼了。");
        assert!(data.get("内容").is_none());
    }

    #[test]
    fn test_field_alias_move_target_location() {
        let actions = make_test_actions();
        let map = FieldAliasMap::from_actions(&actions);

        let mut data = serde_json::json!({"目标地点": "龙门厨房"});
        map.translate_data("移动", &mut data);
        assert_eq!(data["target_location"], "龙门厨房");
        assert!(data.get("目标地点").is_none());
    }

    #[test]
    fn test_field_alias_destination_to_target_location() {
        let actions = make_test_actions();
        let map = FieldAliasMap::from_actions(&actions);

        let mut data = serde_json::json!({"destination": "龙门厨房"});
        map.translate_data("移动", &mut data);
        assert_eq!(data["target_location"], "龙门厨房");
    }

    #[test]
    fn test_field_alias_item_id() {
        let actions = make_test_actions();
        let map = FieldAliasMap::from_actions(&actions);

        let mut data = serde_json::json!({"物品ID": "馒头"});
        map.translate_data("进食", &mut data);
        assert_eq!(data["item_id"], "馒头");
    }

    #[test]
    fn test_field_alias_unknown_action_noop() {
        let actions = make_test_actions();
        let map = FieldAliasMap::from_actions(&actions);

        let mut data = serde_json::json!({"内容": "hello"});
        map.translate_data("未知动作", &mut data);
        // 未知 action_type 不翻译，原样保留
        assert_eq!(data["内容"], "hello");
    }

    #[test]
    fn test_field_alias_no_required_fields_noop() {
        let actions = make_test_actions();
        let map = FieldAliasMap::from_actions(&actions);

        let mut data = serde_json::json!({"foo": "bar"});
        map.translate_data("休息", &mut data);
        // 休息 没有 required_fields，不翻译
        assert_eq!(data["foo"], "bar");
    }

    #[test]
    fn test_field_alias_canonical_key_preserved() {
        let actions = make_test_actions();
        let map = FieldAliasMap::from_actions(&actions);

        // 已是 canonical key，不应被修改
        let mut data = serde_json::json!({"content": "hello", "target_location": "inn"});
        map.translate_data("说话", &mut data);
        assert_eq!(data["content"], "hello");

        let mut data2 = serde_json::json!({"target_location": "inn"});
        map.translate_data("移动", &mut data2);
        assert_eq!(data2["target_location"], "inn");
    }

    #[test]
    fn test_field_alias_non_required_field_not_translated() {
        let actions = make_test_actions();
        let map = FieldAliasMap::from_actions(&actions);

        // "thought" 不是 说话 的 required_field，即使碰巧和某个别名同名也不翻译
        let mut data = serde_json::json!({"thought": "我要说话"});
        map.translate_data("说话", &mut data);
        assert_eq!(data["thought"], "我要说话");
    }

    // ========================================================================
    // EntityTranslationRegistry tests
    // ========================================================================

    #[test]
    fn test_registry_translate_target_location() {
        let registry = EntityTranslationRegistry {
            field_maps: vec![
                (
                    "target_location".to_string(),
                    EntityAliasMap::from_entries(vec![
                        (
                            "酒泉".to_string(),
                            vec!["jiuquan".to_string(), "绿洲".to_string()],
                        ),
                        ("龙门客栈".to_string(), vec!["longmen_inn".to_string()]),
                    ]),
                ),
                ("item_id".to_string(), EntityAliasMap::empty()),
                ("target_agent_id".to_string(), EntityAliasMap::empty()),
            ],
        };

        let mut data = serde_json::json!({"target_location": "jiuquan"});
        registry.translate(&mut data);
        assert_eq!(data["target_location"], "酒泉");

        let mut data2 = serde_json::json!({"target_location": "绿洲"});
        registry.translate(&mut data2);
        assert_eq!(data2["target_location"], "酒泉");
    }

    #[test]
    fn test_registry_translate_item_id() {
        let registry = EntityTranslationRegistry {
            field_maps: vec![
                ("target_location".to_string(), EntityAliasMap::empty()),
                (
                    "item_id".to_string(),
                    EntityAliasMap::from_entries(vec![
                        ("馒头".to_string(), vec!["mantou".to_string()]),
                        (
                            "水".to_string(),
                            vec!["water".to_string(), "水".to_string()],
                        ),
                    ]),
                ),
                ("target_agent_id".to_string(), EntityAliasMap::empty()),
            ],
        };

        let mut data = serde_json::json!({"item_id": "水"});
        registry.translate(&mut data);
        assert_eq!(data["item_id"], "水");
    }

    #[test]
    fn test_registry_translate_agent_name_to_uuid() {
        let registry = EntityTranslationRegistry {
            field_maps: vec![
                ("target_location".to_string(), EntityAliasMap::empty()),
                ("item_id".to_string(), EntityAliasMap::empty()),
                (
                    "target_agent_id".to_string(),
                    EntityAliasMap::from_entries(vec![
                        (
                            "cd6101be-868c-4c6b-bf17-47f5611f3aac".to_string(),
                            vec!["小鹿".to_string()],
                        ),
                        (
                            "82835b43-3ae8-495d-a350-8035883debd5".to_string(),
                            vec!["柳如烟".to_string()],
                        ),
                    ]),
                ),
            ],
        };

        // 中文名 → UUID
        let mut data = serde_json::json!({"target_agent_id": "小鹿", "content": "你好"});
        registry.translate(&mut data);
        assert_eq!(
            data["target_agent_id"],
            "cd6101be-868c-4c6b-bf17-47f5611f3aac"
        );
        assert_eq!(data["content"], "你好"); // content 不受影响

        // UUID 原样保留
        let mut data2 =
            serde_json::json!({"target_agent_id": "82835b43-3ae8-495d-a350-8035883debd5"});
        registry.translate(&mut data2);
        assert_eq!(
            data2["target_agent_id"],
            "82835b43-3ae8-495d-a350-8035883debd5"
        );
    }

    #[test]
    fn test_registry_unregistered_field_untouched() {
        let registry = EntityTranslationRegistry {
            field_maps: vec![
                ("target_location".to_string(), EntityAliasMap::empty()),
                ("item_id".to_string(), EntityAliasMap::empty()),
                ("target_agent_id".to_string(), EntityAliasMap::empty()),
            ],
        };

        let mut data = serde_json::json!({"content": "你好", "quantity": 5});
        registry.translate(&mut data);
        assert_eq!(data["content"], "你好");
        assert_eq!(data["quantity"], 5);
    }

    #[test]
    fn test_registry_multiple_fields_in_one_pass() {
        let registry = EntityTranslationRegistry {
            field_maps: vec![
                (
                    "target_location".to_string(),
                    EntityAliasMap::from_entries(vec![(
                        "龙门客栈".to_string(),
                        vec!["客栈".to_string()],
                    )]),
                ),
                (
                    "item_id".to_string(),
                    EntityAliasMap::from_entries(vec![(
                        "馒头".to_string(),
                        vec!["mantou".to_string()],
                    )]),
                ),
                (
                    "target_agent_id".to_string(),
                    EntityAliasMap::from_entries(vec![(
                        "uuid-123".to_string(),
                        vec!["赵万金".to_string()],
                    )]),
                ),
            ],
        };

        let mut data = serde_json::json!({
            "target_location": "客栈",
            "item_id": "mantou",
            "target_agent_id": "赵万金"
        });
        registry.translate(&mut data);
        assert_eq!(data["target_location"], "龙门客栈");
        assert_eq!(data["item_id"], "馒头");
        assert_eq!(data["target_agent_id"], "uuid-123");
    }
}
