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
/// 内部状态: `(Option<WorldState>, WorldState)` = `(prev, curr)`
/// - 首次 new 后 prev = None（无历史）
/// - 每次 update: prev <- 旧 curr, curr <- new_state
#[derive(Clone)]
pub struct WorldStateStore {
    state: Arc<RwLock<(Option<WorldState>, WorldState)>>,
}

impl WorldStateStore {
    /// 创建新 store，传入初始 WorldState 作为 curr
    ///
    /// prev 为 None，表示首次 tick 无历史数据
    pub fn new(initial: WorldState) -> Self {
        Self {
            state: Arc::new(RwLock::new((None, initial))),
        }
    }

    /// 原子更新: prev <- curr, curr <- new_state
    pub async fn update(&self, new_state: WorldState) {
        let mut guard = self.state.write().await;
        let prev = std::mem::replace(&mut guard.1, new_state);
        guard.0 = Some(prev);
    }

    /// 获取当前 WorldState（clone）
    pub async fn current(&self) -> WorldState {
        self.state.read().await.1.clone()
    }

    /// 获取上一个 WorldState（首次 update 前返回 None）
    pub async fn previous(&self) -> Option<WorldState> {
        self.state.read().await.0.clone()
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
    async fn test_previous_is_none_initially() {
        let store = WorldStateStore::new(make_world_state(1));
        assert!(store.previous().await.is_none());
        assert_eq!(store.current().await.tick_id, 1);
    }

    #[tokio::test]
    async fn test_update_sets_curr_and_prev() {
        let store = WorldStateStore::new(make_world_state(1));
        store.update(make_world_state(2)).await;

        assert_eq!(store.current().await.tick_id, 2);
        let prev = store.previous().await;
        assert!(prev.is_some());
        assert_eq!(prev.unwrap().tick_id, 1);
    }

    #[tokio::test]
    async fn test_current_returns_latest() {
        let store = WorldStateStore::new(make_world_state(1));
        store.update(make_world_state(2)).await;
        store.update(make_world_state(3)).await;

        assert_eq!(store.current().await.tick_id, 3);
        assert_eq!(store.previous().await.unwrap().tick_id, 2);
    }
}
