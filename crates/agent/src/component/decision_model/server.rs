// ============================================================================
// 决策模型推理运行时：llama-server 子进程管理 + letter logits 读出
// ============================================================================
//
// 移植自 startlux_decision/gguf_server.py（Python 参考实现），差异仅在宿主：
//   - gguf_server 假设 llama-server 由外部拉起；本模块随 agent 启停（懒启动，
//     kill_on_drop 兜底回收，模型文件变更时自动重启）；
//   - 读出口径逐字对齐 _letter_logprobs：/completion 传 prompt + n_probs=256，
//     letter token 缺失时回退全词表（262144），取 last prompt position 的
//     letter logprob（全词表 log-softmax；除以温度后在选项子集上 softmax，
//     归一化抵消 log-sum-exp 常数项，等价于对原始 logits 操作）；
//   - letter token 映射（check_tokenizer 语义）用 /tokenize 运行时验证：
//     探针 prompt 后接每个字母必须恰好追加一个 token 且 id 与
//     decision_config.json 一致；不符即判运行时不可用（上层回退 LLM 路径）。
//   - 一次前向回答一个问题：/completion 请求经本模块互斥锁串行化。

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio::time::{Instant, timeout};
use tracing::{info, warn};

use super::prompt;
use super::prompt::{QType, QuestionSpec};

/// decision_config.json（随模型分发；letter ids 与校准温度的唯一事实源）
#[derive(Debug, Clone, Deserialize)]
pub struct DecisionModelParams {
    /// 26 个字母的 token id（与训练 tokenizer 逐一核对过）
    pub letter_token_ids: Vec<u32>,
    /// 题型 → 校准温度（渲染前先除 logits）
    #[serde(default)]
    pub temperature_by_type: std::collections::HashMap<String, f64>,
    #[serde(default)]
    pub max_options_per_pass: Option<usize>,
}

impl DecisionModelParams {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let params: DecisionModelParams =
            serde_json::from_slice(bytes).context("decision_config.json 解析失败")?;
        if params.letter_token_ids.len() != 26 {
            bail!(
                "letter_token_ids 必须为 26 个（实际 {}）",
                params.letter_token_ids.len()
            );
        }
        let distinct = params
            .letter_token_ids
            .iter()
            .collect::<std::collections::HashSet<_>>();
        if distinct.len() != 26 {
            bail!("letter_token_ids 存在重复");
        }
        Ok(params)
    }

    /// 题型校准温度（缺失回退 1.0，与参考实现 self.temperature.get(type, 1.0) 一致）
    pub fn temperature(&self, qtype: QType) -> f64 {
        self.temperature_by_type
            .get(qtype.config_key())
            .copied()
            .unwrap_or(1.0)
    }
}

/// 单问题决策答案
#[derive(Debug, Clone)]
pub struct SingleAnswer {
    /// 选中的选项 id（原样字符串，供回填 action_data）
    pub choice: String,
    /// 选项 id → 概率（按渲染顺序）
    pub probabilities: Vec<(String, f64)>,
    /// (p_max − 1/n) / (1 − 1/n)，[0,1]
    pub confidence: f64,
}

struct Running {
    child: tokio::process::Child,
    port: u16,
    gguf_path: PathBuf,
    base_url: String,
}

/// llama-server 运行时（懒启动；同一时刻至多一个子进程）
pub struct LlamaServer {
    /// 可执行文件解析候选（显式配置 → 安装目录 → 可执行文件同级 → PATH）
    binary_candidates: Vec<PathBuf>,
    extra_args: Vec<String>,
    preferred_port: u16,
    startup_timeout: Duration,
    call_timeout: Duration,
    http: reqwest::Client,
    running: Mutex<Option<Running>>,
}

impl LlamaServer {
    pub fn new(cfg: &crate::config::DecisionModelConfig, install_dir: &Path) -> Self {
        let exe_name = if cfg!(windows) {
            "llama-server.exe"
        } else {
            "llama-server"
        };
        let mut candidates = Vec::new();
        if let Some(p) = cfg.llama_server_path.as_deref()
            && !p.trim().is_empty()
        {
            candidates.push(PathBuf::from(p));
        }
        candidates.push(install_dir.join(exe_name));
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            candidates.push(dir.join(exe_name));
        }
        candidates.push(PathBuf::from(exe_name)); // PATH 查找
        Self {
            binary_candidates: candidates,
            extra_args: cfg.llama_server_args.clone(),
            preferred_port: cfg.port,
            startup_timeout: Duration::from_millis(cfg.startup_timeout_ms.max(1_000)),
            call_timeout: Duration::from_millis(cfg.timeout_ms.max(1_000)),
            http: reqwest::Client::builder()
                .timeout(Duration::from_millis(cfg.timeout_ms.max(1_000)))
                .build()
                .expect("构建 llama-server http client 失败"),
            running: Mutex::new(None),
        }
    }

    /// 解析 llama-server 可执行文件（存在即选中，按序尝试）：
    /// 显式配置 → 版本目录（分发仓下载的平台命名二进制，跨平台命名见
    /// `binary_file_names`）→ 安装目录 → agent 可执行文件同级 → PATH。
    fn resolve_binary(&self, hint_dir: Option<&Path>) -> Result<PathBuf> {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(dir) = hint_dir {
            candidates.extend(binary_file_names().iter().map(|n| dir.join(n)));
        }
        candidates.extend(self.binary_candidates.iter().cloned());
        for c in &candidates {
            if c.is_file() {
                return Ok(c.clone());
            }
            // PATH 中的裸名：交给 spawn 时系统解析，直接返回
            if (c.parent().is_none()
                || c.to_string_lossy() == *c.file_name().unwrap_or_default().to_string_lossy())
                && which_exists(c)
            {
                return Ok(c.clone());
            }
        }
        bail!(
            "找不到 llama-server 可执行文件（已尝试: {}）",
            candidates
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }

    /// 确保子进程已运行且加载的是指定 gguf（模型文件变更时重启）
    ///
    /// `binary_hint_dir`：版本目录提示（分发仓下载的 llama-server 与权重同目录）；
    /// 解析顺序见 `resolve_binary`。
    pub async fn ensure_running(
        &self,
        gguf_path: &Path,
        params: &DecisionModelParams,
        binary_hint_dir: Option<&Path>,
    ) -> Result<u16> {
        let mut guard = self.running.lock().await;
        if let Some(r) = guard.as_ref() {
            if r.gguf_path == gguf_path && health_ok(&self.http, r.port).await {
                return Ok(r.port);
            }
            info!("决策模型权重变更或服务失活，重启 llama-server");
            if let Some(mut old) = guard.take() {
                let _ = old.child.kill().await;
            }
        }
        let binary = self.resolve_binary(binary_hint_dir)?;
        let port = match self.preferred_port {
            0 => pick_free_port()?,
            p => p,
        };
        let base_url = format!("http://127.0.0.1:{port}");
        info!(
            "启动 llama-server: {} -m {} --port {} (参数: {:?})",
            binary.display(),
            gguf_path.display(),
            port,
            self.extra_args
        );
        let mut cmd = tokio::process::Command::new(&binary);
        // 共享库路径兜底：分发仓归档把 launcher 与库平铺在版本目录，
        // rpath（@loader_path/$ORIGIN）已覆盖；此处再注入环境变量，
        // 兼容 rpath 缺失的构建（Windows 按可执行文件目录自动搜索，无需注入）
        if let Some(dir) = binary_hint_dir {
            let dir_str = dir.to_string_lossy().to_string();
            if cfg!(target_os = "macos") {
                cmd.env("DYLD_LIBRARY_PATH", &dir_str);
            } else if cfg!(target_os = "linux") {
                cmd.env("LD_LIBRARY_PATH", &dir_str);
            }
        }
        cmd.arg("-m")
            .arg(gguf_path)
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .arg("-c")
            .arg("16384")
            .args(&self.extra_args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd
            .spawn()
            .with_context(|| format!("拉起 llama-server 失败: {}", binary.display()))?;
        // stderr 后台排空（防管道阻塞），debug 级留痕
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                use tokio::io::AsyncBufReadExt;
                let mut lines = tokio::io::BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(target: "llama_server", "{line}");
                }
            });
        }

        let started = Instant::now();
        loop {
            if health_ok(&self.http, port).await {
                break;
            }
            if started.elapsed() > self.startup_timeout {
                let _ = child.kill().await;
                bail!(
                    "llama-server 启动超时（{}s 内健康检查未通过）",
                    self.startup_timeout.as_secs()
                );
            }
            // 子进程提前退出：立即失败，避免空等
            if let Ok(Some(status)) = child.try_wait() {
                bail!("llama-server 提前退出: {status}");
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }

        verify_letter_tokens(&self.http, port, params)
            .await
            .context("letter token 映射校验失败（tokenizer/chat 模板与训练分布不一致）")?;

        *guard = Some(Running {
            child,
            port,
            gguf_path: gguf_path.to_path_buf(),
            base_url: base_url.clone(),
        });
        info!("llama-server 就绪: {base_url}");
        Ok(port)
    }

    /// 单问题决策：渲染 → 一次前向 → letter 读出 → 温度 softmax → confidence
    pub async fn decide_one(
        &self,
        state: &str,
        q: &QuestionSpec,
        params: &DecisionModelParams,
    ) -> Result<SingleAnswer> {
        if self.running.lock().await.is_none() {
            bail!("llama-server 未启动");
        }
        let (system, user) = prompt::messages(q, state)
            .with_context(|| format!("决策问题渲染失败: {}", q.instructions))?;
        let prompt_text = prompt::render_chat_text(&system, &user);
        let logprobs = self
            .letter_logprobs(&prompt_text, &params.letter_token_ids[..q.options.len()])
            .await?;
        if logprobs.len() != q.options.len() {
            bail!(
                "letter logprob 数量不符: {} / {}",
                logprobs.len(),
                q.options.len()
            );
        }
        let temperature = params.temperature(q.qtype);
        let probs = softmax_scaled(&logprobs, temperature as f32);
        let (best_idx, _) = probs
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap_or((0, &0.0));
        Ok(SingleAnswer {
            choice: q.options[best_idx].id.clone(),
            probabilities: q
                .options
                .iter()
                .zip(probs.iter())
                .map(|(o, p)| (o.id.clone(), *p))
                .collect(),
            confidence: choice_confidence(&probs),
        })
    }

    /// letter 读出（gguf_server._letter_logprobs 逐字对齐）
    ///
    /// n_probs=256 命中所有目标字母即返回；否则回退全词表重试一次。
    /// 目标序列 = decision_config.json 的 letter_token_ids（启动时已用 /tokenize
    /// 逐字母校验与运行时 tokenizer 一致）。
    async fn letter_logprobs(
        &self,
        prompt_text: &str,
        target_letter_ids: &[u32],
    ) -> Result<Vec<f32>> {
        let guard = self.running.lock().await;
        let running = guard
            .as_ref()
            .ok_or_else(|| anyhow!("llama-server 未启动"))?;
        for n_probs in [256usize, 262_144] {
            let body = serde_json::json!({
                "prompt": prompt_text,
                "n_predict": 1,
                "temperature": -1,
                "n_probs": n_probs,
                "cache_prompt": false,
            });
            let url = format!("{}/completion", running.base_url);
            let resp = timeout(self.call_timeout, self.http.post(&url).json(&body).send())
                .await
                .map_err(|_| anyhow!("llama-server /completion 超时"))?
                .with_context(|| "请求 llama-server /completion 失败")?;
            let status = resp.status();
            if !status.is_success() {
                bail!("llama-server /completion 返回 {status}");
            }
            let out: serde_json::Value = resp.json().await.context("解析 /completion 响应失败")?;
            let rows = out
                .get("completion_probabilities")
                .or_else(|| out.get("probs"))
                .and_then(|v| v.as_array())
                .ok_or_else(|| anyhow!("/completion 响应缺少 completion_probabilities"))?;
            let row = rows
                .first()
                .ok_or_else(|| anyhow!("completion_probabilities 为空"))?;
            let tokens = row
                .get("top_logprobs")
                .or_else(|| row.get("probs"))
                .and_then(|v| v.as_array())
                .ok_or_else(|| anyhow!("概率行缺少 top_logprobs/probs"))?;
            let mut got: std::collections::HashMap<u32, f64> =
                std::collections::HashMap::with_capacity(tokens.len());
            for t in tokens {
                if let (Some(id), Some(lp)) = (
                    t.get("id").and_then(|v| v.as_u64()),
                    t.get("logprob").and_then(|v| v.as_f64()),
                ) {
                    got.insert(id as u32, lp);
                }
            }
            let targets = target_letter_ids;
            if targets.iter().all(|t| got.contains_key(t)) {
                return Ok(targets.iter().map(|t| got[t] as f32).collect());
            }
            if n_probs == 262_144 {
                break;
            }
            warn!("letter tokens 不在 top-256 内，回退全词表重试");
        }
        bail!("letter tokens 缺失于 llama-server 的 log-probabilities")
    }

    /// 显式停止子进程（幂等）
    pub async fn shutdown(&self) {
        if let Some(mut r) = self.running.lock().await.take() {
            info!("停止 llama-server (port {})", r.port);
            let _ = r.child.kill().await;
        }
    }
}

/// 分发仓 llama-server 二进制文件名（按当前平台；版本目录内查找用）
fn binary_file_names() -> Vec<String> {
    let platform = super::current_platform();
    let exe = if cfg!(windows) { ".exe" } else { "" };
    vec![
        format!(
            "{}{platform}{exe}",
            super::manifest::LLAMA_SERVER_NAME_PREFIX
        ),
        format!("llama-server{exe}"),
    ]
}

/// 健康检查（llama-server /health）
async fn health_ok(http: &reqwest::Client, port: u16) -> bool {
    let url = format!("http://127.0.0.1:{port}/health");
    matches!(
        http.get(&url).send().await,
        Ok(resp) if resp.status().is_success()
    )
}

/// letter token 映射运行时验证（check_tokenizer 语义）：
/// 探针 prompt 以 THINK_OFF_SUFFIX 结尾，且每个字母恰为一个 token、id 与配置一致。
async fn verify_letter_tokens(
    http: &reqwest::Client,
    port: u16,
    params: &DecisionModelParams,
) -> Result<()> {
    let probe_q = QuestionSpec {
        qtype: QType::Noul,
        instructions: "q".to_string(),
        options: vec![
            prompt::OptionSpec::bare("true"),
            prompt::OptionSpec::bare("false"),
        ],
    };
    let (system, user) = prompt::messages(&probe_q, "s")?;
    let text = prompt::render_chat_text(&system, &user);
    if !text.ends_with(prompt::THINK_OFF_SUFFIX) {
        bail!("chat 模板渲染缺少 thinking-off assistant 前缀");
    }
    let base_url = format!("http://127.0.0.1:{port}");
    let tokenize = |content: String| {
        let http = http.clone();
        let url = format!("{base_url}/tokenize");
        async move {
            let body = serde_json::json!({ "content": content, "add_special": false });
            let resp = http.post(&url).json(&body).send().await?;
            let out: serde_json::Value = resp.json().await?;
            let toks = out
                .get("tokens")
                .and_then(|v| v.as_array())
                .ok_or_else(|| anyhow!("/tokenize 响应缺少 tokens"))?;
            Ok::<Vec<u32>, anyhow::Error>(
                toks.iter()
                    .filter_map(|t| t.as_u64().map(|v| v as u32))
                    .collect(),
            )
        }
    };
    let base_ids = tokenize(text.clone())
        .await
        .context("tokenize 探针 prompt 失败")?;
    for (k, letter) in prompt::LETTERS.iter().enumerate() {
        let expected = params.letter_token_ids[k];
        let extended = tokenize(format!("{text}{letter}")).await?;
        let tail_ok = extended.len() == base_ids.len() + 1
            && extended[..base_ids.len()] == base_ids[..]
            && extended[extended.len() - 1] == expected;
        if !tail_ok {
            bail!(
                "字母 {letter} token 映射不符（期望 id {expected}）——运行时 tokenizer 与训练分布不一致"
            );
        }
    }
    Ok(())
}

/// 温度缩放 softmax（在"列出的选项"上归一；log-softmax 的常数项在归一化中抵消，
/// 与对原始 logits 操作等价——见 gguf_server.py 模块 docstring）
pub fn softmax_scaled(logprobs: &[f32], temperature: f32) -> Vec<f64> {
    let t = if temperature.is_finite() && temperature > 0.0 {
        temperature
    } else {
        1.0
    };
    let scaled: Vec<f64> = logprobs.iter().map(|&l| (l as f64) / (t as f64)).collect();
    let max = scaled.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let sum: f64 = scaled.iter().map(|&s| (s - max).exp()).sum();
    scaled
        .iter()
        .map(|&s| {
            if sum > 0.0 {
                ((s - max).exp()) / sum
            } else {
                1.0 / scaled.len() as f64
            }
        })
        .collect()
}

/// TypeSafe choice 置信度：(p_max − 1/n) / (1 − 1/n)，截断到 [0,1]（model.py.choice_confidence）
pub fn choice_confidence(p: &[f64]) -> f64 {
    let n = p.len();
    if n < 2 {
        return 1.0;
    }
    let p_max = p.iter().cloned().fold(0.0_f64, f64::max);
    ((n as f64 * p_max - 1.0) / (n as f64 - 1.0)).clamp(0.0, 1.0)
}

fn pick_free_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").context("选择空闲端口失败")?;
    let port = listener.local_addr().context("读取本地端口失败")?.port();
    drop(listener);
    Ok(port)
}

fn which_exists(candidate: &Path) -> bool {
    let as_str = candidate.to_string_lossy();
    if as_str.contains('/') || as_str.contains('\\') {
        return candidate.exists();
    }
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let full = dir.join(candidate);
            if full.is_file() {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 二进制解析：版本目录（提示）优先于安装目录与 PATH
    #[test]
    fn resolve_binary_prefers_hint_dir() {
        let cfg = crate::config::DecisionModelConfig {
            llama_server_path: None,
            ..Default::default()
        };
        let install = tempfile::tempdir().expect("安装目录");
        let hint = tempfile::tempdir().expect("版本目录");
        let name = super::binary_file_names().remove(0);
        std::fs::write(hint.path().join(&name), b"stub").expect("写提示目录二进制");
        // 安装目录也放一份（应被提示目录压过）
        let install_name = if cfg!(windows) {
            "llama-server.exe"
        } else {
            "llama-server"
        };
        std::fs::write(install.path().join(install_name), b"stub2").expect("写安装目录二进制");

        let srv = LlamaServer::new(&cfg, install.path());
        let picked = srv
            .resolve_binary(Some(hint.path()))
            .expect("应命中提示目录");
        assert_eq!(picked, hint.path().join(&name));

        // 提示目录为空：回退安装目录
        let empty = tempfile::tempdir().expect("空目录");
        let picked2 = srv
            .resolve_binary(Some(empty.path()))
            .expect("应回退安装目录");
        assert_eq!(picked2, install.path().join(install_name));
    }

    /// softmax/温度/confidence 公式：已知 logits 的手算对照
    #[test]
    fn softmax_and_confidence_known_values() {
        // 均匀 logprob → 均匀概率，confidence = 0
        let p = softmax_scaled(&[0.0, 0.0, 0.0], 1.3742);
        assert!((p[0] - 1.0 / 3.0).abs() < 1e-9);
        assert!((choice_confidence(&p)).abs() < 1e-9);

        // 独热 → confidence = 1（log-softmax 常数项不参与）
        let p = softmax_scaled(&[0.0, -100.0, -100.0], 1.3742);
        assert!(p[0] > 0.999_999);
        assert!((choice_confidence(&p) - 1.0).abs() < 1e-6);

        // 温度升高 → 分布更平（两选项 logprob [ln4, 0]）
        let ln4 = 4.0_f32.ln();
        let cold = softmax_scaled(&[ln4, 0.0], 0.5);
        assert!(cold[0] > 0.9, "低温应更尖锐: {cold:?}");
        let hot = softmax_scaled(&[ln4, 0.0], 8.0);
        assert!(hot[0] < 0.65, "高温应更平缓: {hot:?}");
    }

    /// confidence 与 Python choice_confidence 对拍：n=12, p_max=0.8
    #[test]
    fn confidence_matches_python_formula() {
        // (12*0.8 - 1) / 11 = 0.781818...
        let mut p = vec![0.8]; // 其余均分 0.2/11 —— 公式只看 p_max 与 n
        p.resize(12, 0.2 / 11.0);
        let got = choice_confidence(&p);
        assert!((got - (12.0 * 0.8 - 1.0) / 11.0).abs() < 1e-12);
        // p_max < 1/n → clamp 到 0
        let flat = vec![0.05_f64; 12];
        assert_eq!(choice_confidence(&flat), 0.0);
        // 单选项 → 1.0
        assert_eq!(choice_confidence(&[1.0]), 1.0);
    }

    /// letter_token_ids 与 Qwen 系已知字母表一致（"A".."Z" = id 32..57）
    #[test]
    fn letter_ids_match_qwen_known_table() {
        let known: [u32; 26] = std::array::from_fn(|i| 32 + i as u32);
        let cfg = std::path::Path::new("../../tmp/models/ours-2b/decision_config.json");
        let Ok(bytes) = std::fs::read(cfg) else {
            return; // 仓外环境跳过
        };
        let params = DecisionModelParams::parse(&bytes).expect("决策配置合法");
        assert_eq!(params.letter_token_ids, known.to_vec());
        assert!((params.temperature(QType::Choice) - 1.3742).abs() < 1e-9);
        assert!((params.temperature(QType::Noul) - 1.3742).abs() < 1e-9);
    }

    #[test]
    fn decision_params_rejects_wrong_shape() {
        let bad = br#"{"letter_token_ids": [32], "temperature_by_type": {"choice": 1.0}}"#;
        assert!(DecisionModelParams::parse(bad).is_err());
        let dup = br#"{"letter_token_ids": [32,32,34,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,52,53,54,55,56,57], "temperature_by_type": {}}"#;
        assert!(DecisionModelParams::parse(dup).is_err(), "重复 id 拒绝");
    }
}
