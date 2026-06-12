use cyber_jianghu_protocol::GovernanceCode;

/// Server 端统一 GovernanceCode 映射
///
/// 设计文档 §6.1: "Server 治理入口统一将 RawRejectionFact 映射为治理分类码"
/// Agent 端不自行做字符串匹配，而是接收 Server 映射后的 GovernanceCode 枚举。
pub struct ServerGovernanceMapper;

impl ServerGovernanceMapper {
    /// 从 rejection error message 映射 GovernanceCode
    pub fn map_from_error(error_msg: &str) -> GovernanceCode {
        if error_msg.contains("未知的动作类型") || error_msg.starts_with("unknown action") {
            return GovernanceCode::UnknownAction;
        }
        if error_msg.contains("表达力不足") || error_msg.contains("能力缺失") {
            return GovernanceCode::ExpressionGap;
        }
        GovernanceCode::NonGovernanceReject
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_unknown_action() {
        assert_eq!(
            ServerGovernanceMapper::map_from_error("未知的动作类型: mining"),
            GovernanceCode::UnknownAction
        );
        assert_eq!(
            ServerGovernanceMapper::map_from_error("unknown action type"),
            GovernanceCode::UnknownAction
        );
    }

    #[test]
    fn test_map_expression_gap() {
        assert_eq!(
            ServerGovernanceMapper::map_from_error("动作表达力不足"),
            GovernanceCode::ExpressionGap
        );
        assert_eq!(
            ServerGovernanceMapper::map_from_error("能力缺失: 该动作需要更多参数"),
            GovernanceCode::ExpressionGap
        );
    }

    #[test]
    fn test_map_non_governance() {
        assert_eq!(
            ServerGovernanceMapper::map_from_error("缺少必需字段: target_agent_id"),
            GovernanceCode::NonGovernanceReject
        );
        assert_eq!(
            ServerGovernanceMapper::map_from_error("属性 stamina 不足"),
            GovernanceCode::NonGovernanceReject
        );
    }
}
