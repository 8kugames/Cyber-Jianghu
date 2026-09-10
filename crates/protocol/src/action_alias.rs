//! 动作别名规范化（LLM 边界共享真源：Server 与 Agent 使用同一映射）
//!
//! LLM（尤其是非头部模型）会输出语义化/英文动作名（如 `idle`、`进食`、`给予`），
//! 而动作注册表使用 canonical 单字键（`休整`、`用`、`予`）。本模块在认知边界
//! 将别名规范化为 canonical 键，并注入映射动作所需的缺省 action_data 字段
//! （如 采集 → 取 需补 `source_type: "resource"`）。
//!
//! 已在注册表中的 canonical 键原样通过，不做任何改写。

use serde_json::Value;

/// 动作别名 → (canonical action_type, 需注入的缺省字段)
///
/// 注入仅在对应字段缺失时生效，不覆盖 LLM 的显式值。
/// 缺省字段存 &str（const 友好），运行时转为 JSON 值。
/// 缺省字段：动作别名映射到 canonical 键时需注入的 action_data 字段
/// （字段名, 字符串值）。
type ActionDefaults<'a> = &'a [(&'a str, &'a str)];
type AliasEntry = (&'static str, &'static str, ActionDefaults<'static>);
const ALIAS_MAP: &[AliasEntry] = &[
    // ── 用 系（进食/饮水/使用道具）──
    ("进食", "吃", &[]),
    ("饮水", "喝", &[]),
    ("使用", "用", &[]),
    ("eat", "吃", &[]),
    ("drink", "喝", &[]),
    ("use", "用", &[]),
    // ── 予 系（给角色 / 丢地面）──
    ("给予", "予", &[("recipient_type", "agent")]),
    ("丢弃", "予", &[("recipient_type", "ground")]),
    ("give", "予", &[("recipient_type", "agent")]),
    ("discard", "予", &[("recipient_type", "ground")]),
    // ── 取 系（拾地面 / 采资源 / 从角色获取）──
    ("拾取", "取", &[("source_type", "ground")]),
    ("采集", "取", &[("source_type", "resource")]),
    ("偷窃", "取", &[("source_type", "agent")]),
    ("pick", "取", &[("source_type", "ground")]),
    ("gather", "取", &[("source_type", "resource")]),
    ("steal", "取", &[("source_type", "agent")]),
    // ── 说话 系（公开 / 私语 / 大喊）──
    ("私语", "说话", &[("channel", "private")]),
    ("大喊", "说话", &[("channel", "broadcast")]),
    ("speak", "说话", &[]),
    ("talk", "说话", &[]),
    ("whisper", "说话", &[("channel", "private")]),
    ("shout", "说话", &[("channel", "broadcast")]),
    // ── 休整 系（LLM 常见的英文/语义变体）──
    ("休息", "休整", &[]),
    ("打坐", "休整", &[]),
    ("修炼", "休整", &[]),
    ("idle", "休整", &[]),
    ("rest", "休整", &[]),
    ("meditate", "休整", &[]),
    // ── 其余动作的英文变体 ──
    ("传授", "教导", &[]),
    ("teach", "教导", &[]),
    ("craft", "制造", &[]),
    ("move", "移动", &[]),
    ("observe", "观察", &[]),
    ("attack", "攻击", &[]),
];

/// 将 LLM 输出的动作名规范化为 canonical 键。
///
/// - `action_type`: LLM 输出的动作名（原样比较，大小写不敏感以兼容英文变体）
/// - `action_data`: 原位注入映射动作的缺省字段（仅在字段缺失时）
///
/// 返回规范化后的 action_type；无别名命中时原样返回输入。
pub fn normalize_action_type(action_type: &str, action_data: &mut Option<Value>) -> String {
    let input = action_type.trim();
    let lowered = input.to_lowercase();

    let canonical = ALIAS_MAP
        .iter()
        .find(|(alias, _, _)| *alias == input || *alias == lowered);

    let Some((_, canonical_type, defaults)) = canonical else {
        return input.to_string();
    };

    if !defaults.is_empty() {
        let data = action_data.get_or_insert_with(|| Value::Object(serde_json::Map::new()));
        if let Some(obj) = data.as_object_mut() {
            for (key, value) in *defaults {
                obj.entry(key.to_string())
                    .or_insert_with(|| Value::String(value.to_string()));
            }
        }
    }

    canonical_type.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_canonical_passes_through() {
        let mut data = Some(json!({"item_id": "x"}));
        assert_eq!(normalize_action_type("用", &mut data), "用");
        assert_eq!(normalize_action_type("休整", &mut data), "休整");
        assert_eq!(normalize_action_type("教导", &mut data), "教导");
        // canonical 动作不注入任何字段
        assert_eq!(data, Some(json!({"item_id": "x"})));
    }

    #[test]
    fn test_alias_to_canonical() {
        let mut idle_data = None;
        assert_eq!(normalize_action_type("idle", &mut idle_data), "休整");
        assert_eq!(normalize_action_type("进食", &mut None), "吃");
        assert_eq!(normalize_action_type("饮水", &mut None), "喝");
        assert_eq!(normalize_action_type("传授", &mut None), "教导");
    }

    #[test]
    fn test_case_insensitive_english() {
        let mut data = None;
        assert_eq!(normalize_action_type("Idle", &mut data), "休整");
        assert_eq!(normalize_action_type("GATHER", &mut data), "取");
    }

    #[test]
    fn test_injects_default_fields_only_when_missing() {
        // 采集 → 取(resource)，注入 source_type
        let mut data = Some(json!({"item_id": "abc"}));
        assert_eq!(normalize_action_type("采集", &mut data), "取");
        assert_eq!(
            data,
            Some(json!({"item_id": "abc", "source_type": "resource"}))
        );

        // 已有显式值不被覆盖
        let mut data = Some(json!({"source_type": "ground"}));
        normalize_action_type("采集", &mut data);
        assert_eq!(
            data,
            Some(json!({"source_type": "ground"})),
            "LLM 显式字段不被注入覆盖"
        );

        // 无 action_data 时创建
        let mut data = None;
        normalize_action_type("拾取", &mut data);
        assert_eq!(data, Some(json!({"source_type": "ground"})));
    }

    #[test]
    fn test_unknown_action_passthrough() {
        let mut data = Some(json!({"foo": 1}));
        assert_eq!(normalize_action_type("飞天遁地", &mut data), "飞天遁地");
        assert_eq!(data, Some(json!({"foo": 1})));
    }

    #[test]
    fn test_whisper_shout_channel_injection() {
        let mut data = None;
        assert_eq!(normalize_action_type("私语", &mut data), "说话");
        assert_eq!(data, Some(json!({"channel": "private"})));

        let mut data = None;
        assert_eq!(normalize_action_type("大喊", &mut data), "说话");
        assert_eq!(data, Some(json!({"channel": "broadcast"})));
    }
}
