// ============================================================================
// OpenClaw Cyber-Jianghu Agent SDK - Agent 人设 Prompt
// ============================================================================
//
// 本模块定义 Agent Prompt 的数据结构。
// 实际 persona 数据由服务端配置下发，不在此硬编码。
// ============================================================================

/// Agent Prompt定义
#[derive(Debug, Clone)]
pub struct AgentPrompt {
    /// Agent名称
    pub name: &'static str,

    /// 系统Prompt（人设）
    pub system_prompt: &'static str,
}
