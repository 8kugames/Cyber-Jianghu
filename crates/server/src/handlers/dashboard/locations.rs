// ============================================================================
// 地点拓扑端点 (C4)
// ============================================================================
//
// GET /api/dashboard/locations
//
// 从 game_data 的 LocationRegistry 暴露完整节点+边图，供前端绘制地图、
// 显示可达区域、计算路径。返回的是当前进程加载的静态配置数据（locations.yaml），
// 与运行期 agent 位置无关。
// ============================================================================

use axum::{Json, extract::State};
use serde::Serialize;
use std::sync::Arc;

use crate::state::AppState;
use cyber_jianghu_protocol::{LocationEdge, LocationGraph, LocationNode};

/// 地点拓扑响应
#[derive(Debug, Serialize)]
pub struct LocationsResponse {
    /// 节点总数
    pub node_count: usize,
    /// 边总数（含重复，因为双向边可能存为两条）
    pub edge_count: usize,
    /// 完整节点表 { node_id: LocationNode }
    pub nodes: std::collections::HashMap<String, LocationNode>,
    /// 邻接表 { from_node_id: [edges...] }
    pub edges: std::collections::HashMap<String, Vec<LocationEdge>>,
    /// 当前游戏日（resolved_descriptions 的解析基准）
    pub game_day: i64,
    /// 各节点当前时代的解析描述 { node_id: description }
    /// （时代变体命中优先，回落基础描述；无描述节点不入表。天道全知视角不过滤隐藏，
    /// 但描述随时代演进）
    pub resolved_descriptions: std::collections::HashMap<String, String>,
}

/// GET /api/dashboard/locations
///
/// 返回完整地点图（节点+边），数据来自 LocationRegistry 的内存快照。
pub async fn get_locations(State(state): State<Arc<AppState>>) -> Json<LocationsResponse> {
    // location_snapshot 返回 owned LocationRegistry（已脱离读锁）
    let registry = state.game_data.location_snapshot();
    let graph: LocationGraph = registry.export_graph();

    let node_count = graph.nodes.len();
    let edge_count: usize = graph.edges.values().map(|v| v.len()).sum();

    // 时代感知描述：按当前世界 tick 换算游戏日后解析。tick 读不可得时按第 1 日退化，
    // 但不静默：DB 故障下面板会展示错误时代描述，需可辨识。
    let current_tick = match crate::db::get_current_world_tick_id(&state.db_pool).await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("读取当前世界 tick 失败，时代描述按第 1 日退化: {}", e);
            0
        }
    };
    let game_day = crate::game_data::registry::time_registry::TimeRegistry::game_day(current_tick);
    let resolved_descriptions = registry.resolved_descriptions_at(game_day);

    Json(LocationsResponse {
        node_count,
        edge_count,
        nodes: graph.nodes,
        edges: graph.edges,
        game_day,
        resolved_descriptions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_response_is_serialize() {
        fn assert_serialize<T: serde::Serialize>() {}
        assert_serialize::<LocationsResponse>();
    }
}
