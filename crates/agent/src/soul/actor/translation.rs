// ============================================================================
// 中文 LLM 边界翻译层
// ============================================================================
//
// 薄翻译层：LLM 输出中文 action_type 和 action_data key → 英文 canonical
// 翻译硬边界：必须在 ReflectorSoul 之前完成
// 数据驱动：映射来自 AvailableAction 的 aliases/field_aliases，零硬编码
// ============================================================================

use cyber_jianghu_protocol::AvailableAction;
use std::collections::HashMap;

/// action_type 别名映射: alias (lowercase) → canonical english key
///
/// 包含 action_type 自身（英文 canonical 也可查到自身）
pub struct ActionAliasMap {
    /// alias (lowercase) → canonical english key
    forward: HashMap<String, String>,
    /// canonical english key → chinese name (用于 prompt 展示)
    chinese_names: HashMap<String, String>,
}

/// action_data 字段别名映射: (action_type, field_alias) → canonical field
///
/// 按 action_type 隔离，仅翻译该 action 的 required_fields 对应的别名
pub struct FieldAliasMap(HashMap<String, HashMap<String, String>>);

impl ActionAliasMap {
    /// 从 AvailableAction list 构建
    ///
    /// 构建 alias → canonical 映射：
    /// - 每个动作的 `action` (英文 canonical) 映射到自身
    /// - 每个动作的 `name` (中文名) 映射到 canonical
    /// - 每个别名映射到 canonical
    /// - 所有 key 统一转小写以支持大小写不敏感匹配
    pub fn from_actions(actions: &[AvailableAction]) -> Self {
        let mut forward = HashMap::new();
        let mut chinese_names = HashMap::new();
        for a in actions {
            let canonical = a.action.to_lowercase();
            // canonical english key → self
            forward.entry(canonical.clone())
                .or_insert_with(|| a.action.clone());
            // chinese name → canonical
            if !a.name.is_empty() {
                forward.insert(a.name.to_lowercase(), a.action.clone());
            }
            // aliases → canonical
            for alias in &a.aliases {
                forward.insert(alias.to_lowercase(), a.action.clone());
            }
            // english canonical → chinese name
            if !a.name.is_empty() {
                chinese_names.insert(a.action.to_lowercase(), a.name.clone());
            }
        }
        Self { forward, chinese_names }
    }

    /// 翻译 action_type（中文/别名 → 英文 canonical）
    ///
    /// 查找顺序:
    /// 1. 精确匹配（lowercase）
    /// 2. 未匹配时返回 None（fail-fast，由调用方决定处理）
    pub fn translate(&self, input: &str) -> Option<String> {
        self.forward.get(&input.to_lowercase()).cloned()
    }

    /// 反向查找：英文 canonical → 中文名（用于 prompt 展示）
    pub fn chinese_name(&self, action_type: &str) -> String {
        self.chinese_names
            .get(&action_type.to_lowercase())
            .cloned()
            .unwrap_or_else(|| action_type.to_string())
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

    /// 翻译 action_data 的 key（中文/别名 → 英文 canonical）
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
                action: "idle".to_string(),
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
                action: "speak".to_string(),
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
                action: "move".to_string(),
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
                action: "eat".to_string(),
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
    fn test_action_alias_chinese_name() {
        let actions = make_test_actions();
        let map = ActionAliasMap::from_actions(&actions);
        assert_eq!(map.translate("说话"), Some("speak".to_string()));
        assert_eq!(map.translate("休息"), Some("idle".to_string()));
        assert_eq!(map.translate("移动"), Some("move".to_string()));
        assert_eq!(map.translate("进食"), Some("eat".to_string()));
    }

    #[test]
    fn test_action_alias_chinese_variant() {
        let actions = make_test_actions();
        let map = ActionAliasMap::from_actions(&actions);
        assert_eq!(map.translate("交谈"), Some("speak".to_string()));
        assert_eq!(map.translate("静修"), Some("idle".to_string()));
        assert_eq!(map.translate("行走"), Some("move".to_string()));
        assert_eq!(map.translate("吃"), Some("eat".to_string()));
    }

    #[test]
    fn test_action_alias_english_typo() {
        let actions = make_test_actions();
        let map = ActionAliasMap::from_actions(&actions);
        assert_eq!(map.translate("say"), Some("speak".to_string()));
        assert_eq!(map.translate("Speak"), Some("speak".to_string())); // case insensitive
    }

    #[test]
    fn test_action_alias_canonical_pass_through() {
        let actions = make_test_actions();
        let map = ActionAliasMap::from_actions(&actions);
        // 英文 canonical 也应查到自身
        assert_eq!(map.translate("speak"), Some("speak".to_string()));
        assert_eq!(map.translate("move"), Some("move".to_string()));
        assert_eq!(map.translate("idle"), Some("idle".to_string()));
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
        map.translate_data("speak", &mut data);
        assert_eq!(data["content"], "各位好汉，在下有礼了。");
        assert!(data.get("内容").is_none());
    }

    #[test]
    fn test_field_alias_move_target_location() {
        let actions = make_test_actions();
        let map = FieldAliasMap::from_actions(&actions);

        let mut data = serde_json::json!({"目标地点": "longmen_kitchen"});
        map.translate_data("move", &mut data);
        assert_eq!(data["target_location"], "longmen_kitchen");
        assert!(data.get("目标地点").is_none());
    }

    #[test]
    fn test_field_alias_destination_to_target_location() {
        let actions = make_test_actions();
        let map = FieldAliasMap::from_actions(&actions);

        let mut data = serde_json::json!({"destination": "longmen_kitchen"});
        map.translate_data("move", &mut data);
        assert_eq!(data["target_location"], "longmen_kitchen");
    }

    #[test]
    fn test_field_alias_item_id() {
        let actions = make_test_actions();
        let map = FieldAliasMap::from_actions(&actions);

        let mut data = serde_json::json!({"物品ID": "mantou"});
        map.translate_data("eat", &mut data);
        assert_eq!(data["item_id"], "mantou");
    }

    #[test]
    fn test_field_alias_unknown_action_noop() {
        let actions = make_test_actions();
        let map = FieldAliasMap::from_actions(&actions);

        let mut data = serde_json::json!({"内容": "hello"});
        map.translate_data("unknown_action", &mut data);
        // 未知 action_type 不翻译，原样保留
        assert_eq!(data["内容"], "hello");
    }

    #[test]
    fn test_field_alias_no_required_fields_noop() {
        let actions = make_test_actions();
        let map = FieldAliasMap::from_actions(&actions);

        let mut data = serde_json::json!({"foo": "bar"});
        map.translate_data("idle", &mut data);
        // idle 没有 required_fields，不翻译
        assert_eq!(data["foo"], "bar");
    }

    #[test]
    fn test_field_alias_canonical_key_preserved() {
        let actions = make_test_actions();
        let map = FieldAliasMap::from_actions(&actions);

        // 已是 canonical key，不应被修改
        let mut data = serde_json::json!({"content": "hello", "target_location": "inn"});
        map.translate_data("speak", &mut data);
        assert_eq!(data["content"], "hello");

        let mut data2 = serde_json::json!({"target_location": "inn"});
        map.translate_data("move", &mut data2);
        assert_eq!(data2["target_location"], "inn");
    }

    #[test]
    fn test_field_alias_non_required_field_not_translated() {
        let actions = make_test_actions();
        let map = FieldAliasMap::from_actions(&actions);

        // "thought" 不是 speak 的 required_field，即使碰巧和某个别名同名也不翻译
        let mut data = serde_json::json!({"thought": "我要说话"});
        map.translate_data("speak", &mut data);
        assert_eq!(data["thought"], "我要说话");
    }
}
