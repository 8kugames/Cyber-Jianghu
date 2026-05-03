// ============================================================================
// OpenClaw Cyber-Jianghu MVP 数据库公共模块
// ============================================================================
//
// 本模块提供数据库连接池初始化和共享工具函数

use anyhow::{Context, Result};
use sqlx::PgPool;
use std::time::Duration;
use tracing::{info, warn};
use uuid::Uuid;

use rand::RngExt;

// ============================================================================
// 数据库连接池初始化
// ============================================================================

/// 初始化数据库连接池
///
/// # 参数
/// - database_url: PostgreSQL连接URL
///
/// # 返回
/// - Ok(PgPool): 数据库连接池
/// - Err: 连接失败
///
/// # 示例
/// ```rust,no_run
/// use cyber_jianghu_server::init_db_pool;
///
/// # #[tokio::main]
/// # async fn main() -> anyhow::Result<()> {
/// let pool = init_db_pool("postgres://user:pass@localhost/db", 5, 2).await?;
/// # Ok(())
/// # }
/// ```
pub async fn init_db_pool(
    database_url: &str,
    max_retries: u32,
    retry_delay_secs: u64,
) -> Result<PgPool> {
    // 隐藏密码用于日志输出
    let safe_url = database_url.split('@').next_back().unwrap_or("unknown");
    info!("初始化数据库连接池: {}", safe_url);

    let retry_delay = Duration::from_secs(retry_delay_secs);

    for attempt in 1..=max_retries {
        match PgPool::connect(database_url).await {
            Ok(pool) => {
                // 测试连接
                match sqlx::query("SELECT 1").fetch_one(&pool).await {
                    Ok(_) => {
                        info!("数据库连接池初始化成功");
                        return Ok(pool);
                    }
                    Err(e) => {
                        warn!("数据库测试查询失败: {}", e);
                        if attempt < max_retries {
                            info!(
                                "{} 秒后重试... ({}/{})",
                                retry_delay.as_secs(),
                                attempt,
                                max_retries
                            );
                            tokio::time::sleep(retry_delay).await;
                        }
                    }
                }
            }
            Err(e) => {
                warn!("数据库连接失败: {}", e);
                if attempt < max_retries {
                    info!(
                        "{} 秒后重试... ({}/{})",
                        retry_delay.as_secs(),
                        attempt,
                        max_retries
                    );
                    tokio::time::sleep(retry_delay).await;
                } else {
                    return Err(e).context(format!(
                        "Failed to connect to PostgreSQL after {} attempts",
                        max_retries
                    ));
                }
            }
        }
    }

    Err(anyhow::anyhow!("Failed to connect to database"))
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

    #[tokio::test]
    #[ignore] // 需要数据库连接，默认忽略
    async fn test_init_db_pool() {
        // 使用环境变量 DATABASE_URL，便于 CI/CD 和本地测试
        let database_url =
            std::env::var("DATABASE_URL").expect("DATABASE_URL environment variable must be set");
        let result = init_db_pool(&database_url, 5, 2).await;
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
}
