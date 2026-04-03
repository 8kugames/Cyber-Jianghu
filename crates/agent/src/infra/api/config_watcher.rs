//! LLM 配置文件监听服务
//!
//! 使用 notify crate 监听配置文件变更，通过 broadcast channel 通知订阅者
//! 内置 debounce：macOS 上 notify 对单次修改触发多个事件（metadata + data + modify），
//! 合并 100ms 窗口内的事件为单次通知

use anyhow::Result;
use notify::Watcher;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::info;

pub struct ConfigWatcher {
    _watcher: notify::RecommendedWatcher,
    pub tx: broadcast::Sender<()>,
}

impl ConfigWatcher {
    pub fn new(config_path: PathBuf) -> Result<Self> {
        let (tx, _) = broadcast::channel(4);

        let pending = Arc::new(AtomicBool::new(false));
        let pending_clone = pending.clone();
        let tx_clone = tx.clone();

        // notify 回调在独立线程运行，不能用 tokio::spawn（无 runtime context）
        // 用 std::thread 做 debounce
        let mut watcher =
            notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
                if let Ok(event) = res
                    && (event.kind.is_modify() || event.kind.is_create())
                {
                    info!("检测到配置文件变更: {:?}", event.paths);
                    if pending_clone
                        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                    {
                        let tx = tx_clone.clone();
                        let pending = pending_clone.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(std::time::Duration::from_millis(100));
                            pending.store(false, Ordering::SeqCst);
                            let _ = tx.send(());
                        });
                    }
                }
            })?;

        if config_path.exists() {
            watcher.watch(&config_path, notify::RecursiveMode::NonRecursive)?;
        } else {
            info!("配置文件不存在，跳过监听: {:?}", config_path);
        }

        Ok(Self {
            _watcher: watcher,
            tx,
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.tx.subscribe()
    }
}
