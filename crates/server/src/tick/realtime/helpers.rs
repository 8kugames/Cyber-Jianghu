// ============================================================================
// worker channel 构造
// ============================================================================

use super::WorkerMessage;
use tokio::sync::mpsc;

// ============================================================================

/// 创建 IntentWorker channel（有界，容量 256）
pub fn create_worker_channel() -> (mpsc::Sender<WorkerMessage>, mpsc::Receiver<WorkerMessage>) {
    mpsc::channel(256)
}
