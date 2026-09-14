// ============================================================================
// 天魂 Layer 0：硬性逻辑审查（目标可见性）
// ============================================================================
//
// 从 validator.rs 拆出的确定性目标校验（无 LLM 参与），
// 由 validator.rs 的 validate_pipeline 作为第一步调用。
// ============================================================================

use super::types::ValidationRequest;
use crate::soul::item_source::{ItemActionSource, classify_item_action};
use cyber_jianghu_protocol::WorldBuildingRules;

/// Layer 0：硬性逻辑审查（目标可见性，纯确定性检查，无 LLM 参与）
///
/// 天魂审查第一步，在任何 action_type / 规则引擎 / LLM 审查之前拦截 LLM 臆测的目标引用：
/// - 人物目标（target_agent_id / source_id / recipient_id）必须在感知范围内（entities）
/// - 物品目标（item_id）按动作来源分类（soul/item_source）：
///   - 消耗/转出类（用/吃/喝/予）必须持有（inventory）或链内前序已验证"取"动作
///     获得（acquired_item_ids）——与服务端执行语义对齐（ItemUsed/予 均按背包
///     remove_item 校验），引用地面物品的消耗意图在服务端必回滚
///   - 采集/拾取类（取）及未知动作维持宽可见集合（持有 ∨ nearby ∨ gatherable ∨
///     链内获得物）
///   - 例外：取-agent 的目标在对方背包中，本方不可观察，仅保留世界定义存在性检查
///
/// 口径分层记录（接替旧 Layer2 权衡注释）：subsequent_intents 链内逐个验证共享
/// 同一 WorldState 快照，可见性校验对链内后序 intent 有误拦风险。取(ground/
/// resource/agent)后即用经 acquired_item_ids 由调用方注入（主循环与自纠分支
/// 双入口）；制造→用成品链因产出物需反查配方暂不覆盖，由 self-correct 兜底、
/// 下一 tick 快照刷新后自愈。
///
/// world_state 缺失时跳过（无法校验，与 Layer 2 行为一致）
pub(super) async fn validate_hard_targets(
    request: &mut ValidationRequest,
    rules: &std::sync::Arc<tokio::sync::RwLock<WorldBuildingRules>>,
) -> std::result::Result<(), String> {
    let Some(world_state) = request.world_state.as_ref() else {
        return Ok(());
    };

    let action_type = request.intent.action_type.as_str();
    let action_data = request.intent.action_data.as_ref();
    let nearby_ids: Vec<uuid::Uuid> = world_state.entities.iter().map(|e| e.id).collect();
    let nearby_names: Vec<String> = world_state
        .entities
        .iter()
        .map(|e| format!("{} ({})", e.name, cyber_jianghu_protocol::short_id(&e.id)))
        .collect();

    // 人物目标：必须在范围内可见（说话/观察/攻击/教导 的 target_agent_id）
    if let Some(target_id) = action_data
        .and_then(|d| d.get("target_agent_id"))
        .and_then(|v| v.as_str())
        && let Err(e) = cyber_jianghu_protocol::resolve_agent_id(target_id, &nearby_ids)
    {
        return Err(format_target_rejection(
            target_id,
            e,
            &nearby_names,
            "目标角色",
        ));
    }

    // 取-agent：source_id 必填且在附近
    if action_type == "取"
        && action_data
            .and_then(|d| d.get("source_type"))
            .and_then(|v| v.as_str())
            == Some("agent")
    {
        match action_data
            .and_then(|d| d.get("source_id"))
            .and_then(|v| v.as_str())
        {
            None => return Err("取(从角色获取)必须指定 source_id".to_string()),
            Some(id) => {
                if let Err(e) = cyber_jianghu_protocol::resolve_agent_id(id, &nearby_ids) {
                    return Err(format_target_rejection(id, e, &nearby_names, "来源角色"));
                }
            }
        }
    }

    // 予-agent：recipient_id 必填且在附近
    if action_type == "予"
        && action_data
            .and_then(|d| d.get("recipient_type"))
            .and_then(|v| v.as_str())
            == Some("agent")
    {
        match action_data
            .and_then(|d| d.get("recipient_id"))
            .and_then(|v| v.as_str())
        {
            None => return Err("予(给角色)必须指定 recipient_id".to_string()),
            Some(id) => {
                if let Err(e) = cyber_jianghu_protocol::resolve_agent_id(id, &nearby_ids) {
                    return Err(format_target_rejection(id, e, &nearby_names, "目标角色"));
                }
            }
        }
    }

    // 物品目标：按动作来源分类（用/吃/喝/取/予 等 item_id 动作，
    // 分类器含 actions.json 数据驱动 + 五原语内置兜底，见 soul/item_source）
    // 引用体系（全面 uuid 化）：协议字段携带完整 uuid（v5 从裸 item_id 派生），
    // LLM 可能提交三种形态，统一规范化为完整 uuid 回写：
    // 1. 完整 uuid（从背包/prompt 复制）→ 直接可见性校验
    // 2. `名称[短uuid]`（照抄观察/执行文本）→ 一致性校验后派生完整 uuid
    // 3. 裸 item_id（纯名字）→ 派生完整 uuid（known_item_ids 存在性仍适用）
    let item_source = classify_item_action(
        action_type,
        &crate::infra::api::cognitive_context::load_available_actions_from_file(),
    );
    if item_source == ItemActionSource::Unknown {
        return Ok(());
    }
    let Some(raw_item_id) = action_data
        .and_then(|d| d.get("item_id"))
        .and_then(|v| v.as_str())
    else {
        return Ok(());
    };

    let normalized_item_id: String;
    let mut known_item_name: Option<String> = None;
    if let Ok(full_uuid) = uuid::Uuid::parse_str(raw_item_id) {
        // 形态 1：完整 uuid。known_item_ids 非空时校验 uuid 必须派生自已知物品
        //（覆盖取-agent 例外路径——臆造 uuid 不能因可见性豁免而绕过存在性拦截）
        let known = rules.read().await.known_item_ids.clone();
        if !known.is_empty()
            && !known
                .iter()
                .any(|k| cyber_jianghu_protocol::item_uuid(k) == full_uuid)
        {
            return Err(format!(
                "物品 uuid '{}' 不存在于世界物品定义中，属于臆造引用。请从观察结果或背包列表中照抄真实物品标识",
                raw_item_id
            ));
        }
        normalized_item_id = full_uuid.to_string();
    } else {
        // 形态 2/3：剥 `名称[短uuid]` 后缀或接受裸名，派生完整 uuid
        let (bare, short_uuid) = cyber_jianghu_protocol::parse_item_ref(raw_item_id);

        // uuid 一致性：短 uuid 必须与裸名派生结果一致，否则为臆造/篡造引用
        if let Some(short) = &short_uuid
            && cyber_jianghu_protocol::short_id(&cyber_jianghu_protocol::item_uuid(&bare)) != *short
        {
            return Err(format!(
                "物品 '{}' 的 uuid 与名称不符，属于臆造引用。请从观察结果或背包列表中照抄真实物品标识",
                raw_item_id
            ));
        }
        known_item_name = Some(bare.clone());
        normalized_item_id = cyber_jianghu_protocol::item_uuid(&bare).to_string();
    }
    let item_id = normalized_item_id;

    // 存在性：世界定义中不存在的物品 = LLM 臆造。
    // 仅对裸名/名称[短uuid] 形态适用（known_item_ids 为裸名清单，
    // 完整 uuid 形态已在形态 1 分支用派生 uuid 集合校验）
    if let Some(bare) = &known_item_name {
        let known = rules.read().await.known_item_ids.clone();
        if !known.is_empty() && !known.contains(bare) {
            let preview = known.iter().take(5).cloned().collect::<Vec<_>>().join(", ");
            return Err(format!(
                "物品「{}」不存在于世界物品定义中。合法物品: [{}]",
                bare, preview
            ));
        }
    }

    // 可见性例外标志：取-agent 的目标在对方背包中，本方不可观察
    //（先于回写计算，结束 action_data 不可变借用）
    let from_other_agent = action_type == "取"
        && action_data
            .and_then(|d| d.get("source_type"))
            .and_then(|v| v.as_str())
            == Some("agent");

    // 规范化回写：统一为完整 uuid（Server 端动作边界按 uuid 反解内部 item_id，
    // 裸 id 引用已被拒绝）。所有放行路径（含取-agent 例外）均需回写，
    // 保证到达 Server 的 item_id 字段始终是完整 uuid。
    if let Some(obj) = request
        .intent
        .action_data
        .as_mut()
        .and_then(|d| d.as_object_mut())
    {
        obj.insert(
            "item_id".to_string(),
            serde_json::Value::String(item_id.clone()),
        );
    }

    if from_other_agent {
        return Ok(());
    }

    // 消耗/转出类（用/吃/喝/予）：物品必须已持有（背包 ∨ 链内前序「取」获得）。
    // 引用地面/资源点物品的消耗意图在服务端必因背包无货回滚（ItemUsed 按
    // remove_item 校验），此处前置拦截与执行语义对齐。
    if item_source == ItemActionSource::Inventory {
        let owned = world_state
            .self_state
            .inventory
            .iter()
            .any(|i| i.item_id == item_id)
            || request
                .runtime
                .acquired_item_ids
                .iter()
                .any(|id| id == &item_id);
        if owned {
            return Ok(());
        }
        let inventory_preview: Vec<&str> = world_state
            .self_state
            .inventory
            .iter()
            .take(5)
            .map(|i| i.item_id.as_str())
            .collect();
        let nearby_preview: Vec<&str> = world_state
            .nearby_items
            .iter()
            .take(5)
            .map(|i| i.item_id.as_str())
            .collect();
        return Err(format!(
            "物品「{}」不可见：{}需要背包中已持有的物品（附近物品须先「取」入背包）。你的背包: [{}]，附近可见: [{}]",
            item_id,
            action_type,
            inventory_preview.join(", "),
            nearby_preview.join(", ")
        ));
    }

    // 可见集合（采集/拾取类与未知动作，维持宽口径）：
    // 持有 ∨ 附近 ∨ 本地点可采集 ∨ 链内前序已验证"取"动作获得
    //（链内共享快照，制造→用成品链暂不覆盖：产出物需查配方，由 self-correct
    //  兜底，下一 tick 快照刷新后自愈——已知局限，勿静默移除本注释）
    let visible = world_state
        .self_state
        .inventory
        .iter()
        .any(|i| i.item_id == item_id)
        || world_state
            .nearby_items
            .iter()
            .any(|i| i.item_id == item_id)
        || world_state
            .location
            .gatherable_items
            .iter()
            .any(|g| g.item_id == item_id)
        || request
            .runtime
            .acquired_item_ids
            .iter()
            .any(|id| id == &item_id);
    if visible {
        return Ok(());
    }

    // 拒绝消息列出全部可观察物品，引导 LLM 改用真实目标
    let inventory_preview: Vec<&str> = world_state
        .self_state
        .inventory
        .iter()
        .take(5)
        .map(|i| i.item_id.as_str())
        .collect();
    let nearby_preview: Vec<&str> = world_state
        .nearby_items
        .iter()
        .take(5)
        .map(|i| i.item_id.as_str())
        .collect();
    let gatherable_preview: Vec<&str> = world_state
        .location
        .gatherable_items
        .iter()
        .take(5)
        .map(|g| g.item_id.as_str())
        .collect();
    Err(format!(
        "物品「{}」不可见：既不在你的背包中，也不在附近或本地点可采集。你的背包: [{}]，附近物品: [{}]，可采集: [{}]",
        item_id,
        inventory_preview.join(", "),
        nearby_preview.join(", "),
        gatherable_preview.join(", ")
    ))
}

/// 格式化目标存在性校验的拒绝消息（复用现有 target_agent_id 校验的三分支格式）
fn format_target_rejection(
    target_id: &str,
    err: cyber_jianghu_protocol::ResolveAgentIdError,
    nearby_names: &[String],
    label: &str,
) -> String {
    use cyber_jianghu_protocol::ResolveAgentIdError;
    match err {
        ResolveAgentIdError::Ambiguous { matched, .. } => {
            format!(
                "{} ID '{}' 匹配到多个角色，请从匹配结果中照抄更长的完整 ID。匹配结果: [{}]。当前附近的角色: [{}]",
                label,
                target_id,
                matched
                    .iter()
                    .map(cyber_jianghu_protocol::short_id)
                    .collect::<Vec<_>>()
                    .join(", "),
                nearby_names.join(", ")
            )
        }
        _ => {
            // 混合自纠：确定性指引随拒绝消息注入，使同 tick 自纠与跨 tick 决策
            // 直接获得「换前置动作」的纠错方向，而非重复对不可见目标提交同类动作
            let guidance = if nearby_names.is_empty() {
                "当前位置看不到任何其他角色，任何针对特定角色的动作（说话/予/取/教导）都无法执行。请改用【移动】前往相邻场景寻找目标，或【观察】确认环境后再行动，不要对不可见目标重复提交同类动作。"
            } else {
                "若要与某人交互，请从上方列表照抄其完整 ID。"
            };
            format!(
                "{} {} 不在附近实体中。当前附近的角色: [{}]。{}",
                label,
                target_id,
                nearby_names.join(", "),
                guidance
            )
        }
    }
}
