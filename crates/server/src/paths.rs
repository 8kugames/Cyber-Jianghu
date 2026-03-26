use std::env;
use std::path::PathBuf;
use std::sync::OnceLock;

/// 获取配置目录路径
///
/// 优先级：
/// 1. 环境变量 CYBER_JIANGHU_CONFIG_DIR
/// 2. 当前目录下的 config/
/// 3. 开发环境 fallback: crates/server/config/
pub fn get_config_dir() -> PathBuf {
    static CONFIG_DIR: OnceLock<PathBuf> = OnceLock::new();
    CONFIG_DIR
        .get_or_init(|| {
            if let Ok(dir) = env::var("CYBER_JIANGHU_CONFIG_DIR") {
                return PathBuf::from(dir);
            }

            let current = PathBuf::from("config");
            if current.exists() {
                return current;
            }

            // Fallback for local development
            PathBuf::from("crates/server/config")
        })
        .clone()
}

/// 获取静态文件目录路径 (Dashboard)
///
/// 优先级：
/// 1. 环境变量 CYBER_JIANGHU_STATIC_DIR
/// 2. 当前目录下的 static/
/// 3. 开发环境 fallback: crates/server/static/
pub fn get_static_dir() -> PathBuf {
    static STATIC_DIR: OnceLock<PathBuf> = OnceLock::new();
    STATIC_DIR
        .get_or_init(|| {
            if let Ok(dir) = env::var("CYBER_JIANGHU_STATIC_DIR") {
                return PathBuf::from(dir);
            }

            let current = PathBuf::from("static");
            if current.exists() {
                return current;
            }

            // Fallback for local development
            PathBuf::from("crates/server/static")
        })
        .clone()
}

/// 获取日志目录路径
///
/// 优先级：
/// 1. 环境变量 CYBER_JIANGHU_LOGS_DIR
/// 2. 当前目录下的 logs/
/// 3. 默认: logs/
pub fn get_logs_dir() -> PathBuf {
    static LOGS_DIR: OnceLock<PathBuf> = OnceLock::new();
    LOGS_DIR
        .get_or_init(|| {
            if let Ok(dir) = env::var("CYBER_JIANGHU_LOGS_DIR") {
                return PathBuf::from(dir);
            }

            // 如果目录不存在，尝试创建它（在运行时）
            // 这里只返回路径，创建逻辑由调用者负责
            PathBuf::from("logs")
        })
        .clone()
}
