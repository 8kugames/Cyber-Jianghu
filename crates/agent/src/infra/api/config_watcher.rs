//! LLM 配置文件监听服务
//!
//! 使用 notify crate 监听配置文件变更，通过 broadcast channel 通知订阅者

use anyhow::Result;
use notify::Watcher;
use std::path::PathBuf;
use tokio::sync::broadcast;
use tracing::{info, warn};

/// LLM 配置文件监听器
pub struct ConfigWatcher {
    /// 内部 notify watcher（keep alive 确保不被释放）
    _watcher: notify::RecommendedWatcher,
    /// 广播发送端
    pub tx: broadcast::Sender<()>,
}

impl ConfigWatcher {
    /// 创建新的文件监听器
    ///
    /// # 参数
    /// - `config_path`: 要监听的配置文件路径
    pub fn new(config_path: PathBuf) -> Result<Self> {
        let (tx, _) = broadcast::channel(1);
        let tx_clone = tx.clone();

        let mut watcher =
            notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
                match res {
                    Ok(event) if event.kind.is_modify() || event.kind.is_create() => {
                        info!("检测到配置文件变更: {:?}", event.paths);
                        // 忽略发送错误（可能没有接收端）
                        let _ = tx_clone.send(());
                    }
                    Err(e) => {
                        warn!("文件监听错误: {:?}", e);
                    }
                    _ => {}
                }
            })?;

        match watcher.watch(&config_path, notify::RecursiveMode::NonRecursive) {
            Ok(()) => {}
            Err(e) if matches!(e.kind, notify::ErrorKind::PathNotFound) => {
                warn!("配置文件不存在，跳过监听: {:?}", config_path);
            }
            Err(e) => return Err(e.into()),
        }

        Ok(Self {
            _watcher: watcher,
            tx,
        })
    }

    /// 创建订阅接收端
    ///
    /// 用于 ActorSoul 和 ReflectorSoul 订阅配置变更通知
    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.tx.subscribe()
    }
}
