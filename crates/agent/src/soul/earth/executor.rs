// ============================================================================
// 地魂复合 ToolExecutor
// ============================================================================
//
// 路由 tool call 到具体执行器：
// - skill_view → 本地缓存/文件加载
// - search_memory → 语义搜索记忆
// - recall_archived → 按时间倒序回忆近期被遗忘的事件
// - get_relationship / list_relationships / record_social_event → RelationshipStore
// - query_rules → RuleCache + PromptTemplate 按需检索
// ============================================================================

use crate::component::llm::tool_types::{ToolDefinition, ToolExecutor};
use crate::component::rule_cache::RuleCache;
use crate::component::social::RelationshipStore;
use crate::component::state_store::WorldStateStore;
use anyhow::Result;
use async_trait::async_trait;
use cyber_jianghu_protocol::types::entities::{AvailableAction, RecipeDetail};
use cyber_jianghu_protocol::types::prompt_template::PromptTemplateConfig;
use std::collections::HashMap;
use std::sync::Arc;

/// 地魂工具执行器的依赖上下文
///
/// 统一构造参数。新增依赖只需加字段，调用方无需改签名。
pub struct EarthToolContext {
    pub skill_cache: HashMap<String, String>,
    pub memory_manager: Option<Arc<tokio::sync::RwLock<crate::component::memory::MemoryManager>>>,
    pub relationship_store: Option<RelationshipStore>,
    pub recipe_details: Vec<RecipeDetail>,
    pub world_state_store: Option<Arc<WorldStateStore>>,
    pub available_actions: Vec<AvailableAction>,
    pub rule_cache: Option<RuleCache>,
    pub prompt_template: Option<Arc<PromptTemplateConfig>>,
}

/// 地魂复合工具执行器
pub struct EarthToolExecutor {
    skill_cache: HashMap<String, String>,
    memory_manager: Option<Arc<tokio::sync::RwLock<crate::component::memory::MemoryManager>>>,
    relationship_store: Option<RelationshipStore>,
    recipe_details: Vec<RecipeDetail>,
    world_state_store: Option<Arc<WorldStateStore>>,
    available_actions: Vec<AvailableAction>,
    rule_cache: Option<RuleCache>,
    prompt_template: Option<Arc<PromptTemplateConfig>>,
}

impl EarthToolExecutor {
    /// 从 EarthToolContext 创建
    pub fn from_context(ctx: EarthToolContext) -> Self {
        Self {
            skill_cache: ctx.skill_cache,
            memory_manager: ctx.memory_manager,
            relationship_store: ctx.relationship_store,
            recipe_details: ctx.recipe_details,
            world_state_store: ctx.world_state_store,
            available_actions: ctx.available_actions,
            rule_cache: ctx.rule_cache,
            prompt_template: ctx.prompt_template,
        }
    }

    /// 获取所有可用 tool 定义（实例方法，支持动态 query_rules description）
    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        let mut defs = vec![
            super::skill_tool::skill_view_definition(),
            super::memory_tool::search_memory_definition(),
            super::memory_tool::recall_archived_definition(),
            super::relationship_tool::get_relationship_definition(),
            super::relationship_tool::list_relationships_definition(),
            super::relationship_tool::record_social_event_definition(),
            super::recipe_tool::list_known_recipes_definition(),
            super::recipe_tool::view_recipe_detail_definition(),
            super::state_tool::get_action_detail_definition(),
            super::state_tool::query_world_definition(),
            super::state_tool::lookup_character_definition(),
            super::state_tool::list_skills_definition(),
        ];
        if let Some(ref cache) = self.rule_cache {
            defs.push(super::rule_tool::query_rules_definition(cache.categories()));
        }
        defs
    }
}

#[async_trait]
impl ToolExecutor for EarthToolExecutor {
    async fn execute(
        &self,
        name: &str,
        arguments: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        match name {
            "skill_view" => {
                let skill_id = arguments["skill_id"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("缺少 skill_id 参数"))?;

                Ok(super::skill_tool::execute_skill_view(
                    skill_id,
                    &self.skill_cache,
                ))
            }
            "search_memory" => {
                let query = arguments["query"].as_str().unwrap_or("未知查询");
                let limit = arguments["limit"].as_u64().map(|v| v as usize).unwrap_or(5);

                if let Some(ref memory_manager) = self.memory_manager {
                    let manager = memory_manager.read().await;
                    Ok(super::memory_tool::execute_search_memory(&manager, query, limit).await)
                } else {
                    Ok(serde_json::json!({
                        "success": false,
                        "implemented": false,
                        "message": "记忆管理器未初始化，无法搜索记忆"
                    }))
                }
            }
            "recall_archived" => {
                let limit = arguments["limit"].as_u64().map(|v| v as usize).unwrap_or(5);

                if let Some(ref memory_manager) = self.memory_manager {
                    let manager = memory_manager.read().await;
                    Ok(super::memory_tool::execute_recall_archived(&manager, limit).await)
                } else {
                    Ok(serde_json::json!({
                        "success": false,
                        "implemented": false,
                        "message": "记忆管理器未初始化，无法回忆记忆"
                    }))
                }
            }
            "get_relationship" => {
                let identifier = arguments["identifier"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("缺少 identifier 参数"))?;

                if let Some(ref store) = self.relationship_store {
                    Ok(super::relationship_tool::execute_get_relationship(
                        store, identifier,
                    ))
                } else {
                    Ok(serde_json::json!({
                        "success": false,
                        "implemented": false,
                        "message": "关系存储未初始化，无法查询关系"
                    }))
                }
            }
            "list_relationships" => {
                let min_fav = arguments["min_favorability"].as_i64().map(|v| v as i32);
                let max_fav = arguments["max_favorability"].as_i64().map(|v| v as i32);

                if let Some(ref store) = self.relationship_store {
                    Ok(super::relationship_tool::execute_list_relationships(
                        store, min_fav, max_fav,
                    ))
                } else {
                    Ok(serde_json::json!({
                        "success": false,
                        "implemented": false,
                        "message": "关系存储未初始化，无法列出关系"
                    }))
                }
            }
            "list_known_recipes" => Ok(super::recipe_tool::execute_list_known_recipes(
                &self.recipe_details,
            )),
            "view_recipe_detail" => {
                let recipe_id = arguments["recipe_id"].as_str().unwrap_or("");
                Ok(super::recipe_tool::execute_view_recipe_detail(
                    recipe_id,
                    &self.recipe_details,
                ))
            }
            "record_social_event" => {
                let target_agent_id = arguments["target_agent_id"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("缺少 target_agent_id 参数"))?;
                let target_name = arguments["target_name"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("缺少 target_name 参数"))?;
                let tick_id = arguments["tick_id"]
                    .as_i64()
                    .ok_or_else(|| anyhow::anyhow!("缺少 tick_id 参数"))?;
                let action = arguments["action"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("缺少 action 参数"))?;
                let description = arguments["description"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("缺少 description 参数"))?;
                let delta = arguments["favorability_delta"]
                    .as_i64()
                    .ok_or_else(|| anyhow::anyhow!("缺少 favorability_delta 参数"))?
                    as i32;

                if let Some(ref store) = self.relationship_store {
                    Ok(super::relationship_tool::execute_record_social_event(
                        store,
                        target_agent_id,
                        target_name,
                        tick_id,
                        action,
                        description,
                        delta,
                    ))
                } else {
                    Ok(serde_json::json!({
                        "success": false,
                        "implemented": false,
                        "message": "关系存储未初始化，无法记录社交事件"
                    }))
                }
            }
            "get_action_detail" => {
                let action_type = arguments["action_type"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("缺少 action_type 参数"))?;
                Ok(super::state_tool::execute_get_action_detail(
                    action_type,
                    &self.available_actions,
                ))
            }
            "query_world" => {
                let section = arguments["section"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("缺少 section 参数"))?;
                let filter = arguments["filter"].as_str();
                if let Some(ref store) = self.world_state_store {
                    Ok(super::state_tool::execute_query_world(section, filter, store).await)
                } else {
                    Ok(serde_json::json!({
                        "success": false,
                        "message": "WorldStateStore 未初始化"
                    }))
                }
            }
            "lookup_character" => {
                let name = arguments["name"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("缺少 name 参数"))?;
                if let Some(ref store) = self.world_state_store {
                    Ok(super::state_tool::execute_lookup_character(name, store).await)
                } else {
                    Ok(serde_json::json!({
                        "success": false,
                        "message": "WorldStateStore 未初始化"
                    }))
                }
            }
            "list_skills" => Ok(super::state_tool::execute_list_skills(&self.skill_cache)),
            "query_rules" => {
                let categories = arguments["categories"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();

                match (&self.rule_cache, &self.prompt_template) {
                    (Some(cache), Some(tmpl)) => Ok(super::rule_tool::execute_query_rules(
                        &categories,
                        cache,
                        tmpl,
                    )),
                    _ => Ok(serde_json::json!({
                        "success": false,
                        "implemented": false,
                        "message": "规则缓存未初始化，无法查询规则"
                    })),
                }
            }
            _ => Err(anyhow::anyhow!("地魂未知工具: {}", name)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_definitions_count_without_rule_cache() {
        let executor = EarthToolExecutor::from_context(EarthToolContext {
            skill_cache: HashMap::new(),
            memory_manager: None,
            relationship_store: None,
            recipe_details: vec![],
            world_state_store: None,
            available_actions: vec![],
            rule_cache: None,
            prompt_template: None,
        });
        let defs = executor.tool_definitions();
        assert_eq!(defs.len(), 12);
    }

    #[test]
    fn test_from_context() {
        let ctx = EarthToolContext {
            skill_cache: HashMap::new(),
            memory_manager: None,
            relationship_store: None,
            recipe_details: vec![],
            world_state_store: None,
            available_actions: vec![],
            rule_cache: None,
            prompt_template: None,
        };
        let executor = EarthToolExecutor::from_context(ctx);
        assert!(executor.skill_cache.is_empty());
    }

    #[test]
    fn test_skill_view_from_cache() {
        let mut cache = HashMap::new();
        cache.insert("bargaining".to_string(), "讨价还价指引".to_string());
        let executor = EarthToolExecutor::from_context(EarthToolContext {
            skill_cache: cache,
            memory_manager: None,
            relationship_store: None,
            recipe_details: vec![],
            world_state_store: None,
            available_actions: vec![],
            rule_cache: None,
            prompt_template: None,
        });

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt
            .block_on(
                executor.execute("skill_view", &serde_json::json!({"skill_id": "bargaining"})),
            )
            .unwrap();

        assert_eq!(result["skill_id"], "bargaining");
        assert_eq!(result["content"], "讨价还价指引");
    }
}
