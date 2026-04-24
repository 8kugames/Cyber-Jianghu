// ============================================================================
// 地魂复合 ToolExecutor
// ============================================================================
//
// 路由 tool call 到具体执行器：
// - skill_view → 本地缓存/文件加载（已实现）
// - search_memory / recall_archived → MemoryManager（预留，暂返回提示）
// ============================================================================

use crate::component::llm::tool_types::{ToolDefinition, ToolExecutor};
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// 地魂复合工具执行器
pub struct EarthToolExecutor {
    /// 技能内容缓存（skill_id → SKILL.md body）
    skill_cache: HashMap<String, String>,
    /// 配置目录（用于从文件加载 SKILL.md）
    config_dir: PathBuf,
    /// 记忆管理器（用于 search_memory / recall_archived）
    memory_manager: Option<Arc<tokio::sync::RwLock<crate::component::memory::MemoryManager>>>,
}

impl EarthToolExecutor {
    /// 创建地魂执行器
    pub fn new(skill_cache: HashMap<String, String>, config_dir: PathBuf) -> Self {
        Self {
            skill_cache,
            config_dir,
            memory_manager: None,
        }
    }

    /// 创建带记忆管理器的地魂执行器
    pub fn with_memory_manager(
        skill_cache: HashMap<String, String>,
        config_dir: PathBuf,
        memory_manager: Option<Arc<tokio::sync::RwLock<crate::component::memory::MemoryManager>>>,
    ) -> Self {
        Self {
            skill_cache,
            config_dir,
            memory_manager,
        }
    }

    /// 从 CognitiveEngine 的 skill_cache RwLock 创建
    pub fn from_rw_lock(
        cache: &RwLock<HashMap<String, String>>,
        config_dir: PathBuf,
    ) -> Self {
        let skill_cache = cache.read().unwrap().clone();
        Self {
            skill_cache,
            config_dir,
            memory_manager: None,
        }
    }

    /// 从 CognitiveEngine 的相关缓存和管理器创建
    pub fn from_engine(
        cache: &RwLock<HashMap<String, String>>,
        config_dir: PathBuf,
        memory_manager: Option<Arc<tokio::sync::RwLock<crate::component::memory::MemoryManager>>>,
    ) -> Self {
        let skill_cache = cache.read().unwrap().clone();
        Self {
            skill_cache,
            config_dir,
            memory_manager,
        }
    }

    /// 获取所有可用 tool 定义
    pub fn tool_definitions() -> Vec<ToolDefinition> {
        vec![
            super::skill_tool::skill_view_definition(),
            super::memory_tool::search_memory_definition(),
            super::memory_tool::recall_archived_definition(),
        ]
    }
}

#[async_trait]
impl ToolExecutor for EarthToolExecutor {
    async fn execute(&self, name: &str, arguments: &serde_json::Value) -> Result<serde_json::Value> {
        match name {
            "skill_view" => {
                let skill_id = arguments["skill_id"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("缺少 skill_id 参数"))?;

                Ok(super::skill_tool::execute_skill_view(
                    skill_id,
                    &self.skill_cache,
                    &self.config_dir,
                ))
            }
            "search_memory" | "recall_archived" => {
                let query = arguments["query"]
                    .as_str()
                    .unwrap_or("未知查询");
                let limit = arguments["limit"]
                    .as_u64()
                    .map(|v| v as usize)
                    .unwrap_or(5);

                if let Some(ref memory_manager) = self.memory_manager {
                    let manager = memory_manager.read().await;
                    if name == "recall_archived" {
                        Ok(super::memory_tool::execute_recall_archived(&manager, query, limit).await)
                    } else {
                        Ok(super::memory_tool::execute_search_memory(&manager, query, limit).await)
                    }
                } else {
                    Ok(serde_json::json!({
                        "success": false,
                        "implemented": false,
                        "message": "记忆管理器未初始化，无法搜索记忆"
                    }))
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
    fn test_tool_definitions_count() {
        let defs = EarthToolExecutor::tool_definitions();
        assert_eq!(defs.len(), 3);
        assert_eq!(defs[0].function.name, "skill_view");
        assert_eq!(defs[1].function.name, "search_memory");
        assert_eq!(defs[2].function.name, "recall_archived");
    }

    #[test]
    fn test_skill_view_from_cache() {
        let mut cache = HashMap::new();
        cache.insert("bargaining".to_string(), "讨价还价指引".to_string());
        let executor = EarthToolExecutor::new(cache, PathBuf::from("/tmp"));

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(executor.execute(
            "skill_view",
            &serde_json::json!({"skill_id": "bargaining"}),
        )).unwrap();

        assert_eq!(result["skill_id"], "bargaining");
        assert_eq!(result["content"], "讨价还价指引");
    }

    #[test]
    fn test_skill_view_not_found() {
        let executor = EarthToolExecutor::new(HashMap::new(), PathBuf::from("/tmp"));

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(executor.execute(
            "skill_view",
            &serde_json::json!({"skill_id": "nonexistent"}),
        )).unwrap();

        assert!(result["error"].is_string());
    }

    #[test]
    fn test_from_rw_lock() {
        let cache = RwLock::new(HashMap::new());
        let executor = EarthToolExecutor::from_rw_lock(&cache, PathBuf::from("/tmp"));
        assert!(executor.skill_cache.is_empty());
    }
}
