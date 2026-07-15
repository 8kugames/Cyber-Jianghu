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
}

/// GET /api/dashboard/locations
///
/// 返回完整地点图（节点+边），数据来自 LocationRegistry 的内存快照。
pub async fn get_locations(
    State(state): State<Arc<AppState>>,
) -> Json<LocationsResponse> {
    // location_snapshot 返回 owned LocationRegistry（已脱离读锁）
    let registry = state.game_data.location_snapshot();
    let graph: LocationGraph = registry.export_graph();

    let node_count = graph.nodes.len();
    let edge_count: usize = graph.edges.values().map(|v| v.len()).sum();

    Json(LocationsResponse {
        node_count,
        edge_count,
        nodes: graph.nodes,
        edges: graph.edges,
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
