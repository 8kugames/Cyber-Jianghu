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
    #[serde(default)]
    pub sanitize: TraceSanitizeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceOutputConfig {
    /// 默认开（用户要求：支持训练专用模型，默认为开）
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_base_dir")]
    pub base_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceUploadConfig {
    /// 默认开（开时回传 server，关时仅本地）
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceSanitizeConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_enabled")]
    pub persona_name_hash: bool,
    #[serde(default = "default_enabled")]
    pub persona_description_mask: bool,
    #[serde(default = "default_enabled")]
    pub dream_content_mask: bool,
    #[serde(default = "default_enabled")]
    pub dialogue_content_mask: bool,
}

fn default_enabled() -> bool {
    true
}
fn default_base_dir() -> String {
    "traces".to_string()
}
fn default_batch_size() -> usize {
    32
}

impl Default for TraceOutputConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            base_dir: default_base_dir(),
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

impl Default for TraceSanitizeConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            persona_name_hash: default_enabled(),
            persona_description_mask: default_enabled(),
            dream_content_mask: default_enabled(),
            dialogue_content_mask: default_enabled(),
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
/// 脱敏在 record 时对将要落盘/回传的 user_prompt 做（注入源保持原文，不影响 agent 推理）。
pub fn record(mut trace: LlmTrace) {
    // 配置未加载或未启用 → 空操作
    let cfg = match TRACE_CONFIG
        .get()
        .and_then(|c| c.as_ref())
    {
        Some(c) if c.output.enabled => c,
        _ => return,
    };

    // 脱敏：在 trace 写入前对 user_prompt 做模式替换（注入源原文不影响 agent 推理）
    if cfg.sanitize.enabled {
        trace.user_prompt = sanitize_user_prompt(&trace.user_prompt, &cfg.sanitize);
        trace.character_name = if cfg.sanitize.persona_name_hash {
            sanitize_persona_name(&trace.character_name)
        } else {
            trace.character_name
        };
    }

    if let Ok(mut buf) = trace_buffer().lock() {
        buf.push(trace);
    }
}

/// 对已拼接的 user_prompt 做脱敏（模式匹配 dream 段 + dialogue 段）。
///
/// dream 格式：`### 托梦\n{原文}\n`
/// dialogue 格式：`## 与{partner}的对话 (session: ...)\n` 段落内的 Partner 行
///
/// 注意：此函数只作用于 trace 的副本，不影响 agent 实际推理用的 prompt。
fn sanitize_user_prompt(prompt: &str, cfg: &TraceSanitizeConfig) -> String {
    let mut result = prompt.to_string();

    // dream 脱敏：匹配 "### 托梦\n" 到下一个 "\n### " 或 "\n## " 或段落结束
    if cfg.dream_content_mask {
        let mut sanitized = String::new();
        let mut in_dream = false;
        for line in result.lines() {
            if line == "### 托梦" {
                in_dream = true;
                sanitized.push_str(line);
                sanitized.push('\n');
                continue;
            }
            if in_dream {
                if line.starts_with("### ") || line.starts_with("## ") || line.is_empty() {
                    in_dream = false;
                    sanitized.push_str(line);
                    sanitized.push('\n');
                } else {
                    // dream 内容行 → 占位化
                    sanitized.push_str(&sanitize_dream(line));
                    sanitized.push('\n');
                }
            } else {
                sanitized.push_str(line);
                sanitized.push('\n');
            }
        }
        result = sanitized;
    }

    // dialogue 脱敏：partner_name 哈希化 + Partner 发言行占位化
    // 格式：`## 与{name}的对话` 标题 + `- {name}: {content}` Partner 行
    if cfg.dialogue_content_mask {
        let mut sanitized = String::new();
        let mut current_partner_name: Option<String> = None;
        let mut current_partner_hash: Option<String> = None;
        for line in result.lines() {
            // 匹配对话标题 "## 与{name}的对话 ..."
            if line.starts_with("## 与") && line.contains("的对话") {
                let after_prefix = &line["## 与".len()..];
                if let Some(end_idx) = after_prefix.find("的对话") {
                    let partner_name = after_prefix[..end_idx].to_string();
                    let hash = sanitize_dialogue_partner(&partner_name);
                    current_partner_name = Some(partner_name.clone());
                    current_partner_hash = Some(hash.clone());
                    let new_line = line.replacen(&partner_name, &hash, 1);
                    sanitized.push_str(&new_line);
                    sanitized.push('\n');
                    continue;
                }
            }
            // 匹配 Partner 发言行 "- {partner_original_name}: {content}"
            // 注意：Partner 行用的是原始 name（不是 hash），需用原始 name 匹配
            if let (Some(name), Some(_hash)) = (&current_partner_name, &current_partner_hash) {
                let prefix = format!("- {}: ", name);
                if line.starts_with(&prefix) {
                    let content = &line[prefix.len()..];
                    let hash = current_partner_hash.as_ref().unwrap();
                    sanitized.push_str(&format!("- {}: {}", hash, sanitize_dialogue_content(content)));
                    sanitized.push('\n');
                    continue;
                }
            }
            // 新的 ## 标题（非对话标题）重置 partner 上下文
            if line.starts_with("## ") && !line.contains("的对话") {
                current_partner_name = None;
                current_partner_hash = None;
            }
            sanitized.push_str(line);
            sanitized.push('\n');
        }
        result = sanitized;
    }

    result
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
                    system_prompt: t.system_prompt.clone(),
                    user_prompt: t.user_prompt.clone(),
                    response: t.response.clone(),
                    prompt_tokens: t.prompt_tokens,
                    completion_tokens: t.completion_tokens,
                    ok: t.ok,
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

    Ok(())
}

// ============================================================================
// 脱敏纯函数（在注入源调用，非事后清洗）
// ============================================================================

/// 计算短哈希（取 SHA256 前 8 位十六进制），用于不可逆标识。
///
/// 用 SHA256（非 DefaultHasher）保证跨 Rust 版本/platform 稳定，
/// 训练数据长期复现时哈希不漂移。
fn short_hash(input: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let result = hasher.finalize();
    format!("{:08x}", u32::from_be_bytes(result[..4].try_into().unwrap()))
}

/// 脱敏角色名：哈希化（不可逆）
///
/// 注意：persona 脱敏在 trace record 时针对 character_name 字段做。
/// persona 描述（base_description）当前不脱敏——因为 system_prompt 当前留空
/// （Direct 路径内嵌在 tick_msg），persona 描述不在 trace 中。未来若填充
/// system_prompt，需在此处补 system_prompt 脱敏。
pub fn sanitize_persona_name(name: &str) -> String {
    format!("角色_{}", &short_hash(name))
}

/// 脱敏托梦内容：占位化（保留哈希标识便于训练时关联）
pub fn sanitize_dream(content: &str) -> String {
    format!("[托梦内容已脱敏_{}]", short_hash(content))
}

/// 脱敏玩家私聊内容：占位化
pub fn sanitize_dialogue_content(_content: &str) -> String {
    "[对话内容已脱敏]".to_string()
}

/// 脱敏对话伙伴名：哈希化
pub fn sanitize_dialogue_partner(name: &str) -> String {
    format!("玩家_{}", short_hash(name))
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_user_prompt_dream_and_dialogue() {
        // 集成测试：真实 user_prompt 含 dream + dialogue 段落，脱敏后原文消失
        let cfg = TraceSanitizeConfig::default();
        let prompt = "## 附近的人\n- 张三 (ID: abc123)\n\n### 记忆上下文\n### 托梦\n去京城找李四报仇\n\n## 与王翠花的对话 (session: s1)\n- 你: 你好\n- 王翠花: 我有个秘密\n\n## 最近行动";
        let result = sanitize_user_prompt(prompt, &cfg);

        // dream 原文应被占位化
        assert!(!result.contains("去京城找李四报仇"), "dream 原文应脱敏");
        assert!(result.contains("[托梦内容已脱敏_"), "dream 应占位化");

        // dialogue partner_name 应哈希化
        assert!(!result.contains("王翠花"), "partner_name 应脱敏");
        assert!(result.contains("玩家_"), "partner 应哈希化");

        // dialogue Partner 发言应占位化
        assert!(!result.contains("我有个秘密"), "Partner 发言应脱敏");
        assert!(result.contains("[对话内容已脱敏]"), "Partner 发言应占位化");

        // Own 发言（agent 自己说的）不应脱敏
        assert!(result.contains("你好"), "Own 发言不应脱敏");

        // 非玩家输入段落应保留
        assert!(result.contains("张三"), "附近的人（NPC）不应脱敏");
        assert!(result.contains("abc123"), "NPC ID 不应脱敏");
    }

    #[test]
    fn test_sanitize_user_prompt_disabled_preserves_original() {
        // sanitize.mask 开关控制各入口独立脱敏
        let prompt = "### 托梦\n秘密内容";
        let cfg_on = TraceSanitizeConfig {
            dream_content_mask: true,
            ..Default::default()
        };
        let result = sanitize_user_prompt(prompt, &cfg_on);
        assert!(!result.contains("秘密内容"), "dream_mask=true 应脱敏");

        let cfg_off = TraceSanitizeConfig {
            dream_content_mask: false,
            ..Default::default()
        };
        let result = sanitize_user_prompt(prompt, &cfg_off);
        assert!(result.contains("秘密内容"), "dream_mask=false 应保留原文");
    }

    #[test]
    fn test_default_config_is_enabled() {
        // A1 验收：默认开（用户要求）
        assert!(TraceOutputConfig::default().enabled, "output 默认必须开");
        assert!(TraceUploadConfig::default().enabled, "upload 默认必须开");
        assert!(
            TraceSanitizeConfig::default().enabled,
            "sanitize 默认必须开"
        );
    }

    #[test]
    fn test_sanitize_persona_name_hashes() {
        // A2 验收：角色名哈希化，不泄露原名
        let result = sanitize_persona_name("张三丰");
        assert!(result.starts_with("角色_"), "应以 角色_ 开头");
        assert!(!result.contains("张三丰"), "不得包含原名");
    }

    #[test]
    fn test_sanitize_persona_name_deterministic() {
        // 相同输入应得相同哈希
        assert_eq!(sanitize_persona_name("李四"), sanitize_persona_name("李四"));
        // 不同输入得不同哈希
        assert_ne!(sanitize_persona_name("李四"), sanitize_persona_name("王五"));
    }

    #[test]
    fn test_sanitize_dream_masks_content() {
        // A3 验收：托梦占位化，不泄露原文
        let result = sanitize_dream("去京城找李四报仇");
        assert!(result.starts_with("[托梦内容已脱敏_"), "应占位化");
        assert!(!result.contains("京城"), "不得包含原文");
        assert!(!result.contains("李四"), "不得包含原文");
    }

    #[test]
    fn test_sanitize_dialogue_masks_content() {
        // A4 验收：私聊占位化
        let result = sanitize_dialogue_content("我有个秘密告诉你");
        assert_eq!(result, "[对话内容已脱敏]");
        assert!(!result.contains("秘密"));
    }

    #[test]
    fn test_sanitize_dialogue_partner_hashes() {
        let result = sanitize_dialogue_partner("王翠花");
        assert!(result.starts_with("玩家_"));
        assert!(!result.contains("王翠花"));
    }

    #[test]
    fn test_trace_config_load_with_full_config() {
        // 完整配置（含 upload + sanitize）可正确加载
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("trace.yaml"),
            "version: \"0.0.2\"\noutput:\n  enabled: true\n  base_dir: \"traces\"\nupload:\n  enabled: false\nsanitize:\n  enabled: true\n  persona_name_hash: false\n",
        )
        .unwrap();
        let cfg = TraceConfig::load(tmp.path()).unwrap();
        assert!(cfg.output.enabled);
        assert!(!cfg.upload.enabled, "upload.enabled=false 应被读取");
        assert!(cfg.sanitize.enabled);
        assert!(
            !cfg.sanitize.persona_name_hash,
            "persona_name_hash=false 应被读取"
        );
    }

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
    fn test_record_is_sync_no_async() {
        // T2 验收：record() 是同步函数（编译保证无 await）
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
