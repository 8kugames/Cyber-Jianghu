// ============================================================================
// WorldStateStore - Agent 侧 WorldState 本地落存
// ============================================================================
//
// 维护 prev/curr 两个 WorldState，供 Delta Engine 做增量检测。
// update() 原子性完成 prev <- curr, curr <- new_state。
//
// 设计决策：
// - 与 HttpApiState.current_state (Arc<RwLock<Option<WorldState>>>) 并存，
//   后者服务于 HTTP API 查询，本组件服务于 Delta Engine。
// - 不复用 HttpApiState 是因为需要 prev/curr pair，且 HttpApiState
//   是巨大 struct，不宜为其添加语义不明的字段。
// ============================================================================

use cyber_jianghu_protocol::WorldState;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Agent 侧 WorldState 存储（含 prev/curr，供 Delta Engine 使用）
///
/// prev/curr WorldState pair
type WorldStatePair = (Option<WorldState>, WorldState);

/// 内部状态: `Option<(Option<WorldState>, WorldState)>` = `Option<(prev, curr)>`
/// - 首次 new 后内部为 None（等待首次 update 注入 WorldState）
/// - 每次 update: 首次设置 curr，后续 prev <- 旧 curr, curr <- new_state
#[derive(Clone, Default)]
pub struct WorldStateStore {
    state: Arc<RwLock<Option<WorldStatePair>>>,
}

impl WorldStateStore {
    /// 创建空 store（懒初始化，等待首次 update 注入 WorldState）
    pub fn new() -> Self {
        Self::default()
    }

    /// 原子更新: 首次设置 curr，后续 prev <- 旧 curr, curr <- new_state
    pub async fn update(&self, new_state: WorldState) {
        let mut guard = self.state.write().await;
        match guard.take() {
            None => {
                // 首次 update：无 prev
                *guard = Some((None, new_state));
            }
            Some((_, old_curr)) => {
                *guard = Some((Some(old_curr), new_state));
            }
        }
    }

    /// 获取当前 WorldState（首次 update 前返回 None）
    pub async fn current(&self) -> Option<WorldState> {
        self.state
            .read()
            .await
            .as_ref()
            .map(|(_, curr)| curr.clone())
    }

    /// 获取上一个 WorldState（首次 update 前返回 None）
    pub async fn previous(&self) -> Option<WorldState> {
        self.state
            .read()
            .await
            .as_ref()
            .and_then(|(prev, _)| prev.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyber_jianghu_protocol::WorldTime;
    use uuid::Uuid;

    /// 构造最小可用的 WorldState（仅填 required 字段，其余用 Default/空）
    fn make_world_state(tick_id: i64) -> WorldState {
        use cyber_jianghu_protocol::{AgentSelfState, Location};
        use std::collections::HashMap;

        WorldState {
            event_type: "world_state".to_string(),
            tick_id,
            agent_id: Some(Uuid::new_v4()),
            world_time: WorldTime {
                year: 1,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
                weather: String::new(),
            },
            location: Location {
                node_id: "test_loc".to_string(),
                name: "测试地点".to_string(),
                node_type: "test".to_string(),
                adjacent_nodes: vec![],
                gatherable_items: vec![],
            },
            self_state: AgentSelfState {
                attributes: HashMap::new(),
                derived_attributes: HashMap::new(),
                attribute_descriptions: HashMap::new(),
                status_effects: vec![],
                inventory: vec![],
                skills: vec![],
                recipe_details: vec![],
                age_years: None,
                max_age: None,
            },
            entities: vec![],
            nearby_items: vec![],
            events_log: vec![],
            private_dialogue_log: vec![],
            last_execution_summary: None,
            lessons_learned: vec![],
        }
    }

    #[tokio::test]
    async fn test_empty_before_first_update() {
        let store = WorldStateStore::new();
        assert!(store.current().await.is_none());
        assert!(store.previous().await.is_none());
    }

    #[tokio::test]
    async fn test_first_update_sets_curr_no_prev() {
        let store = WorldStateStore::new();
        store.update(make_world_state(1)).await;
        assert_eq!(store.current().await.unwrap().tick_id, 1);
        assert!(store.previous().await.is_none());
    }

    #[tokio::test]
    async fn test_second_update_sets_prev() {
        let store = WorldStateStore::new();
        store.update(make_world_state(1)).await;
        store.update(make_world_state(2)).await;
        assert_eq!(store.current().await.unwrap().tick_id, 2);
        assert_eq!(store.previous().await.unwrap().tick_id, 1);
    }

    #[tokio::test]
    async fn test_current_returns_latest() {
        let store = WorldStateStore::new();
        store.update(make_world_state(1)).await;
        store.update(make_world_state(2)).await;
        store.update(make_world_state(3)).await;
        assert_eq!(store.current().await.unwrap().tick_id, 3);
        assert_eq!(store.previous().await.unwrap().tick_id, 2);
    }
}
