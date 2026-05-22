use serde::Serialize;


#[derive(Serialize)]
pub struct DashboardStats {
    pub current_active_agents: i64,
    pub total_registered_agents: i64,
    pub dau: i64,
    pub active_3d: i64,
    pub active_7d: i64,
    pub mau: i64,
    pub yau: i64,
    pub server_uptime_secs: i64,
    pub server_running_days: i64,
    pub game_time: WorldTime,
    pub game_flow_total_hours: i64,
    pub world_overview: String,
    pub tick_duration_secs: u64,
    /// 当前 tick ID（供前端计算平滑时间）
    pub current_tick_id: i64,
    /// 每游戏小时对应的 tick 数（供前端计算平滑时间）
    pub ticks_per_hour: f64,

    // Bug #5: 新增监控指标
    pub natural_deaths_last_24h: i64,
    pub abnormal_deaths_last_24h: i64,
    pub offline_duration_distribution: OfflineDistribution,
}

#[derive(Serialize)]
pub struct OfflineDistribution {
    pub less_than_1h: i64,
    pub one_to_24h: i64,
    pub one_to_7d: i64,
    pub more_than_7d: i64,
}

#[derive(Serialize)]
pub struct WorldTime {
    pub year: i32,
    pub month: i32,
    pub day: i32,
    pub hour: i32,
    pub minute: i32,
    pub second: i32,
    /// 当前季节名称
    pub season: String,
    /// 天道历格式文本
    pub text: String,
}

