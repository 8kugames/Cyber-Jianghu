// ============================================================================
// OpenClaw Cyber-Jianghu 验证函数
// ============================================================================
//
// 本模块从 models/mod.rs 拆分出来，包含所有验证相关的函数
// ============================================================================

use crate::game_data::StateRegistry;

/// 获取 Agent 名称最大长度
pub fn get_max_agent_name_length() -> usize {
    StateRegistry::validation().max_agent_name_length
}

/// 获取 Agent system_prompt 最大长度
pub fn get_max_system_prompt_length() -> usize {
    StateRegistry::validation().max_system_prompt_length
}

/// 获取对话内容最大长度（预留：对话系统）
#[allow(dead_code)]
pub fn get_max_speak_content_length() -> usize {
    StateRegistry::validation().max_speak_content_length
}
