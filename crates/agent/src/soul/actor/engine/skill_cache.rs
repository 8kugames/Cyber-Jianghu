// ============================================================================
// 技能缓存磁盘持久化（load/persist skill cache）
// ============================================================================

use super::*;

impl CognitiveEngine {
    /// 数据目录（用于本地持久化，如 skill_cache.json）
    pub(super) fn resolve_data_dir() -> std::path::PathBuf {
        crate::config::data_base_dir()
    }

    /// skill_cache.json 路径
    pub(super) fn skill_cache_path() -> std::path::PathBuf {
        Self::resolve_data_dir().join("skill_cache.json")
    }

    /// 启动时从本地文件加载 skill 缓存
    pub(super) fn load_skill_cache_from_disk(&self) {
        let path = Self::skill_cache_path();
        if !path.exists() {
            return;
        }
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                let cached: std::collections::HashMap<String, String> =
                    match serde_json::from_str(&content) {
                        Ok(m) => m,
                        Err(e) => {
                            tracing::warn!("skill_cache.json 解析失败，忽略: {}", e);
                            return;
                        }
                    };
                let mut cache = self.skill_cache.write().expect("rwlock poisoned");
                let count = cached.len();
                *cache = cached;
                tracing::info!("从 skill_cache.json 加载了 {} 个技能", count);
            }
            Err(e) => {
                tracing::debug!("读取 skill_cache.json 失败: {}", e);
            }
        }
    }

    /// 持久化 skill 缓存到本地文件
    pub(super) fn persist_skill_cache_to_disk(&self) {
        let path = Self::skill_cache_path();
        let cache = self.skill_cache.read().expect("rwlock poisoned").clone();
        if cache.is_empty() {
            return;
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(&cache) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    tracing::warn!("skill_cache.json 写入失败: {}", e);
                }
            }
            Err(e) => {
                tracing::warn!("skill_cache.json 序列化失败: {}", e);
            }
        }
    }
}
