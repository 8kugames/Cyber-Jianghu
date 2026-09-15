// ============================================================================
// 物品动作来源分类
// ============================================================================
//
// actor（chaos 生成）与 reflector（layer0 审查）共享的物品动作语义分类：
// 消耗/转出类（Inventory）物品必须来自背包，采集/拾取类（World）物品来自
// 地面/资源点/他人。分类口径与服务端执行语义对齐（ItemUsed/予 均按背包
// remove_item 校验），消除「审查放行、执行必败」的契约错位。
//
// 五原语内置权威映射优先（免疫本地 actions.json 陈旧缓存误判，如旧缓存
// 「取」缺 source_type 字段会被启发式误判为 Inventory）；新增动作走数据
// 启发式（required_fields 含 source_type → World，否则 Inventory）。
// ============================================================================

use cyber_jianghu_protocol::AvailableAction;

// 物品展示引用单一真源在 protocol（Server display_item_name / Agent 照抄指引同源）
pub use cyber_jianghu_protocol::{display_item_ref, short_item_hex};

/// 物品动作的物品来源语义
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemActionSource {
    /// 消耗/转出类（用/吃/喝/予）：物品须已在背包（或链内前序「取」获得）
    Inventory,
    /// 采集/拾取类（取）：物品来自地面/资源点/他人
    World,
    /// 非物品动作或数据缺失：维持历史宽口径（不收窄可见集合）
    Unknown,
}

/// 五原语权威映射（actions.yaml v2 原子动作，语义稳定契约）
fn builtin_source(action_type: &str) -> Option<ItemActionSource> {
    match action_type {
        "取" => Some(ItemActionSource::World),
        "用" | "吃" | "喝" | "予" => Some(ItemActionSource::Inventory),
        _ => None,
    }
}

/// 数据启发式（仅服务内置映射未覆盖的新增动作）
fn heuristic_source(action: &AvailableAction) -> ItemActionSource {
    let has_item_field = action
        .required_fields
        .iter()
        .chain(action.optional_fields.iter())
        .any(|f| f == "item_id");
    if !has_item_field {
        return ItemActionSource::Unknown;
    }
    // 携带 source_type 的物品动作即采集/拾取语义（source_type 字面即物品来源）
    if action.required_fields.iter().any(|f| f == "source_type") {
        ItemActionSource::World
    } else {
        ItemActionSource::Inventory
    }
}

/// 分类物品动作的物品来源语义
pub fn classify_item_action(action_type: &str, actions: &[AvailableAction]) -> ItemActionSource {
    if let Some(source) = builtin_source(action_type) {
        return source;
    }
    actions
        .iter()
        .find(|a| a.action == action_type || a.name == action_type)
        .map_or(ItemActionSource::Unknown, heuristic_source)
}

/// 判定动作是否携带物品目标（item_id 字段）。
///
/// 数据驱动：actions.json（Server 下发缓存）中 required/optional 字段含 item_id
/// 的动作自动纳入；内置五原语兜底保证校验不因配置缺失而失效。
/// （自 reflector/hard_logic.rs 迁入，与 classify 共享同一套判定，
/// 消除双份硬编码兜底清单的漂移风险）
pub fn is_item_action(action_type: &str) -> bool {
    classify_item_action(
        action_type,
        &crate::infra::api::cognitive_context::load_available_actions_from_file(),
    ) != ItemActionSource::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(name: &str, required: &[&str], optional: &[&str]) -> AvailableAction {
        AvailableAction {
            action: name.to_string(),
            name: name.to_string(),
            description: String::new(),
            category: String::new(),
            valid_targets: None,
            required_fields: required.iter().map(|s| s.to_string()).collect(),
            optional_fields: optional.iter().map(|s| s.to_string()).collect(),
            ooc_risk: Default::default(),
            requirements: Vec::new(),
            effects: Vec::new(),
        }
    }

    #[test]
    fn test_builtin_mapping_authoritative() {
        // 五原语内置映射优先，与动作数据是否在场无关
        let actions: Vec<AvailableAction> = Vec::new();
        assert_eq!(
            classify_item_action("取", &actions),
            ItemActionSource::World
        );
        for consume in ["用", "吃", "喝", "予"] {
            assert_eq!(
                classify_item_action(consume, &actions),
                ItemActionSource::Inventory
            );
        }
        // 非物品动作
        assert_eq!(
            classify_item_action("观察", &actions),
            ItemActionSource::Unknown
        );
    }

    #[test]
    fn test_builtin_overrides_stale_cache() {
        // 陈旧缓存（取 缺 source_type 字段）不得把内置「取」误判为 Inventory
        let stale = vec![action("取", &["item_id", "quantity"], &[])];
        assert_eq!(classify_item_action("取", &stale), ItemActionSource::World);
    }

    #[test]
    fn test_short_item_hex() {
        let uuid = cyber_jianghu_protocol::item_uuid("馒头").to_string();
        assert_eq!(short_item_hex(&uuid), &uuid[..8]);
        // 非 uuid 形态（测试夹具遗留的英文 id）原样返回
        assert_eq!(short_item_hex("mantou"), "mantou");
        // 空/短串安全
        assert_eq!(short_item_hex(""), "");
    }

    #[test]
    fn test_display_item_ref() {
        let uuid = cyber_jianghu_protocol::item_uuid("水").to_string();
        assert_eq!(display_item_ref("水", &uuid), format!("水[{}]", &uuid[..8]));
    }

    #[test]
    fn test_heuristic_for_new_actions() {
        // 新动作：含 item_id + source_type → World
        let world_like = vec![action("捕猎", &["source_type", "item_id", "quantity"], &[])];
        assert_eq!(
            classify_item_action("捕猎", &world_like),
            ItemActionSource::World
        );
        // 新动作：仅 item_id → Inventory
        let inv_like = vec![action("研墨", &["item_id"], &[])];
        assert_eq!(
            classify_item_action("研墨", &inv_like),
            ItemActionSource::Inventory
        );
        // 新动作：item_id 仅在 optional → Inventory
        let opt_like = vec![action("鉴赏", &[], &["item_id"])];
        assert_eq!(
            classify_item_action("鉴赏", &opt_like),
            ItemActionSource::Inventory
        );
        // 数据中不存在的动作 → Unknown
        assert_eq!(
            classify_item_action("御剑", &inv_like),
            ItemActionSource::Unknown
        );
    }
}
