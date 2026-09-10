//! ID 解析工具（共享真源：Server 与 Agent 必须使用同一派生算法）
//!
//! - Agent ID：UUID prefix 匹配，降低 LLM 输出长 UUID 的准确性压力
//! - 物品 ID：UUID v5 确定性派生与 `名称[短uuid]` 展示格式解析

use uuid::Uuid;

/// 从候选列表中解析 agent ID
///
/// 匹配策略 (按优先级):
/// 1. 完整 UUID 精确匹配 (`Uuid::parse_str` 成功)
/// 2. UUID prefix 模糊匹配 (输入是合法 hex，且仅匹配到一个候选)
///
/// # 参数
/// - `input`: LLM 输出的 target_agent_id (完整 UUID 或 prefix)
/// - `candidates`: 候选 agent UUID 列表
///
/// # 错误
/// - `Err(ResolveAgentIdError::InvalidFormat)`: 输入不是合法 hex
/// - `Err(ResolveAgentIdError::NotFound)`: 无匹配
/// - `Err(ResolveAgentIdError::Ambiguous { matches })`: prefix 匹配到多个候选
pub fn resolve_agent_id(input: &str, candidates: &[Uuid]) -> Result<Uuid, ResolveAgentIdError> {
    let input = input.trim();

    // 1. 完整 UUID 精确匹配
    if let Ok(uuid) = Uuid::parse_str(input) {
        return Ok(uuid);
    }

    // 2. prefix 匹配 — 输入必须是合法 hex
    let input_lower = input.to_lowercase();
    if !input_lower.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ResolveAgentIdError::InvalidFormat {
            input: input.to_string(),
        });
    }

    let matched: Vec<Uuid> = candidates
        .iter()
        .filter(|uuid| uuid.to_string().starts_with(&input_lower))
        .copied()
        .collect();

    match matched.len() {
        0 => Err(ResolveAgentIdError::NotFound {
            input: input.to_string(),
        }),
        1 => Ok(matched[0]),
        _ => Err(ResolveAgentIdError::Ambiguous {
            input: input.to_string(),
            matched,
        }),
    }
}

/// 从候选列表中解析 agent ID (不带歧义检测，返回第一个匹配)
///
/// 用于 `Uuid::parse_str` 的直接替代场景，行为与 `resolve_agent_id` 相同，
/// 但在 prefix 匹配到多个候选时不报错，返回第一个匹配项。
///
/// 适用场景: prefix 已知无碰撞 (如 8 位 hex 在 <1000 agent 下)。
pub fn resolve_agent_id_lenient(input: &str, candidates: &[Uuid]) -> Option<Uuid> {
    let input = input.trim();

    // 1. 完整 UUID 精确匹配
    if let Ok(uuid) = Uuid::parse_str(input) {
        return Some(uuid);
    }

    // 2. prefix 匹配
    let input_lower = input.to_lowercase();
    if !input_lower.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }

    candidates
        .iter()
        .find(|uuid| uuid.to_string().starts_with(&input_lower))
        .copied()
}

/// 返回 UUID 的短 ID (前 8 位 hex)
///
/// 用于 LLM prompt 中显示，降低 token 消耗和复制错误率。
pub fn short_id(uuid: &Uuid) -> String {
    uuid.to_string()
        .split('-')
        .next()
        .unwrap_or_default()
        .to_string()
}

// ============================================================================
// 物品标识解析
// ============================================================================

/// 物品 uuid 派生命名空间（项目专用固定值，ASCII "cjh-item-uuid-v1"）。
///
/// 物品是数据驱动的"类型"而非实例，uuid 用 UUID v5（SHA-1）从 item_id
/// 确定性派生：同一 item_id 在任何部署、任何重启下得到同一 uuid，
/// 零配置、零迁移、天然防碰撞。
pub const ITEM_UUID_NAMESPACE: Uuid = Uuid::from_u128(0x636a_682d_6974_656d_2d75_7569_642d_7631);

/// 获取物品的稳定 uuid（UUID v5，从 item_id 确定性派生）。
///
/// 无需查表：任何 item_id（含未注册的）都可派生；
/// 展示层取前 8 位作为短 uuid。
pub fn item_uuid(item_id: &str) -> Uuid {
    Uuid::new_v5(&ITEM_UUID_NAMESPACE, item_id.as_bytes())
}

/// 解析 `名称[短uuid]` 形态的物品引用（观察结果与执行消息的展示格式）。
///
/// LLM 会照抄展示文本中的 `刀[1a2b3c4d]` 作为 item_id，此函数拆出裸名称与短 uuid。
/// 输入不含合法后缀时原样返回裸名称、uuid 为 None。
///
/// 短 uuid 与名称的匹配性校验由调用方执行（`item_uuid(bare)` 前 8 位比对），
/// 不匹配即臆造/篡造，应拦截。
pub fn parse_item_ref(input: &str) -> (String, Option<String>) {
    let trimmed = input.trim();
    if let Some(open) = trimmed.rfind('[')
        && trimmed.ends_with(']')
        && let Some(short) = trimmed.get(open + 1..trimmed.len() - 1)
        && short.len() == 8
        && short.chars().all(|c| c.is_ascii_hexdigit())
    {
        return (trimmed[..open].to_string(), Some(short.to_lowercase()));
    }
    (trimmed.to_string(), None)
}

/// Agent ID 解析错误
#[derive(Debug, Clone, thiserror::Error)]
pub enum ResolveAgentIdError {
    #[error("无效的 agent ID 格式: '{input}'")]
    InvalidFormat { input: String },

    #[error("未找到匹配的 agent ID: '{input}'")]
    NotFound { input: String },

    #[error("agent ID prefix '{input}' 匹配到多个候选: {}", matched.iter().map(short_id).collect::<Vec<_>>().join(", "))]
    Ambiguous { input: String, matched: Vec<Uuid> },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uuid_from_hex(hex: &str) -> Uuid {
        // 构造测试用 UUID: 用 hex 填充前 8 位，其余填 0
        let padded = format!("{}-0000-0000-0000-000000000000", hex);
        Uuid::parse_str(&padded).unwrap()
    }

    #[test]
    fn test_resolve_full_uuid() {
        let id = uuid_from_hex("a65df604");
        let candidates = vec![id, uuid_from_hex("b75b7479")];

        let result = resolve_agent_id(&id.to_string(), &candidates);
        assert_eq!(result.unwrap(), id);
    }

    #[test]
    fn test_resolve_prefix_8() {
        let id = uuid_from_hex("a65df604");
        let candidates = vec![id, uuid_from_hex("b75b7479")];

        let result = resolve_agent_id("a65df604", &candidates);
        assert_eq!(result.unwrap(), id);
    }

    #[test]
    fn test_resolve_prefix_4() {
        let id1 = uuid_from_hex("a65df604");
        let id2 = uuid_from_hex("a65db704");
        let candidates = vec![id1, id2];

        // "a65d" 匹配到两个 → 歧义
        let result = resolve_agent_id("a65d", &candidates);
        assert!(matches!(result, Err(ResolveAgentIdError::Ambiguous { .. })));
    }

    #[test]
    fn test_resolve_prefix_unique() {
        let id = uuid_from_hex("a65df604");
        let candidates = vec![id, uuid_from_hex("b75b7479")];

        // "a65d" 仅匹配一个
        let result = resolve_agent_id("a65d", &candidates);
        assert_eq!(result.unwrap(), id);
    }

    #[test]
    fn test_resolve_not_found() {
        let candidates = vec![uuid_from_hex("a65df604")];
        let result = resolve_agent_id("ffffffff", &candidates);
        assert!(matches!(result, Err(ResolveAgentIdError::NotFound { .. })));
    }

    #[test]
    fn test_resolve_invalid_format() {
        let candidates = vec![uuid_from_hex("a65df604")];
        let result = resolve_agent_id("not-a-hex!", &candidates);
        assert!(matches!(
            result,
            Err(ResolveAgentIdError::InvalidFormat { .. })
        ));
    }

    #[test]
    fn test_short_id() {
        let uuid = Uuid::parse_str("a65df604-b0e4-4dff-89fe-46ef82672377").unwrap();
        assert_eq!(short_id(&uuid), "a65df604");
    }

    #[test]
    fn test_resolve_lenient() {
        let id = uuid_from_hex("a65df604");
        let candidates = vec![id];
        let result = resolve_agent_id_lenient("a65df604", &candidates);
        assert_eq!(result, Some(id));
    }

    #[test]
    fn test_resolve_lenient_no_match() {
        let candidates = vec![uuid_from_hex("a65df604")];
        let result = resolve_agent_id_lenient("ffffffff", &candidates);
        assert_eq!(result, None);
    }

    // ================================================================
    // 物品标识
    // ================================================================

    #[test]
    fn test_item_uuid_deterministic() {
        let a = item_uuid("馒头");
        let b = item_uuid("馒头");
        assert_eq!(a, b, "同一 item_id 必须派生出同一 uuid");
        assert_ne!(a, item_uuid("刀"), "不同 item_id 必须派生出不同 uuid");
        assert_eq!(a.get_version_num(), 5, "必须是 v5 uuid");
    }

    #[test]
    fn test_item_uuid_namespace_stable() {
        // 固定 namespace：跨部署/跨重启派生结果不变（写死样例防漂移）
        let u = item_uuid("馒头");
        assert_eq!(
            u.to_string(),
            uuid::Uuid::new_v5(&ITEM_UUID_NAMESPACE, "馒头".as_bytes()).to_string()
        );
    }

    #[test]
    fn test_parse_item_ref_with_uuid_suffix() {
        let (bare, short) = parse_item_ref("刀[1a2b3c4d]");
        assert_eq!(bare, "刀");
        assert_eq!(short.as_deref(), Some("1a2b3c4d"));
    }

    #[test]
    fn test_parse_item_ref_bare() {
        let (bare, short) = parse_item_ref("馒头");
        assert_eq!(bare, "馒头");
        assert_eq!(short, None);
    }

    #[test]
    fn test_parse_item_ref_rejects_malformed_suffix() {
        // 非 hex / 长度不对 / ']' 不在末尾 → 均视为裸名称
        assert_eq!(parse_item_ref("刀[zzzzzzzz]").1, None);
        assert_eq!(parse_item_ref("刀[1a2b3c]").1, None);
        assert_eq!(parse_item_ref("刀[1a2b3c4d]多余").1, None);
        // 名称内含 '[' 时取最右一方括号
        let (bare, short) = parse_item_ref("奇[怪]刀[1a2b3c4d]");
        assert_eq!(bare, "奇[怪]刀");
        assert_eq!(short.as_deref(), Some("1a2b3c4d"));
    }

    #[test]
    fn test_parse_item_ref_trims_whitespace() {
        let (bare, short) = parse_item_ref("  刀[1a2b3c4d] ");
        assert_eq!(bare, "刀");
        assert_eq!(short.as_deref(), Some("1a2b3c4d"));
    }
}
