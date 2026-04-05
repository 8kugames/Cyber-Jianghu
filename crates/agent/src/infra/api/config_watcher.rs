//! LLM 配置文件监听服务
//!
//! 使用 notify crate 监听配置文件变更，通过 broadcast channel 通知订阅者。
//!
//! 防日志风暴机制（双层防护）：
//! 1. 内容哈希比对：只有文件内容真正变化时才触发通知
//! 2. 时间窗口 debounce：合并 2s 内的多次事件为单次通知
//!
//! 这确保了即使 save_to_file() 的 atomic rename 触发 notify 事件，
//! 只要内容没变（如热重载后的写回），也不会产生多余的通知。

use anyhow::Result;
use notify::Watcher;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::broadcast;
use tracing::debug;

/// debounce 窗口：2 秒内的事件合并为一次通知
const DEBOUNCE_SECS: u64 = 2;

/// 计算文件内容的哈希值，用于检测真正的内容变更
fn content_hash(path: &std::path::Path) -> Option<u64> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    Some(hasher.finish())
}

pub struct ConfigWatcher {
    _watcher: notify::RecommendedWatcher,
    pub tx: broadcast::Sender<()>,
}

impl ConfigWatcher {
    pub fn new(config_path: PathBuf) -> Result<Self> {
        let (tx, _) = broadcast::channel(4);

        // 第一层：内容哈希 — 只有内容真正变化才通知（根治反馈循环）
        //
        // CRITICAL: 此防护依赖 lifecycle reload (lifecycle.rs:287-367) 只读不写 agent.yaml。
        // 如果 reload 路径开始写回 agent.yaml，内容哈希防护将失效。
        let initial_hash = content_hash(&config_path).unwrap_or(0);
        let last_hash = Arc::new(AtomicU64::new(initial_hash));
        let last_hash_clone = last_hash.clone();

        // 第二层：时间 debounce — 合并同窗口内的多次事件
        let last_fire = Arc::new(AtomicU64::new(0));
        let last_fire_clone = last_fire.clone();

        let tx_clone = tx.clone();
        let watched_path = config_path.clone();

        let mut watcher =
            notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
                if let Ok(event) = res
                    && (event.kind.is_modify() || event.kind.is_create())
                {
                    // 第一层：内容哈希比对（compare_exchange 防多线程竞态）
                    let current = content_hash(&watched_path);
                    let prev = last_hash_clone.load(Ordering::Acquire);
                    match current {
                        Some(h) if h != prev => {
                            // CAS：只有一个线程能成功更新哈希并继续
                            match last_hash_clone.compare_exchange(
                                prev,
                                h,
                                Ordering::AcqRel,
                                Ordering::Acquire,
                            ) {
                                Ok(_) => {}
                                Err(_) => return, // 另一个线程已处理
                            }
                        }
                        Some(_) => return, // 内容没变
                        None => return,    // 文件不存在
                    }

                    // 第二层：debounce 检查
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    let prev_fire = last_fire_clone.load(Ordering::Relaxed);
                    if now.saturating_sub(prev_fire) < DEBOUNCE_SECS * 1000 {
                        return;
                    }
                    last_fire_clone.store(now, Ordering::Relaxed);

                    // 延迟发送（debounce 窗口内合并后续事件）
                    let tx = tx_clone.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_secs(DEBOUNCE_SECS));
                        let _ = tx.send(());
                    });
                }
            })?;

        if config_path.exists() {
            watcher.watch(&config_path, notify::RecursiveMode::NonRecursive)?;
        } else {
            debug!("配置文件不存在，跳过监听: {:?}", config_path);
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
