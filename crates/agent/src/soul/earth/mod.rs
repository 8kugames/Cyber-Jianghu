// ============================================================================
// 地魂（EarthSoul）— 工具池
// ============================================================================
//
// Agent 的 tool-calling 执行层。LLM 在决策阶段可调用工具获取精确数据：
// - skill_view: 按需加载已掌握技能的 SKILL.md 行为指引
// - search_memory: 语义搜索记忆
// - recall_archived: 按时间倒序回忆近期被遗忘的事件
// - get_relationship / list_relationships / record_social_event: 关系管理
//
// 设计原则：progressive disclosure — prompt 只注入索引，LLM 自主判断何时加载详情。
// ============================================================================

mod executor;
mod memory_tool;
mod relationship_tool;
mod skill_tool;

pub use executor::{EarthToolContext, EarthToolExecutor};
pub(crate) use skill_tool::extract_skill_body;
