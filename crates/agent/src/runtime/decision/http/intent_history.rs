// ============================================================================
// Intent History Store - Intent 历史存储
// ============================================================================
//
// 用于存储每个 Tick 的 Intent 提交记录，支持经历日志查询。
// 数据来源：
// - thought_log: Agent 提交 Intent 时的思考日志
// - observer_thought: Observer Agent 审查时的思维链

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

/// 默认最大保存条目数
const DEFAULT_MAX_ENTRIES: usize = 100;

/// Intent 历史条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentHistoryEntry {
    /// Tick ID
    pub tick_id: i64,
    /// Intent ID
    pub intent_id: Uuid,
    /// 动作类型
    pub action_type: String,
    /// Agent 思考日志（intent_summary 的来源）
    pub thought_log: Option<String>,
    /// Observer 思维链
    pub observer_thought: Option<String>,
    /// 创建时间
    pub created_at: DateTime<Utc>,
}

/// Intent 历史存储
///
/// 内存存储，按 tick_id 索引，支持快速查询。
/// 自动清理过期条目，防止内存无限增长。
#[derive(Debug)]
pub struct IntentHistoryStore {
    /// 按 tick_id 索引的条目
    entries: RwLock<HashMap<i64, IntentHistoryEntry>>,
    /// 最大保存条目数
    max_entries: usize,
}

impl Default for IntentHistoryStore {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_ENTRIES)
    }
}

impl IntentHistoryStore {
    /// 创建新的存储
    ///
    /// # Arguments
    /// * `max_entries` - 最大保存条目数，超过后自动清理最旧的条目
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            max_entries,
        }
    }

    /// 记录 Intent 提交
    ///
    /// 在 Agent 提交 Intent 时调用，记录 thought_log。
    ///
    /// # Arguments
    /// * `tick_id` - Tick ID
    /// * `intent_id` - Intent 唯一 ID
    /// * `action_type` - 动作类型
    /// * `thought_log` - Agent 的思考日志
    pub async fn record_intent(
        &self,
        tick_id: i64,
        intent_id: Uuid,
        action_type: String,
        thought_log: Option<String>,
    ) {
        let entry = IntentHistoryEntry {
            tick_id,
            intent_id,
            action_type,
            thought_log,
            observer_thought: None,
            created_at: Utc::now(),
        };

        let mut entries = self.entries.write().await;
        entries.insert(tick_id, entry);

        // 清理过期条目
        if entries.len() > self.max_entries {
            let mut ticks: Vec<i64> = entries.keys().copied().collect();
            ticks.sort();
            let to_remove = entries.len() - self.max_entries;
            for tick in ticks.into_iter().take(to_remove) {
                entries.remove(&tick);
                tracing::debug!("[intent_history] Removed expired entry for tick {}", tick);
            }
        }

        tracing::debug!(
            "[intent_history] Recorded intent for tick {}, total entries: {}",
            tick_id,
            entries.len()
        );
    }

    /// 更新 Observer 思维链
    ///
    /// 在 Observer Agent 提交审查结果时调用。
    ///
    /// # Arguments
    /// * `tick_id` - Tick ID
    /// * `thought` - Observer 的思维链
    pub async fn update_observer_thought(&self, tick_id: i64, thought: String) {
        let mut entries = self.entries.write().await;
        if let Some(entry) = entries.get_mut(&tick_id) {
            entry.observer_thought = Some(thought);
            tracing::debug!(
                "[intent_history] Updated observer thought for tick {}",
                tick_id
            );
        } else {
            tracing::warn!(
                "[intent_history] No entry found for tick {} when updating observer thought",
                tick_id
            );
        }
    }

    /// 获取指定 tick 的条目
    ///
    /// # Arguments
    /// * `tick_id` - Tick ID
    ///
    /// # Returns
    /// 如果找到则返回条目，否则返回 None
    pub async fn get_by_tick(&self, tick_id: i64) -> Option<IntentHistoryEntry> {
        let entries = self.entries.read().await;
        entries.get(&tick_id).cloned()
    }

    /// 批量获取多个 tick 的条目
    ///
    /// # Arguments
    /// * `tick_ids` - Tick ID 列表
    ///
    /// # Returns
    /// tick_id -> IntentHistoryEntry 的映射
    pub async fn get_by_ticks(&self, tick_ids: &[i64]) -> HashMap<i64, IntentHistoryEntry> {
        let entries = self.entries.read().await;
        tick_ids
            .iter()
            .filter_map(|&tick_id| entries.get(&tick_id).cloned().map(|e| (tick_id, e)))
            .collect()
    }

    /// 获取当前条目数量
    pub async fn len(&self) -> usize {
        self.entries.read().await.len()
    }

    /// 检查是否为空
    pub async fn is_empty(&self) -> bool {
        self.entries.read().await.is_empty()
    }
}

/// 创建线程安全的 IntentHistoryStore
pub fn create_intent_history_store(max_entries: usize) -> Arc<IntentHistoryStore> {
    Arc::new(IntentHistoryStore::new(max_entries))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_record_and_get_intent() {
        let store = IntentHistoryStore::new(10);
        let intent_id = Uuid::new_v4();

        store
            .record_intent(
                1,
                intent_id,
                "idle".to_string(),
                Some("思考中...".to_string()),
            )
            .await;

        let entry = store.get_by_tick(1).await;
        assert!(entry.is_some());
        let entry = entry.unwrap();
        assert_eq!(entry.tick_id, 1);
        assert_eq!(entry.action_type, "idle");
        assert_eq!(entry.thought_log, Some("思考中...".to_string()));
        assert!(entry.observer_thought.is_none());
    }

    #[tokio::test]
    async fn test_update_observer_thought() {
        let store = IntentHistoryStore::new(10);
        let intent_id = Uuid::new_v4();

        store
            .record_intent(1, intent_id, "idle".to_string(), None)
            .await;

        store
            .update_observer_thought(1, "这个行为符合人设".to_string())
            .await;

        let entry = store.get_by_tick(1).await.unwrap();
        assert_eq!(entry.observer_thought, Some("这个行为符合人设".to_string()));
    }

    #[tokio::test]
    async fn test_cleanup_old_entries() {
        let store = IntentHistoryStore::new(5);

        // 添加 10 个条目
        for i in 1..=10 {
            store
                .record_intent(i, Uuid::new_v4(), "idle".to_string(), None)
                .await;
        }

        // 应该只保留最新的 5 个
        assert_eq!(store.len().await, 5);

        // 旧的条目应该被清理
        assert!(store.get_by_tick(1).await.is_none());
        assert!(store.get_by_tick(5).await.is_none());

        // 新的条目应该存在
        assert!(store.get_by_tick(6).await.is_some());
        assert!(store.get_by_tick(10).await.is_some());
    }

    #[tokio::test]
    async fn test_get_by_ticks() {
        let store = IntentHistoryStore::new(10);

        for i in 1..=5 {
            store
                .record_intent(
                    i,
                    Uuid::new_v4(),
                    "idle".to_string(),
                    Some(format!("thought {}", i)),
                )
                .await;
        }

        let entries = store.get_by_ticks(&[1, 3, 5, 10]).await;
        assert_eq!(entries.len(), 3); // 1, 3, 5 存在，10 不存在
    }
}
