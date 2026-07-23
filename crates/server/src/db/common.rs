// ============================================================================
// OpenClaw Cyber-Jianghu MVP 数据库公共模块
// ============================================================================
//
// 本模块提供数据库连接池初始化和共享工具函数

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tracing::{error, info, warn};
use uuid::Uuid;

use rand::RngExt;

#[derive(Debug, Clone, Default)]
pub struct DbRuntimeHealth {
    pub is_available: bool,
    pub last_probe_at: Option<DateTime<Utc>>,
    pub last_failure_at: Option<DateTime<Utc>>,
    pub last_recovery_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

pub type DbRuntimeHealthState = Arc<RwLock<DbRuntimeHealth>>;

pub fn create_db_runtime_health_state() -> DbRuntimeHealthState {
    Arc::new(RwLock::new(DbRuntimeHealth {
        is_available: true,
        ..DbRuntimeHealth::default()
    }))
}

pub fn record_db_probe_result(
    health: &mut DbRuntimeHealth,
    current_ok: bool,
    now: DateTime<Utc>,
    error_message: Option<String>,
) {
    let was_available = health.is_available;
    health.is_available = current_ok;
    health.last_probe_at = Some(now);

    if current_ok {
        if !was_available {
            health.last_recovery_at = Some(now);
        }
        health.last_error = None;
    } else {
        if was_available || health.last_failure_at.is_none() {
            health.last_failure_at = Some(now);
        }
        health.last_error = error_message;
    }
}

// ============================================================================
// 数据库连接池初始化
// ============================================================================

/// 初始化数据库连接池
///
/// # 参数
/// - database: PostgreSQL 连接池配置
///
/// # 返回
/// - Ok(PgPool): 数据库连接池
/// - Err: 连接失败
///
/// # 示例
/// ```rust,no_run
/// use cyber_jianghu_server::init_db_pool;
/// use cyber_jianghu_server::config::Config;
///
/// # #[tokio::main]
/// # async fn main() -> anyhow::Result<()> {
/// let config = Config::default();
/// let pool = init_db_pool(&config.database).await?;
/// # Ok(())
/// # }
/// ```
pub async fn init_db_pool(database: &crate::config::DatabaseConfig) -> Result<PgPool> {
    // 隐藏密码用于日志输出
    let safe_url = database.url.split('@').next_back().unwrap_or("unknown");
    info!("初始化数据库连接池: {}", safe_url);

    let retry_delay = Duration::from_secs(database.retry_delay_secs);

    for attempt in 1..=database.max_retries {
        let options = PgPoolOptions::new()
            .max_connections(database.max_connections)
            .min_connections(database.min_connections)
            .acquire_timeout(Duration::from_secs(database.acquire_timeout_secs))
            .idle_timeout(Some(Duration::from_secs(database.idle_timeout_secs)))
            .max_lifetime(Some(Duration::from_secs(database.max_lifetime_secs)))
            .test_before_acquire(true);

        match options.connect(&database.url).await {
            Ok(pool) => {
                // 测试连接
                match sqlx::query("SELECT 1").fetch_one(&pool).await {
                    Ok(_) => {
                        info!("数据库连接池初始化成功");
                        return Ok(pool);
                    }
                    Err(e) => {
                        warn!("数据库测试查询失败: {}", e);
                        if attempt < database.max_retries {
                            info!(
                                "{} 秒后重试... ({}/{})",
                                retry_delay.as_secs(),
                                attempt,
                                database.max_retries
                            );
                            tokio::time::sleep(retry_delay).await;
                        }
                    }
                }
            }
            Err(e) => {
                warn!("数据库连接失败: {}", e);
                if attempt < database.max_retries {
                    info!(
                        "{} 秒后重试... ({}/{})",
                        retry_delay.as_secs(),
                        attempt,
                        database.max_retries
                    );
                    tokio::time::sleep(retry_delay).await;
                } else {
                    return Err(e).context(format!(
                        "Failed to connect to PostgreSQL after {} attempts",
                        database.max_retries
                    ));
                }
            }
        }
    }

    Err(anyhow::anyhow!("Failed to connect to database"))
}

/// 执行 migrations 目录下的所有 SQL 文件（复刻 docker-entrypoint.sh 逻辑）。
///
/// 全量重跑幂等 SQL（与 entrypoint 行为一致），不引入 _sqlx_migrations 追踪表。
/// 非 docker 部署（cargo run / 裸二进制）靠此函数保证 schema 就绪。
///
/// 路径探测顺序（适配多种部署形态）：
///   1. `crates/server/migrations` —— cargo run（仓库根为 cwd）
///   2. `migrations`               —— docker 容器（WORKDIR=/app，migrations 在 /app/migrations）
///   3. 都不存在                   —— warn 后返回 Ok（由 docker entrypoint 兜底执行）
pub async fn run_migrations(pool: &PgPool) -> Result<()> {
    // 多路径探测：cargo run 与 docker 两种 cwd 布局都覆盖
    let migration_dir = ["crates/server/migrations", "migrations"]
        .iter()
        .map(std::path::Path::new)
        .find(|p| p.is_dir());

    let Some(migration_dir) = migration_dir else {
        tracing::warn!(
            "迁移目录不存在（尝试过 crates/server/migrations 与 migrations）；\
             docker 部署由 entrypoint 处理，非 docker 部署请确认 cwd"
        );
        return Ok(());
    };

    let mut files: Vec<_> = std::fs::read_dir(migration_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "sql"))
        .collect();
    files.sort_by_key(|e| e.path());

    tracing::info!("[migration] 使用迁移目录: {}", migration_dir.display());

    for file in &files {
        let filename = file.file_name().to_string_lossy().to_string();
        let sql = std::fs::read_to_string(file.path())?;
        tracing::info!("[migration] 执行: {}", filename);
        // sqlx::raw_sql 支持多语句 raw 执行（迁移文件含 plpgsql $$ 块 + 多 CREATE/COMMENT）
        sqlx::raw_sql(&sql)
            .execute(pool)
            .await
            .map_err(|e| anyhow::anyhow!("迁移失败 {}: {}", filename, e))?;
    }
    tracing::info!("[migration] 全部完成 ({} 个文件)", files.len());
    Ok(())
}

/// 启动数据库运行期健康探针
pub fn start_db_health_probe(
    pool: PgPool,
    health_state: DbRuntimeHealthState,
    probe_interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(probe_interval);

        loop {
            interval.tick().await;
            let probe_result = sqlx::query("SELECT 1").execute(&pool).await;
            let current_ok = probe_result.is_ok();
            let error_message = probe_result.err().map(|e| e.to_string());
            let now = Utc::now();

            let mut health = health_state.write().expect("db runtime health lock poisoned");
            let previous_ok = health.is_available;
            record_db_probe_result(&mut health, current_ok, now, error_message.clone());

            if current_ok != previous_ok {
                if current_ok {
                    info!("数据库运行期健康探针恢复正常");
                } else {
                    error!(
                        "数据库运行期健康探针失败，连接可能已中断: {}",
                        error_message.as_deref().unwrap_or("unknown")
                    );
                }
            }
        }
    })
}

// ============================================================================
// 工具函数
// ============================================================================

/// 生成安全的认证 token
///
/// 格式: {uuid_v4}_{random_16_hex}
/// 提供约 192 位，远超普通 UUID v4 的 122 位
pub fn generate_secure_token() -> String {
    let uuid = Uuid::new_v4();
    let random_suffix: String = rand::rng()
        .sample_iter(rand::distr::Alphanumeric)
        .take(16)
        .map(char::from)
        .collect();

    format!("{}_{}", uuid, random_suffix)
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    #[tokio::test]
    #[ignore] // 需要数据库连接，默认忽略
    async fn test_init_db_pool() {
        // 使用环境变量 DATABASE_URL，便于 CI/CD 和本地测试
        let database_url =
            std::env::var("DATABASE_URL").expect("DATABASE_URL environment variable must be set");
        let config = crate::config::DatabaseConfig {
            url: database_url,
            max_retries: 5,
            retry_delay_secs: 2,
            max_connections: 20,
            min_connections: 1,
            acquire_timeout_secs: 5,
            idle_timeout_secs: 300,
            max_lifetime_secs: 1800,
            probe_interval_secs: 30,
        };
        let result = init_db_pool(&config).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_generate_secure_token() {
        let token = generate_secure_token();
        // Token should be in format: {uuid}_{16_chars}
        let parts: Vec<&str> = token.split('_').collect();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[1].len(), 16);
    }

    #[test]
    fn test_record_db_probe_failure_sets_failure_metadata() {
        let mut health = DbRuntimeHealth::default();
        let now = Utc.with_ymd_and_hms(2026, 6, 24, 12, 0, 0).unwrap();

        record_db_probe_result(&mut health, false, now, Some("db down".to_string()));

        assert!(!health.is_available);
        assert_eq!(health.last_probe_at, Some(now));
        assert_eq!(health.last_failure_at, Some(now));
        assert_eq!(health.last_recovery_at, None);
        assert_eq!(health.last_error.as_deref(), Some("db down"));
    }

    #[test]
    fn test_record_db_probe_recovery_clears_error_and_sets_recovery_time() {
        let failure_at = Utc.with_ymd_and_hms(2026, 6, 24, 12, 0, 0).unwrap();
        let recovery_at = Utc.with_ymd_and_hms(2026, 6, 24, 12, 5, 0).unwrap();
        let mut health = DbRuntimeHealth {
            is_available: false,
            last_probe_at: Some(failure_at),
            last_failure_at: Some(failure_at),
            last_recovery_at: None,
            last_error: Some("db down".to_string()),
        };

        record_db_probe_result(&mut health, true, recovery_at, None);

        assert!(health.is_available);
        assert_eq!(health.last_probe_at, Some(recovery_at));
        assert_eq!(health.last_failure_at, Some(failure_at));
        assert_eq!(health.last_recovery_at, Some(recovery_at));
        assert_eq!(health.last_error, None);
    }

    /// 验证 P1-17：必须存在迁移文件 `019_agent_daily_summaries_fk.sql`，
    /// 且包含 `agent_daily_summaries.agent_id` → `agents(agent_id)` 的
    /// `ON DELETE CASCADE` 外键约束，闭环孤儿行问题。
    #[test]
    fn test_p1_17_agent_daily_summaries_fk_migration_has_cascade() {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let migration = manifest_dir
            .join("migrations")
            .join("019_agent_daily_summaries_fk.sql");
        let sql = std::fs::read_to_string(&migration).unwrap_or_else(|e| {
            panic!(
                "P1-17 修复缺失：未找到迁移文件 {}（{}）",
                migration.display(),
                e
            )
        });

        let lower = sql.to_lowercase();
        assert!(
            lower.contains("alter table agent_daily_summaries"),
            "019 迁移必须 ALTER agent_daily_summaries 表，实际内容:\n{sql}"
        );
        assert!(
            lower.contains("references agents(agent_id)"),
            "019 迁移必须 REFERENCES agents(agent_id)，实际内容:\n{sql}"
        );
        assert!(
            lower.contains("on delete cascade"),
            "019 迁移必须包含 ON DELETE CASCADE，避免孤儿行；实际内容:\n{sql}"
        );
    }

    /// 验证 P1-12：必须存在迁移文件 `020_device_token_rotation.sql`，
    /// 为 `devices` 表加 `token_created_at` 与 `token_rotated_at` 列。
    /// 解决"设备 token 一次生成终身有效"的真实风险。
    #[test]
    fn test_p1_12_device_token_rotation_migration_adds_columns() {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let migration = manifest_dir
            .join("migrations")
            .join("020_device_token_rotation.sql");
        let sql = std::fs::read_to_string(&migration).unwrap_or_else(|e| {
            panic!(
                "P1-12 修复缺失：未找到迁移文件 {}（{}）",
                migration.display(),
                e
            )
        });

        let lower = sql.to_lowercase();
        assert!(
            lower.contains("alter table devices"),
            "020 迁移必须 ALTER devices 表，实际内容:\n{sql}"
        );
        assert!(
            lower.contains("token_created_at"),
            "020 迁移必须包含 token_created_at 列，实际内容:\n{sql}"
        );
        assert!(
            lower.contains("token_rotated_at"),
            "020 迁移必须包含 token_rotated_at 列（可空），实际内容:\n{sql}"
        );
    }
}
