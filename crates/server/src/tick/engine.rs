// ============================================================================
// OpenClaw Cyber-Jianghu MVP Tick Engine (Re-export + Tests)
// ============================================================================
//
// 实时模式：Tick 退化为纯时钟，Intent 由 IntentWorker 实时处理。
// 具体实现已拆分为子模块（scheduler, realtime, processor 等）。
// ============================================================================

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {

    use crate::config::Config;

    /// 创建测试用的配置
    /// 注意：这个函数创建了测试用的配置，仅用于单元测试
    fn create_test_config() -> Config {
        Config {
            server: crate::config::ServerConfig {
                host: "0.0.0.0".to_string(),
                port: 23333,
                admin_read_token: None,
                admin_write_token: None,
            },
            database: crate::config::DatabaseConfig {
                url: "postgres://test".to_string(),
            },
        }
    }

    // 注意：以下测试需要真实的数据库连接池
    // 这些测试应该在集成测试环境中运行

    #[test]
    fn test_config_creation() {
        let config = create_test_config();
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 23333);
    }

    #[tokio::test]
    #[ignore] // 需要真实数据库连接
    async fn test_tick_engine_creation() {
        // 集成测试需要真实的数据库连接池
        // 使用 create_test_tick_engine_with_pool(db_pool) 进行测试
    }
}
