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
    pub output: TraceOutputConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceOutputConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_base_dir")]
    pub base_dir: String,
}

fn default_base_dir() -> String {
    "traces".to_string()
}

impl Default for TraceOutputConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_dir: default_base_dir(),
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
    /// I/O 全文（训练核心数据）
    pub system_prompt: String,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SoulStage {
    /// 人魂（动机推演与规划）
    Renhun,
    /// 地魂（预留，当前不产出——架构限制：run_tool_loop 无 agent_id）
    Earth,
    /// 天魂（规则与世界观审查）
    Tianhun,
}

// ============================================================================
// Recorder（同步 Mutex 聚合，复用 token_tracking.rs 模式）
// ============================================================================

static TRACE_BUFFER: OnceLock<Mutex<Vec<LlmTrace>>> = OnceLock::new();
static TRACE_CONFIG: OnceLock<Option<TraceConfig>> = OnceLock::new();

fn trace_buffer() -> &'static Mutex<Vec<LlmTrace>> {
    TRACE_BUFFER.get_or_init(|| Mutex::new(Vec::new()))
}

/// 初始化 trace recorder（bin 入口调用，在 init_thinking_log 之后）。
///
/// 若 trace.yaml enabled=false 或缺失，recorder 不初始化，record() 空操作。
/// 若 enabled=true，启动后台 flush task 定时写盘。
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

/// 调用方记录 trace（同步 Vec push，非阻塞——O(1)，无 I/O）。
///
/// fire-and-forget：失败只丢 trace，不 panic，不影响 agent tick。
pub fn record(trace: LlmTrace) {
    // 配置未加载或未启用 → 空操作
    if TRACE_CONFIG
        .get()
        .and_then(|c| c.as_ref())
        .map(|c| !c.output.enabled)
        .unwrap_or(true)
    {
        return;
    }

    if let Ok(mut buf) = trace_buffer().lock() {
        buf.push(trace);
    }
}

/// 后台 flush 循环：定时将缓冲区 trace 批量写盘。
async fn flush_loop(cfg: TraceConfig) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
    loop {
        interval.tick().await;
        let batch: Vec<LlmTrace> = {
            let mut buf = trace_buffer().lock().expect("poisoned");
            std::mem::take(&mut *buf)
        };
        if !batch.is_empty()
            && let Err(e) = write_batch(&batch, &cfg).await
        {
            tracing::error!("[trace] flush 失败: {}", e);
        }
    }
}

/// 批量写入 trace 到 JSONL（按 soul_stage + date 分区）。
async fn write_batch(traces: &[LlmTrace], cfg: &TraceConfig) -> Result<()> {
    use std::collections::HashMap;

    // 按 (soul_stage, date) 分组
    let mut groups: HashMap<(String, String), Vec<&LlmTrace>> = HashMap::new();
    for trace in traces {
        let soul = serde_json::to_string(&trace.soul_stage)?
            .trim_matches('"')
            .to_string();
        let date = trace.wall_clock.format("%Y-%m-%d").to_string();
        groups.entry((soul, date)).or_default().push(trace);
    }

    let base = crate::config::data_base_dir().join(&cfg.output.base_dir);

    for ((soul, date), group) in groups {
        let dir = base.join(format!("soul={}", soul));
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

    Ok(())
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trace_config_load_missing_fail_fast() {
        // T1 验收：trace.yaml 缺失必须 Err（非静默）
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
    fn test_trace_config_load_disabled() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("trace.yaml"),
            "version: \"0.0.1\"\noutput:\n  enabled: false\n  base_dir: \"traces\"\n",
        )
        .unwrap();
        let cfg = TraceConfig::load(tmp.path()).unwrap();
        assert!(!cfg.output.enabled);
        assert_eq!(cfg.output.base_dir, "traces");
    }

    #[test]
    fn test_trace_config_load_enabled() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("trace.yaml"),
            "version: \"0.0.1\"\noutput:\n  enabled: true\n  base_dir: \"traces\"\n",
        )
        .unwrap();
        let cfg = TraceConfig::load(tmp.path()).unwrap();
        assert!(cfg.output.enabled);
    }

    #[test]
    fn test_record_is_sync_no_async() {
        // T2 验收：record() 是同步函数（编译保证无 await）
        // 此测试能编译即证明 record 非 async
        let trace = LlmTrace {
            trace_id: Uuid::new_v4().to_string(),
            agent_id: Uuid::new_v4(),
            character_name: "测试".to_string(),
            tick_id: 1,
            soul_stage: SoulStage::Renhun,
            attempt: 0,
            provider: "test".to_string(),
            model: "test".to_string(),
            system_prompt: String::new(),
            user_prompt: "prompt".to_string(),
            response: "response".to_string(),
            prompt_tokens: None,
            completion_tokens: None,
            ok: true,
            wall_clock: chrono::Utc::now(),
        };
        // record 无返回值（同步 void），能调用即证明非 async
        record(trace);
    }

    #[test]
    fn test_llm_trace_serializes_token_as_null() {
        // T9 验收：token 为 None 时序列化为 null（非 0）
        let trace = LlmTrace {
            trace_id: "test".to_string(),
            agent_id: Uuid::nil(),
            character_name: "测试".to_string(),
            tick_id: 1,
            soul_stage: SoulStage::Renhun,
            attempt: 0,
            provider: "test".to_string(),
            model: "test".to_string(),
            system_prompt: String::new(),
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
            json.contains("\"completion_tokens\":null"),
            "completion None 应序列化为 null"
        );
        // agent_id 应是 UUID 格式
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
        let json = serde_json::to_string(&SoulStage::Earth).unwrap();
        assert_eq!(json, "\"earth\"");
    }
}
