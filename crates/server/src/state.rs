use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::info;

use crate::config::Config;
use crate::db::DbPool;
use crate::dialogue;
use crate::game_data;
use crate::websocket;

// ============================================================================
// 速率限制器
// ============================================================================

/// Intent 速率限制器
///
/// 记录每个 Agent 最后一次 Intent 上报时间
/// 防止恶意客户端频繁发送 Intent
pub type RateLimiter = Arc<RwLock<std::collections::HashMap<uuid::Uuid, Instant>>>;

/// 创建速率限制器
pub fn create_rate_limiter() -> RateLimiter {
    Arc::new(RwLock::new(std::collections::HashMap::new()))
}

/// 检查速率限制
///
/// 返回:
/// - true: 允许通过
/// - false: 被限流
pub async fn check_rate_limit(rate_limiter: &RateLimiter, agent_id: uuid::Uuid) -> bool {
    let mut limiter = rate_limiter.write().await;
    let now = Instant::now();

    // 从统一注册表获取配置
    let threshold = game_data::NetworkRegistry::websocket().cleanup_threshold;
    if limiter.len() > threshold {
        cleanup_expired_entries(&mut limiter, now);
    }

    if let Some(last_time) = limiter.get(&agent_id) {
        let elapsed = now.duration_since(*last_time).as_millis() as u64;
        let rate_limit_ms = game_data::NetworkRegistry::websocket().rate_limit_ms;
        if elapsed < rate_limit_ms {
            return false; // 被限流
        }
    }

    limiter.insert(agent_id, now);
    true
}

/// 清理过期的速率限制记录
///
/// 移除超过配置时间未活动的 Agent 记录，防止内存泄漏
fn cleanup_expired_entries(
    limiter: &mut std::collections::HashMap<uuid::Uuid, Instant>,
    now: Instant,
) {
    // 从统一注册表获取配置
    let cleanup_interval_secs = game_data::NetworkRegistry::websocket().cleanup_interval_secs;
    let cleanup_threshold = Duration::from_secs(cleanup_interval_secs);
    let before_count = limiter.len();

    limiter.retain(|_, last_time| now.duration_since(*last_time) < cleanup_threshold);

    let after_count = limiter.len();
    if before_count > after_count {
        let cleaned = before_count - after_count;
        info!("🧹 清理过期速率限制记录: {} 条", cleaned);
    }
}

/// 启动速率限制器清理任务
///
/// 后台定期清理过期的速率限制记录
pub fn start_rate_limiter_cleanup(rate_limiter: RateLimiter) -> JoinHandle<()> {
    tokio::spawn(async move {
        // 从统一注册表获取配置
        let interval_secs = game_data::NetworkRegistry::websocket().cleanup_interval_secs;
        let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
        loop {
            interval.tick().await;
            let mut limiter = rate_limiter.write().await;
            let now = Instant::now();
            cleanup_expired_entries(&mut limiter, now);
        }
    })
}

// ============================================================================
// 应用状态（共享状态）
// ============================================================================

/// 应用状态
///
/// 在整个应用中共享的状态，包括配置、数据库连接池等
#[derive(Debug)]
pub struct AppState {
    /// 配置（预留：运行时配置热更新）
    #[allow(dead_code)]
    pub config: Config,

    /// 数据库连接池
    pub db_pool: DbPool,

    /// WebSocket 连接管理器
    pub connection_manager: websocket::ConnectionManager,

    /// agent_id → device_id 反向映射
    pub agent_to_device_map: websocket::AgentToDeviceMap,

    /// Intent 管理器（临时缓存）
    pub intent_manager: websocket::IntentManager,

    /// Intent 速率限制器
    pub rate_limiter: RateLimiter,

    /// 游戏数据配置缓存
    pub game_data: Arc<game_data::GameDataCache>,

    /// 对话管理器
    pub dialogue_manager: Arc<dialogue::DialogueManager>,

    /// 管理员读 Token (R)
    pub admin_read_token: String,

    /// 管理员读写 Token (RW)
    pub admin_write_token: String,

    /// 服务器启动时间
    pub start_time: chrono::DateTime<chrono::Utc>,

    /// 配置文件目录路径
    pub config_dir: std::path::PathBuf,
}

impl AppState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: Config,
        db_pool: DbPool,
        connection_manager: websocket::ConnectionManager,
        agent_to_device_map: websocket::AgentToDeviceMap,
        intent_manager: websocket::IntentManager,
        rate_limiter: RateLimiter,
        game_data: Arc<game_data::GameDataCache>,
        dialogue_manager: Arc<dialogue::DialogueManager>,
        admin_read_token: String,
        admin_write_token: String,
        config_dir: std::path::PathBuf,
    ) -> Self {
        Self {
            config,
            db_pool,
            connection_manager,
            agent_to_device_map,
            intent_manager,
            rate_limiter,
            game_data,
            dialogue_manager,
            admin_read_token,
            admin_write_token,
            start_time: chrono::Utc::now(),
            config_dir,
        }
    }
}
