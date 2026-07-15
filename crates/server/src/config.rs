// ============================================================================
// OpenClaw Cyber-Jianghu MVP 配置模块
// ============================================================================
//
// 本模块负责配置的加载和管理，包括：
// - 服务端配置（主机、端口）
// - Tick引擎配置（周期）
// - 数据库配置（PostgreSQL）
//
// 配置来源：
// 1. 环境变量（优先级最高）
// 2. 默认值
//
// 设计原则：
// 1. 环境变量优先，方便Docker部署
// 2. 提供合理的默认值
// 3. 必需的配置项明确标注
// 4. 配置验证，避免运行时错误
// ============================================================================

use anyhow::{Context, Result};
use serde::Deserialize;

// ============================================================================
// 配置结构定义
// ============================================================================

/// 主配置结构
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// 服务端配置
    pub server: ServerConfig,

    /// 数据库配置
    pub database: DatabaseConfig,
}

/// 服务端配置
#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    /// 监听地址
    /// 0.0.0.0 表示监听所有网卡（Docker容器内使用）
    /// 127.0.0.1 表示仅本地访问
    pub host: String,

    /// 监听端口
    /// 默认: 23333
    pub port: u16,

    /// 管理员读 Token (可选)
    /// 如果设置，则使用该 Token，否则自动生成
    pub admin_read_token: Option<String>,

    /// 管理员读写 Token (可选)
    /// 如果设置，则使用该 Token，否则自动生成
    pub admin_write_token: Option<String>,

    /// 游戏客户端只读 Token (可选)
    ///
    /// 给前端/游戏客户端使用的低权限只读档：只能命中 dashboard 的 READ 端点，
    /// 不能调任何 WRITE 端点（config 编辑、chronicle 生成、agent cleanup 等）。
    ///
    /// None 表示禁用客户端鉴权档 —— 此时 require_client_read_token 回退接受
    /// admin read token（保持向后兼容）。
    /// 不自动生成：客户端档应由部署方显式下发，避免随机 token 泄漏后被滥用。
    pub client_read_token: Option<String>,
}

/// 数据库配置
#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    /// PostgreSQL连接URL
    /// 格式: postgres://用户名:密码@主机:端口/数据库名
    pub url: String,

    /// 连接重试次数
    #[serde(default = "default_db_max_retries")]
    pub max_retries: u32,

    /// 重试间隔（秒）
    #[serde(default = "default_db_retry_delay_secs")]
    pub retry_delay_secs: u64,

    /// 连接池最大连接数
    #[serde(default = "default_db_max_connections")]
    pub max_connections: u32,

    /// 连接池最小空闲连接数
    #[serde(default = "default_db_min_connections")]
    pub min_connections: u32,

    /// 获取连接超时（秒）
    #[serde(default = "default_db_acquire_timeout_secs")]
    pub acquire_timeout_secs: u64,

    /// 空闲连接回收时间（秒）
    #[serde(default = "default_db_idle_timeout_secs")]
    pub idle_timeout_secs: u64,

    /// 连接最大生命周期（秒）
    #[serde(default = "default_db_max_lifetime_secs")]
    pub max_lifetime_secs: u64,

    /// 后台探针轮询间隔（秒）
    #[serde(default = "default_db_probe_interval_secs")]
    pub probe_interval_secs: u64,
}

fn default_db_max_retries() -> u32 {
    5
}

fn default_db_retry_delay_secs() -> u64 {
    2
}

fn default_db_max_connections() -> u32 {
    20
}

fn default_db_min_connections() -> u32 {
    2
}

fn default_db_acquire_timeout_secs() -> u64 {
    5
}

fn default_db_idle_timeout_secs() -> u64 {
    300
}

fn default_db_max_lifetime_secs() -> u64 {
    1800
}

fn default_db_probe_interval_secs() -> u64 {
    30
}

// ============================================================================
// 配置加载
// ============================================================================

impl Config {
    /// 从环境变量和配置文件加载配置
    ///
    /// 优先级：
    /// 1. 环境变量（最高优先级）
    /// 2. 配置文件（config.yaml）
    /// 3. 默认值（最低优先级）
    ///
    /// 必需的环境变量：
    /// - DATABASE_URL: PostgreSQL连接URL
    ///
    /// 可选的环境变量：
    /// - SERVER_HOST: 服务端监听地址（默认: 0.0.0.0）
    /// - SERVER_PORT: 服务端监听端口（默认: 23333）
    /// - TICK_DURATION_SECS: Tick周期（默认: 60）
    pub fn load() -> Result<Self> {
        // 加载.env文件（如果存在）
        // 这会在开发环境下从.env文件加载环境变量
        dotenv::dotenv().ok();

        // 从环境变量读取服务端配置
        let server = ServerConfig {
            host: std::env::var("SERVER_HOST").unwrap_or_else(|_| "0.0.0.0".to_string()),
            port: std::env::var("SERVER_PORT")
                .unwrap_or_else(|_| "23333".to_string())
                .parse()
                .context("SERVER_PORT must be a valid port number")?,
            admin_read_token: std::env::var("ADMIN_READ_TOKEN")
                .ok()
                .filter(|s| !s.is_empty()),
            admin_write_token: std::env::var("ADMIN_WRITE_TOKEN")
                .ok()
                .filter(|s| !s.is_empty()),
            client_read_token: std::env::var("CLIENT_READ_TOKEN")
                .ok()
                .filter(|s| !s.is_empty()),
        };

        // 从环境变量读取数据库配置（必需）
        let database = DatabaseConfig {
            url: std::env::var("DATABASE_URL")
                .context("DATABASE_URL environment variable must be set")?,
            max_retries: std::env::var("DB_MAX_RETRIES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(5),
            retry_delay_secs: std::env::var("DB_RETRY_DELAY_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(2),
            max_connections: std::env::var("DB_MAX_CONNECTIONS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(default_db_max_connections),
            min_connections: std::env::var("DB_MIN_CONNECTIONS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(default_db_min_connections),
            acquire_timeout_secs: std::env::var("DB_ACQUIRE_TIMEOUT_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(default_db_acquire_timeout_secs),
            idle_timeout_secs: std::env::var("DB_IDLE_TIMEOUT_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(default_db_idle_timeout_secs),
            max_lifetime_secs: std::env::var("DB_MAX_LIFETIME_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(default_db_max_lifetime_secs),
            probe_interval_secs: std::env::var("DB_PROBE_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(default_db_probe_interval_secs),
        };

        Ok(Config { server, database })
    }

    /// 验证配置的有效性
    ///
    /// 检查：
    /// - 端口号在有效范围内（1-65535）
    /// - 数据库URL格式正确
    /// - 数据库密码不是默认值（生产环境安全检查）
    pub fn validate(&self) -> Result<()> {
        // 验证端口号
        if self.server.port == 0 {
            anyhow::bail!("Server port cannot be 0");
        }

        // 验证数据库URL（简单验证）
        if !self.database.url.starts_with("postgres://") {
            anyhow::bail!("Database URL must start with 'postgres://'");
        }

        if self.database.max_connections == 0 {
            anyhow::bail!("DB_MAX_CONNECTIONS must be greater than 0");
        }
        if self.database.min_connections > self.database.max_connections {
            anyhow::bail!("DB_MIN_CONNECTIONS cannot be greater than DB_MAX_CONNECTIONS");
        }
        if self.database.acquire_timeout_secs == 0 {
            anyhow::bail!("DB_ACQUIRE_TIMEOUT_SECS must be greater than 0");
        }
        if self.database.max_lifetime_secs == 0 {
            anyhow::bail!("DB_MAX_LIFETIME_SECS must be greater than 0");
        }
        if self.database.probe_interval_secs == 0 {
            anyhow::bail!("DB_PROBE_INTERVAL_SECS must be greater than 0");
        }

        // 检查是否在生产环境使用默认密码"changeme"
        // 只在生产环境(ENVIRONMENT=production)时检查，否则允许开发环境使用默认密码
        let is_production = std::env::var("ENVIRONMENT")
            .map(|v| v == "production")
            .unwrap_or(false);

        if is_production
            && (self.database.url.contains(":changeme@")
                || self.database.url.contains(":changeme/"))
        {
            anyhow::bail!(
                "Database password 'changeme' is not allowed in production.\n\
                Solutions:\n\
                1. Use './install.sh server start --prod' which auto-generates a secure password\n\
                2. Or manually set DB_PASSWORD in crates/server/.env file\n\
                3. Or set DATABASE_URL environment variable with a secure password"
            );
        }

        Ok(())
    }
}

// ============================================================================
// 默认值
// ============================================================================

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                host: "0.0.0.0".to_string(),
                port: 23333,
                admin_read_token: None,
                admin_write_token: None,
                client_read_token: None,
            },
            database: DatabaseConfig {
                url: "postgres://postgres:changeme@localhost:5432/cyber_jianghu".to_string(),
                max_retries: 5,
                retry_delay_secs: 2,
                max_connections: default_db_max_connections(),
                min_connections: default_db_min_connections(),
                acquire_timeout_secs: default_db_acquire_timeout_secs(),
                idle_timeout_secs: default_db_idle_timeout_secs(),
                max_lifetime_secs: default_db_max_lifetime_secs(),
                probe_interval_secs: default_db_probe_interval_secs(),
            },
        }
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 23333);
        assert_eq!(config.database.max_connections, default_db_max_connections());
        assert_eq!(config.database.min_connections, default_db_min_connections());
    }

    #[test]
    fn test_config_validation() {
        let mut config = Config::default();

        // 有效配置应该通过验证
        config.database.url = "postgres://test@localhost/db".to_string();
        assert!(config.validate().is_ok());

        // 端口号为0应该失败
        config.server.port = 0;
        assert!(config.validate().is_err());

        // 恢复端口号
        config.server.port = 23333;

        // 数据库URL格式错误应该失败
        config.database.url = "mysql://test".to_string();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_load_from_env() {
        // 设置环境变量
        unsafe {
            std::env::set_var("SERVER_HOST", "127.0.0.1");
            std::env::set_var("SERVER_PORT", "9090");
            std::env::set_var("DATABASE_URL", "postgres://test@localhost/testdb");
        }

        // 加载配置
        let config = Config::load().unwrap();

        // 验证配置值
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.port, 9090);
        assert_eq!(config.database.url, "postgres://test@localhost/testdb");

        // 清理环境变量
        unsafe {
            std::env::remove_var("SERVER_HOST");
            std::env::remove_var("SERVER_PORT");
            std::env::remove_var("DATABASE_URL");
        }
    }
}
