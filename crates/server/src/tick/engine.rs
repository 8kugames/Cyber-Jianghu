// ============================================================================
// OpenClaw Cyber-Jianghu MVP Tick Engine (Re-export)
// ============================================================================
//
// Tick引擎是整个游戏世界的心脏，负责：
// 1. 驱动游戏世界的时间流逝
// 2. 收集Agent上报的意图
// 3. 结算意图（处理动作、应用衰减）
// 4. 持久化状态到数据库
// 5. 广播新状态给所有Agent
//
// Tick执行流程（每60秒执行一次）：
// 1. 记录Tick开始
// 2. 收集所有Agent的意图（从IntentManager缓存）
// 3. 应用生理值衰减（饥饿、口渴）
// 4. 结算意图（处理动作）
// 5. 持久化状态到数据库
// 6. 广播新状态给所有Agent
// 7. 记录Tick完成
//
// 设计原则：
// 1. 单线程执行，避免并发问题
// 2. 每个Tick独立，失败不影响下一个Tick
// 3. 详细的性能日志，方便定位问题
// 4. 优雅的错误处理，不崩溃
//
// 模块拆分：
// 本文件已拆分为多个子模块，每个模块负责特定功能：
// - scheduler.rs: 主循环和阶段协调（TickScheduler）
// - event_manager.rs: 事件管理（EventManager）
// - intent_collector.rs: 意图收集（IntentCollector）
// - broadcaster.rs: 状态广播（Broadcaster）
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
