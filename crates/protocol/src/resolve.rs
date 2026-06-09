//! Agent ID 解析工具
//!
//! 支持 UUID prefix 匹配，降低 LLM 输出长 UUID 的准确性压力。
//! LLM 输出前 8 位 hex 即可匹配到唯一 agent。

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
}
