// ============================================================================
// 训练 Trace 结构化落盘（训练数据采集）
// ============================================================================
//
// 将 agent 的 LLM 调用从"日志文本"升级为"训练可读的结构化 JSONL trace"。
// 与 thinking_log 并列（thinking_log 给人看，trace 给训练吃），职责分离。
//
// 当前覆盖：人魂 + 天魂（三要素中语义+全文可得，token 标 None）。
// 地魂不做：run_tool_loop 无 agent_id + 共享路径无法区分 soul_stage（架构限制）。
// 详见 docs/plans/2026-06-26-training-trace-structured-logging.md。
// ============================================================================

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use uuid::Uuid;

// ============================================================================
// 配置（强制配置，缺失即 fail-fast，对齐 reward_loader 模式）
// ============================================================================

/// Trace 配置（对应 trace.yaml）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceConfig {
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub output: TraceOutputConfig,
    #[serde(default)]
    pub upload: TraceUploadConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceOutputConfig {
    /// 默认开（用户要求：支持训练专用模型，默认为开）
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_base_dir")]
    pub base_dir: String,
    /// 日志总体积上限（MB），超过则按 LRU 删除最旧文件。默认 1024 MB（1 GB）
    #[serde(default = "default_max_size_mb")]
    pub max_size_mb: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceUploadConfig {
    /// 默认开（开时回传 server，关时仅本地）
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
}

fn default_enabled() -> bool {
    true
}
fn default_base_dir() -> String {
    "traces".to_string()
}
fn default_max_size_mb() -> u64 {
    1024
}
fn default_batch_size() -> usize {
    32
}

impl Default for TraceOutputConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            base_dir: default_base_dir(),
            max_size_mb: default_max_size_mb(),
        }
    }
}

impl Default for TraceUploadConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            batch_size: default_batch_size(),
        }
    }
}

impl TraceConfig {
    /// 从 config_dir 加载 trace.yaml（强制配置，缺失即 Err）
    pub fn load(config_dir: &Path) -> Result<Self> {
        let yaml_path = config_dir.join("trace.yaml");
        if !yaml_path.exists() {
            return Err(anyhow::anyhow!(
                "[trace] trace.yaml 未找到于 {}（trace 为强制配置，启用训练数据采集时必须存在）",
                yaml_path.display()
            ));
        }
        let content = std::fs::read_to_string(&yaml_path)
            .with_context(|| format!("读取 trace.yaml 失败: {:?}", yaml_path))?;
        serde_yaml::from_str(&content).context("解析 trace.yaml 失败")
    }
}

// ============================================================================
// Trace 数据结构
// ============================================================================

/// 一次 LLM 调用的结构化 trace（训练样本）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmTrace {
    /// 全局唯一 trace id
    pub trace_id: String,
    /// agent 标识（与 reward 按 agent_id 对齐）
    pub agent_id: Uuid,
    /// agent 当前角色名（叙事用）
    pub character_name: String,
    /// tick 锚点（与 reward/soul_cycle 按 tick_id 对齐）
    pub tick_id: i64,
    /// 哪一魂产生的调用（训练分类标签）
    pub soul_stage: SoulStage,
    /// 同一 tick 的重试次数（天魂驳回后人魂重来）
    pub attempt: i32,
    /// 模型信息
    pub provider: String,
    pub model: String,
    /// 角色设定（agent 特有部分，~200 bytes；静态 system 模板由项目配置复用，不重复记录）
    /// 训练时：从此字段重建 persona + 从 prompt_templates.yaml 渲染静态部分 = 完整 system_prompt
    pub persona_name: String,
    pub persona_description: String,
    /// I/O 全文（训练核心数据）
    pub user_prompt: String,
    pub response: String,
    /// token 数（架构限制：当前阶段调用方拿不到，标 None）
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    /// 是否成功
    pub ok: bool,
    /// 记录时间
    pub wall_clock: chrono::DateTime<chrono::Utc>,
}

/// 灵魂阶段（训练分类标签）
///
/// 注意：地魂不作为独立分类——地魂只是工具池，其 tool-calling 循环是人魂
/// 带工具调用（complete_json_with_conversation_and_tools）的内部轮次，
/// 实际调用 LLM 的始终是人魂。故地魂产生的 LLM 调用已包含在人魂 trace 中。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SoulStage {
    /// 人魂（动机推演与规划，含带工具的 tool-calling 轮次）
    Renhun,
    /// 天魂（规则与世界观审查）
    Tianhun,
}

// ============================================================================
// Recorder（同步 Mutex 聚合，复用 token_tracking.rs 模式）
// ============================================================================

static TRACE_BUFFER: OnceLock<Mutex<Vec<LlmTrace>>> = OnceLock::new();
static TRACE_CONFIG: OnceLock<Option<TraceConfig>> = OnceLock::new();
/// 回传 sender：在 agent websocket 连接成功后通过 set_upload_sender 注入
/// （不在 init_trace_recorder 时传入——那时连接尚未建立）
static UPLOAD_SENDER: OnceLock<
    Option<tokio::sync::mpsc::Sender<cyber_jianghu_protocol::ClientMessage>>,
> = OnceLock::new();

fn trace_buffer() -> &'static Mutex<Vec<LlmTrace>> {
    TRACE_BUFFER.get_or_init(|| Mutex::new(Vec::new()))
}

/// 初始化 trace recorder（bin 入口调用，在 init_thinking_log 之后）。
///
/// 若 trace.yaml enabled=false 或缺失，recorder 不初始化，record() 空操作。
/// 若 enabled=true，启动后台 flush task 定时写盘。
/// 回传 sender 通过 set_upload_sender 在 agent 连接成功后单独注入（连接在 init 之后建立）。
pub fn init_trace_recorder(config_dir: &Path) {
    let cfg = match TraceConfig::load(config_dir) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("[trace] 配置加载失败，trace 不启用: {}", e);
            let _ = TRACE_CONFIG.set(None);
            return;
        }
    };

    if !cfg.output.enabled {
        tracing::info!("[trace] trace 未启用（output.enabled=false）");
        let _ = TRACE_CONFIG.set(Some(cfg));
        return;
    }

    tracing::info!(
        "[trace] trace 已启用，输出目录: {}/{}",
        crate::config::data_base_dir().display(),
        cfg.output.base_dir
    );
    let _ = TRACE_CONFIG.set(Some(cfg.clone()));

    // 启动后台 flush task
    tokio::spawn(async move {
        flush_loop(cfg).await;
    });
}

/// 注入回传 sender（在 agent websocket 连接成功后调用）。
///
/// 代理1校准：init_trace_recorder 在 main 顶端调用（连接前），此时无 sender。
/// 真实路径是 agent 连接成功后通过 intent_sender() 获取 sender，再调此函数注入。
pub fn set_upload_sender(sender: tokio::sync::mpsc::Sender<cyber_jianghu_protocol::ClientMessage>) {
    let _ = UPLOAD_SENDER.set(Some(sender));
    tracing::info!("[trace] 回传 sender 已注入，trace 将回传 server");
}

/// 调用方记录 trace（同步 Vec push，非阻塞——O(1)，无 I/O）。
///
/// fire-and-forget：失败只丢 trace，不 panic，不影响 agent tick。
/// 直接记录原文——本项目所有玩家角色均为 LLM 驱动，无真人隐私内容。
pub fn record(trace: LlmTrace) {
    // 配置未加载或未启用 → 空操作
    let cfg = match TRACE_CONFIG.get().and_then(|c| c.as_ref()) {
        Some(c) if c.output.enabled => c,
        _ => return,
    };
    let _ = cfg; // 配置已检查 enabled，push 不再读 cfg

    if let Ok(mut buf) = trace_buffer().lock() {
        buf.push(trace);
    }
}

/// 后台 flush 循环：定时将缓冲区 trace 批量写盘 + 回传。
async fn flush_loop(cfg: TraceConfig) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
    loop {
        interval.tick().await;
        let batch: Vec<LlmTrace> = {
            let mut buf = trace_buffer().lock().expect("poisoned");
            std::mem::take(&mut *buf)
        };
        if batch.is_empty() {
            continue;
        }

        // 1. 本地落盘（始终执行）
        if let Err(e) = write_batch(&batch, &cfg).await {
            tracing::error!("[trace] 本地落盘失败: {}", e);
        }

        // 2. 回传 server（若 upload.enabled 且 sender 已注入）
        if cfg.upload.enabled
            && let Some(sender) = UPLOAD_SENDER.get().and_then(|s| s.as_ref())
        {
            let entries: Vec<cyber_jianghu_protocol::TraceEntry> = batch
                .iter()
                .map(|t| cyber_jianghu_protocol::TraceEntry {
                    trace_id: t.trace_id.clone(),
                    agent_id: t.agent_id,
                    character_name: t.character_name.clone(),
                    tick_id: t.tick_id,
                    soul_stage: serde_json::to_string(&t.soul_stage)
                        .unwrap_or_default()
                        .trim_matches('"')
                        .to_string(),
                    attempt: t.attempt,
                    provider: t.provider.clone(),
                    model: t.model.clone(),
                    persona_name: t.persona_name.clone(),
                    persona_description: t.persona_description.clone(),
                    user_prompt: t.user_prompt.clone(),
                    response: t.response.clone(),
                    prompt_tokens: t.prompt_tokens,
                    completion_tokens: t.completion_tokens,
                    ok: t.ok,
                    wall_clock: Some(t.wall_clock.timestamp_millis()),
                })
                .collect();
            let msg = cyber_jianghu_protocol::ClientMessage::TraceReport { traces: entries };
            // 失败只丢回传，本地已有完整副本（不丢数据）
            if let Err(e) = sender.send(msg).await {
                tracing::warn!("[trace] 回传失败（本地仍有副本）: {}", e);
            }
        }
    }
}

/// 批量写入 trace 到 JSONL（按 soul_stage + agent_id + date 分区）。
///
/// 文件名含 agent_id 避免多 agent 同机并发写冲突（并发修复）。
async fn write_batch(traces: &[LlmTrace], cfg: &TraceConfig) -> Result<()> {
    use std::collections::HashMap;

    // 按 (soul_stage, agent_id, date) 分组
    let mut groups: HashMap<(String, String, String), Vec<&LlmTrace>> = HashMap::new();
    for trace in traces {
        let soul = serde_json::to_string(&trace.soul_stage)?
            .trim_matches('"')
            .to_string();
        let agent = trace.agent_id.to_string();
        let date = trace.wall_clock.format("%Y-%m-%d").to_string();
        groups.entry((soul, agent, date)).or_default().push(trace);
    }

    let base = crate::config::data_base_dir().join(&cfg.output.base_dir);

    for ((soul, agent, date), group) in groups {
        // 路径含 agent=<id>，消除多 agent 同机并发写冲突
        let dir = base
            .join(format!("soul={}", soul))
            .join(format!("agent={}", agent));
        tokio::fs::create_dir_all(&dir).await?;
        let path = dir.join(format!("date={}.jsonl", date));

        let mut content = String::new();
        for trace in group {
            content.push_str(&serde_json::to_string(trace)?);
            content.push('\n');
        }

        // 追加模式（同一天可能多次 flush）
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;
        file.write_all(content.as_bytes()).await?;
    }

    // 滚动覆盖：写完后检查总体积，超过上限则按 LRU 删除最旧文件
    enforce_max_size(&base, cfg.output.max_size_mb).await;

    Ok(())
}

/// 滚动覆盖：检查 traces/ 总体积，超过 max_size_mb 则按文件修改时间删除最旧的文件。
///
/// LRU 策略：遍历所有 *.jsonl，按 modified time 排序，从最旧开始删除，
/// 直到总体积降到 max_size_mb 以下。
async fn enforce_max_size(base: &std::path::Path, max_size_mb: u64) {
    let max_bytes = max_size_mb * 1024 * 1024;

    // 栈式遍历收集所有 jsonl 文件（避免 async 递归）
    let mut files: Vec<(std::path::PathBuf, u64, std::time::SystemTime)> = Vec::new();
    let mut total: u64 = 0;
    let mut stack = vec![base.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let mut reader = match tokio::fs::read_dir(&dir).await {
            Ok(r) => r,
            Err(_) => continue,
        };
        while let Ok(Some(entry)) = reader.next_entry().await {
            let path = entry.path();
            let metadata = match entry.metadata().await {
                Ok(m) => m,
                Err(_) => continue,
            };
            if metadata.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "jsonl") {
                let mtime = metadata.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                let size = metadata.len();
                files.push((path, size, mtime));
                total += size;
            }
        }
    }

    if total <= max_bytes {
        return;
    }

    // 按修改时间排序（最旧在前）
    files.sort_by_key(|(_, _, mtime)| *mtime);

    let mut deleted = 0u64;
    for (path, size, _) in &files {
        if total <= max_bytes {
            break;
        }
        if tokio::fs::remove_file(path).await.is_ok() {
            total -= size;
            deleted += size;
        }
    }

    if deleted > 0 {
        tracing::info!(
            "[trace] 滚动覆盖：删除最旧文件释放 {:.1} MB（当前 {:.1} MB / 上限 {} MB）",
            deleted as f64 / 1024.0 / 1024.0,
            total as f64 / 1024.0 / 1024.0,
            max_size_mb
        );
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_is_enabled() {
        // 默认开（用户要求）
        assert!(TraceOutputConfig::default().enabled, "output 默认必须开");
        assert!(TraceUploadConfig::default().enabled, "upload 默认必须开");
    }

    #[test]
    fn test_trace_config_load_with_upload() {
        // 完整配置（含 upload）可正确加载
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("trace.yaml"),
            "version: \"0.0.2\"\noutput:\n  enabled: true\n  base_dir: \"traces\"\nupload:\n  enabled: false\n",
        )
        .unwrap();
        let cfg = TraceConfig::load(tmp.path()).unwrap();
        assert!(cfg.output.enabled);
        assert!(!cfg.upload.enabled, "upload.enabled=false 应被读取");
    }

    #[test]
    fn test_trace_config_load_missing_fail_fast() {
        // trace.yaml 缺失必须 Err（非静默）
        let tmp = tempfile::TempDir::new().unwrap();
        let result = TraceConfig::load(tmp.path());
        assert!(result.is_err(), "缺失 trace.yaml 必须 fail-fast 返回 Err");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("trace") && err_msg.contains("强制"),
            "错误信息应说明 trace 为强制配置，got: {}",
            err_msg
        );
    }

    #[test]
    fn test_record_is_sync_no_async() {
        // record() 是同步函数（编译保证无 await）
        let trace = LlmTrace {
            trace_id: Uuid::new_v4().to_string(),
            agent_id: Uuid::new_v4(),
            character_name: "测试".to_string(),
            tick_id: 1,
            soul_stage: SoulStage::Renhun,
            attempt: 0,
            provider: "test".to_string(),
            model: "test".to_string(),
            persona_name: "测试".to_string(),
            persona_description: "测试描述".to_string(),
            user_prompt: "prompt".to_string(),
            response: "response".to_string(),
            prompt_tokens: None,
            completion_tokens: None,
            ok: true,
            wall_clock: chrono::Utc::now(),
        };
        record(trace);
    }

    #[test]
    fn test_llm_trace_serializes_token_as_null() {
        // token 为 None 时序列化为 null（非 0）
        let trace = LlmTrace {
            trace_id: "test".to_string(),
            agent_id: Uuid::nil(),
            character_name: "测试".to_string(),
            tick_id: 1,
            soul_stage: SoulStage::Renhun,
            attempt: 0,
            provider: "test".to_string(),
            model: "test".to_string(),
            persona_name: "测试".to_string(),
            persona_description: "测试描述".to_string(),
            user_prompt: "p".to_string(),
            response: "r".to_string(),
            prompt_tokens: None,
            completion_tokens: None,
            ok: true,
            wall_clock: chrono::Utc::now(),
        };
        let json = serde_json::to_string(&trace).unwrap();
        assert!(
            json.contains("\"prompt_tokens\":null"),
            "token None 应序列化为 null，got: {}",
            json
        );
        assert!(
            json.contains("\"agent_id\":\"00000000-0000-0000-0000-000000000000\""),
            "agent_id 应是 UUID 格式"
        );
    }

    #[test]
    fn test_soul_stage_serializes_snake_case() {
        let json = serde_json::to_string(&SoulStage::Renhun).unwrap();
        assert_eq!(json, "\"renhun\"");
        let json = serde_json::to_string(&SoulStage::Tianhun).unwrap();
        assert_eq!(json, "\"tianhun\"");
    }
}
