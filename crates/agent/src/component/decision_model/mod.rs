// ============================================================================
// 决策模型（玩家侧 2B 意图决策）：下载管理 + llama.cpp 读出运行时 + 计量
// ============================================================================
//
// 职责边界（与两段式架构对齐）：
//   - 本模块把"认知摘要 + 世界状态"转成结构化意图的动作选择与实体绑定
//     （单问题 letter 读出），不负责认知生成（人魂 LLM 负责）、不负责审查
//     （天魂四层原样兜底）、不走 LLM 场景路由（非 LLM API 调用，metrics
//     独立标记来源）。
//   - 失败哲学对齐 chaos 兜底：任何环节失败（未安装/下载失败/启动失败/
//     超时/校验失败）只记日志与计数，调用方整体回退既有 LLM 决策路径，
//     绝不阻塞 agent 主循环。
//
// 计量（决策次数/来源占比/置信度分布/每步耗时）经 snapshot_metrics 暴露，
// 由 /api/v1/metrics 的 decision_model 小节透出。

mod downloader;
mod manifest;
mod prompt;
mod questions;
mod server;
mod sys_memory;

pub use downloader::{DownloadProgress, DownloadSource};
pub use prompt::{MAX_OPTIONS, OptionSpec, QType, QuestionSpec};
pub use questions::{
    ACT2_ADOPTABLE_ACTIONS, AGENT_ACTS, ActionBinding, BINDABLE_ACTIONS, COGNITION_TITLE,
    EntityCandidates, ItemProvenance, NONE_OPTION, act1_gate_pass, act2_gate_pass, action_criteria,
    bind_act1, build_act1_question, build_act2_question, build_agent1_question, build_candidates,
    build_item1_question, build_loc1_question, build_state_text, cognition_block, norm_id,
    validate_binding_against_actions,
};
pub use server::{DecisionModelParams, SingleAnswer, choice_confidence, softmax_scaled};

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use tokio::sync::{Mutex, RwLock, broadcast};
use tracing::{info, warn};

use crate::config::DecisionModelConfig;

/// llama-server 运行时故障后的冷却期（期间决策路径直接回退 LLM）
const SERVER_FAILURE_COOLDOWN: Duration = Duration::from_secs(60);

use downloader::{Downloader, candidate_urls};
use manifest::{ModelManifest, verify_file_sha256};
use server::LlamaServer;

/// 决策模型生命周期状态（status 端点 / SSE 序列化视图）
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LifecycleStatus {
    /// 已安装就绪
    Ready {
        version: String,
        quant: String,
        gguf: String,
    },
    /// 下载中
    Downloading {
        file: String,
        downloaded_bytes: u64,
        total_bytes: u64,
    },
    /// 未安装（等待后台下载）
    NotInstalled,
    /// 失败（上次下载/校验错误；决策路径自动回退 LLM）
    Failed { error: String },
}

pub struct InstallInfo {
    version: String,
    quant: String,
    gguf_path: PathBuf,
    params: DecisionModelParams,
}

impl Clone for InstallInfo {
    fn clone(&self) -> Self {
        Self {
            version: self.version.clone(),
            quant: self.quant.clone(),
            gguf_path: self.gguf_path.clone(),
            params: self.params.clone(),
        }
    }
}

enum ManagerState {
    NotInstalled,
    Ready(InstallInfo),
    Failed(String),
}

/// 决策模型管理器（Arc 共享；每 agent 进程一个实例）
pub struct DecisionModelManager {
    cfg: DecisionModelConfig,
    deploy: Deploy,
    install_dir: PathBuf,
    source: DownloadSource,
    resolved_quant: String,
    state: RwLock<ManagerState>,
    server: LlamaServer,
    downloader: Downloader,
    /// 下载安装互斥（防止并发触发双份下载）
    install_lock: Mutex<()>,
    progress_tx: broadcast::Sender<DownloadProgress>,
    /// 最近一次下载进度（status 端点快照用；std Mutex 因进度回调在同步闭包内）
    last_progress: std::sync::Mutex<Option<DownloadProgress>>,
    /// llama-server 最近一次运行时故障时刻（冷却期判据；成功后清空）
    server_failure_since: std::sync::Mutex<Option<Instant>>,
}

impl DecisionModelManager {
    pub fn new(cfg: DecisionModelConfig, progress_tx: broadcast::Sender<DownloadProgress>) -> Self {
        let install_dir = cfg
            .install_dir
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::config::data_base_dir().join("decision-model"));
        let resolved_quant = resolve_quant(&cfg);
        let deploy = resolve_deploy(&cfg).unwrap_or_else(|e| {
            warn!("决策模型部署模式不可用（{}），不装配（走既有 LLM 路径）", e);
            Deploy::Local
        });
        let server = LlamaServer::new(&cfg, &install_dir).with_deploy(deploy.clone());
        Self {
            deploy,
            server,
            source: DownloadSource {
                modelscope_repo: cfg.modelscope_repo.clone(),
                github_release_base: cfg.github_release_url.clone(),
            },
            downloader: Downloader::new(),
            install_dir,
            cfg,
            resolved_quant,
            state: RwLock::new(ManagerState::NotInstalled),
            install_lock: Mutex::new(()),
            progress_tx,
            last_progress: std::sync::Mutex::new(None),
            server_failure_since: std::sync::Mutex::new(None),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.cfg.enabled
    }

    pub fn config(&self) -> &DecisionModelConfig {
        &self.cfg
    }

    /// 生效部署形态（面板/状态端点展示用）
    pub fn deploy(&self) -> &Deploy {
        &self.deploy
    }

    /// 置信度门控阈值
    pub fn threshold(&self) -> f32 {
        self.cfg.threshold
    }

    pub fn subscribe_progress(&self) -> broadcast::Receiver<DownloadProgress> {
        self.progress_tx.subscribe()
    }

    /// 当前生命周期快照
    pub async fn status(&self) -> LifecycleStatus {
        let guard = self.state.read().await;
        match &*guard {
            ManagerState::NotInstalled => {
                // 下载中：进度由 last_progress 提供（SSE 同源）
                if let Ok(g) = self.last_progress.lock()
                    && let Some(p) = g.as_ref()
                {
                    return LifecycleStatus::Downloading {
                        file: p.file.clone(),
                        downloaded_bytes: p.downloaded_bytes,
                        total_bytes: p.total_bytes,
                    };
                }
                LifecycleStatus::NotInstalled
            }
            ManagerState::Ready(i) => LifecycleStatus::Ready {
                version: i.version.clone(),
                quant: i.quant.clone(),
                gguf: i.gguf_path.display().to_string(),
            },
            ManagerState::Failed(e) => LifecycleStatus::Failed { error: e.clone() },
        }
    }

    /// 当前已就绪的安装信息（未就绪返回 Err——热路径不触发下载）
    pub async fn ready_install(&self) -> Result<InstallInfo> {
        let info = match &*self.state.read().await {
            ManagerState::Ready(info) => Some(info.clone()),
            _ => None,
        };
        match info {
            Some(info) => Ok(info),
            None => bail!(
                "决策模型未就绪（状态: {:?}），回退 LLM 决策路径",
                self.status().await
            ),
        }
    }

    /// 确保模型已安装：优先复用本地版本目录（manifest + 权重存在即认就绪），
    /// 缺失/损坏则走双源下载。由启动后台任务与手动触发调用。
    pub async fn ensure_installed(&self) -> Result<InstallInfo> {
        let existing = match &*self.state.read().await {
            ManagerState::Ready(info) => Some(info.clone()),
            _ => None,
        };
        if let Some(info) = existing {
            return Ok(info);
        }
        // remote 模式无本地资产可装：健康探活由 llama-server 模块首问执行，
        // 此处直接置就绪（兼容性校验失败会在决策路径回退并冷却）
        if let Deploy::Remote { ref base_url } = self.deploy {
            let info = InstallInfo {
                version: "remote".to_string(),
                quant: "endpoint".to_string(),
                gguf_path: std::path::PathBuf::from(base_url),
                params: project_default_params(),
            };
            *self.state.write().await = ManagerState::Ready(info.clone());
            return Ok(info);
        }
        let _guard = self.install_lock.lock().await;
        // 双检：等锁期间可能已被并发安装
        let existing = match &*self.state.read().await {
            ManagerState::Ready(info) => Some(info.clone()),
            _ => None,
        };
        if let Some(info) = existing {
            return Ok(info);
        }

        if self.source.is_empty() {
            let err = "下载源未配置（modelscope_repo 与 github_release_url 均为空）".to_string();
            *self.state.write().await = ManagerState::Failed(err.clone());
            bail!(err);
        }

        match self.try_install().await {
            Ok(info) => {
                info!(
                    "决策模型就绪: version={} quant={} gguf={}",
                    info.version,
                    info.quant,
                    info.gguf_path.display()
                );
                *self.state.write().await = ManagerState::Ready(info.clone());
                Ok(info)
            }
            Err(e) => {
                warn!("决策模型安装失败（决策路径将回退 LLM）: {:#}", e);
                *self.state.write().await = ManagerState::Failed(format!("{e:#}"));
                Err(e)
            }
        }
    }

    async fn try_install(&self) -> Result<InstallInfo> {
        tokio::fs::create_dir_all(&self.install_dir)
            .await
            .with_context(|| format!("创建安装目录失败: {}", self.install_dir.display()))?;
        let urls = candidate_urls(&self.source, "manifest.json");
        let manifest_path = self.install_dir.join("manifest.json");
        self.downloader
            .download_file(&urls, &manifest_path, &manifest_entry(), |_| {})
            .await
            .context("下载 manifest.json 失败")?;
        let manifest_bytes = tokio::fs::read(&manifest_path).await?;
        let manifest = ModelManifest::parse(&manifest_bytes)?;

        let quant = self.resolved_quant.clone();
        let platform = current_platform();
        let Some(gguf_entry) = manifest.gguf_for_quant(&quant) else {
            bail!(
                "清单中无 {} 档位权重（可用: {}）",
                quant,
                manifest
                    .files
                    .iter()
                    .filter(|f| f.quant.is_some())
                    .map(|f| f.quant.clone().unwrap_or_default())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        };
        // 清单缺本平台运行时：告警降级而非拒绝安装——manifest 旧版本 / 未覆盖
        // 的平台下，llama_server_path 或 PATH 中的 llama-server 仍可用
        // （启动时解析不到才回退 LLM 路径）
        if manifest.llama_server_for_platform(&platform).is_none() {
            warn!(
                "清单中无本平台（{platform}）llama-server 运行时归档，跳过自动装配；\
                 将使用 llama_server_path / 安装目录 / 可执行文件同级 / PATH 中的 llama-server"
            );
        }
        let version_dir = self.install_dir.join(&manifest.version);
        tokio::fs::create_dir_all(&version_dir).await?;

        // 逐文件下载（本地已存在且 sha256 一致的文件自动跳过）
        for entry in manifest.required_files(&quant, &platform) {
            let dest = version_dir.join(&entry.name);
            let urls = candidate_urls(&self.source, &entry.name);
            let mut last_broadcast = Instant::now();
            let tx = self.progress_tx.clone();
            let last_progress = &self.last_progress;
            self.downloader
                .download_file(&urls, &dest, entry, |p| {
                    // 1MB 步进已由下载器节流；SSE 频率再压到 250ms
                    let now = Instant::now();
                    if p.done || last_broadcast.elapsed() >= Duration::from_millis(250) {
                        last_broadcast = now;
                        let _ = tx.send(p.clone());
                        if let Ok(mut g) = last_progress.lock() {
                            *g = Some(p.clone());
                        }
                    }
                })
                .await
                .with_context(|| format!("下载 {} 失败", entry.name))?;
            // llama-server 是 tar.gz 运行时归档（launcher + 共享库）：解压到版本目录
            // 并给 launcher 补可执行位（tar 保留权限但下载链路外再兜一次；Windows 忽略）
            if entry.kind == Some(manifest::AssetKind::LlamaServer) {
                let archive_path = version_dir.join(&entry.name);
                extract_runtime_archive(&archive_path, &version_dir)
                    .with_context(|| format!("解压 llama-server 运行时归档失败: {}", entry.name))?;
                #[cfg(unix)]
                {
                    let launcher = version_dir.join(llama_server_launcher_name());
                    tokio::fs::set_permissions(
                        &launcher,
                        std::os::unix::fs::PermissionsExt::from_mode(0o755),
                    )
                    .await
                    .with_context(|| format!("设置可执行位失败: {}", launcher.display()))?;
                }
            }
        }

        // 终验：逐文件 sha256（下载器边下边算已验过；此处兜底本地旧文件场景）
        for entry in manifest.required_files(&quant, &platform) {
            let dest = version_dir.join(&entry.name);
            verify_file_sha256(&dest, &entry.sha256)
                .with_context(|| format!("本地文件校验失败: {}", entry.name))?;
        }

        let params_bytes = tokio::fs::read(version_dir.join("decision_config.json"))
            .await
            .context("读取 decision_config.json 失败")?;
        let params = DecisionModelParams::parse(&params_bytes)?;

        // 原子切换版本指针（旧版本目录保留供回滚）
        let pointer = serde_json::json!({ "version": manifest.version });
        let pointer_path = self.install_dir.join("current.json");
        let tmp = self.install_dir.join("current.json.tmp");
        tokio::fs::write(&tmp, serde_json::to_vec_pretty(&pointer)?).await?;
        tokio::fs::rename(&tmp, &pointer_path).await?;

        let gguf_path = version_dir.join(&gguf_entry.name);
        Ok(InstallInfo {
            version: manifest.version.clone(),
            quant,
            gguf_path,
            params,
        })
    }

    /// 从本地版本目录恢复（启动时优先走，避免重复下载）：
    /// current.json 指向的目录内 manifest + 权重 + 决策配置齐全即就绪
    /// （完整性校验用文件大小比对，全量 sha256 只在下载后做）。
    async fn try_restore_local(&self) -> Option<InstallInfo> {
        let pointer_path = self.install_dir.join("current.json");
        let pointer_bytes = tokio::fs::read(&pointer_path).await.ok()?;
        let version = serde_json::from_slice::<serde_json::Value>(&pointer_bytes)
            .ok()?
            .get("version")?
            .as_str()?
            .to_string();
        if version.contains("..") || version.contains('/') {
            return None;
        }
        let version_dir = self.install_dir.join(&version);
        let manifest = ModelManifest::load_from_dir(&version_dir).ok()?;
        let quant = self.resolved_quant.clone();
        let gguf_entry = manifest.gguf_for_quant(&quant)?;
        for entry in manifest.required_files(&quant, &current_platform()) {
            let dest = version_dir.join(&entry.name);
            let meta = tokio::fs::metadata(&dest).await.ok()?;
            if meta.len() != entry.size {
                warn!("本地决策模型文件大小不符: {}（等待重新下载）", entry.name);
                return None;
            }
            // llama-server：归档在位还不够，解压出的 launcher 也必须在
            if entry.kind == Some(manifest::AssetKind::LlamaServer)
                && !tokio::fs::metadata(version_dir.join(llama_server_launcher_name()))
                    .await
                    .map(|m| m.is_file())
                    .unwrap_or(false)
            {
                warn!("llama-server 归档在位但 launcher 缺失（等待重新解压）");
                return None;
            }
        }
        let params_bytes = tokio::fs::read(version_dir.join("decision_config.json"))
            .await
            .ok()?;
        let params = DecisionModelParams::parse(&params_bytes).ok()?;
        Some(InstallInfo {
            version: manifest.version.clone(),
            quant,
            gguf_path: version_dir.join(&gguf_entry.name),
            params,
        })
    }

    /// 启动后台任务入口：先尝试本地恢复，失败才下载
    pub async fn install_if_needed(&self) {
        match self.deploy {
            Deploy::Remote { ref base_url } => {
                // remote：无本地资产；llama-server 模块在首问时做健康检查与
                // letter 校验，此处即席探活一次让状态尽快可视
                info!("决策模型 remote 模式: 端点 {base_url}（首问时校验兼容性）");
                *self.state.write().await = ManagerState::Ready(InstallInfo {
                    version: "remote".to_string(),
                    quant: "endpoint".to_string(),
                    gguf_path: std::path::PathBuf::from(base_url),
                    params: project_default_params(),
                });
            }
            Deploy::Local => {
                if let Some(info) = self.try_restore_local().await {
                    info!(
                        "决策模型本地恢复: version={} quant={}",
                        info.version, info.quant
                    );
                    *self.state.write().await = ManagerState::Ready(info);
                    return;
                }
                let _ = self.ensure_installed().await;
            }
        }
    }

    /// 单问题决策（一次前向回答一个问题；互斥由 llama-server 模块内锁保证）
    ///
    /// 运行时故障（二进制缺失/启动超时/加载崩溃）后进入 SERVER_FAILURE_COOLDOWN
    /// 冷却期：期间直接回退 LLM 路径，避免每次决策都重试拉起把 tick 卡在
    /// startup_timeout 上（启动失败的本质修复在部署侧，冷却只保护主循环）。
    pub async fn decide_one(&self, state_text: &str, q: &QuestionSpec) -> Result<SingleAnswer> {
        let info = self.ready_install().await?;
        if let Ok(g) = self.server_failure_since.lock()
            && let Some(failed_at) = *g
            && failed_at.elapsed() < SERVER_FAILURE_COOLDOWN
        {
            anyhow::bail!(
                "llama-server 冷却期内（上次失败 {:.0}s 前），回退 LLM 路径",
                failed_at.elapsed().as_secs_f32()
            );
        }
        let started = Instant::now();
        let version_dir = info.gguf_path.parent().map(std::path::Path::to_path_buf);
        let result = async {
            self.server
                .ensure_running(&info.gguf_path, &info.params, version_dir.as_deref())
                .await
                .context("llama-server 启动失败")?;
            self.server.decide_one(state_text, q, &info.params).await
        }
        .await;
        let elapsed_ms = started.elapsed().as_millis() as u64;
        match &result {
            Ok(_) => {
                if let Ok(mut g) = self.server_failure_since.lock() {
                    *g = None;
                }
                metrics::record_call(true, elapsed_ms);
            }
            Err(e) => {
                if let Ok(mut g) = self.server_failure_since.lock() {
                    *g = Some(Instant::now());
                }
                metrics::record_call(false, elapsed_ms);
                warn!("决策模型调用失败（回退 LLM 路径）: {:#}", e);
            }
        }
        result
    }

    /// 停止推理子进程（进程退出由 kill_on_drop 兜底；显式调用用于优雅停机）
    pub async fn shutdown(&self) {
        self.server.shutdown().await;
    }
}

// ============================================================================

/// 当前运行平台标识（与分发仓 llama-server 文件名平台段一致，
/// 如 "macos-arm64"/"linux-x86_64"/"windows-x86_64"）
pub(crate) fn current_platform() -> String {
    let os = match std::env::consts::OS {
        "macos" | "linux" | "windows" => std::env::consts::OS,
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    };
    format!("{os}-{arch}")
}

/// 当前平台的 llama-server launcher 文件名（解压产物）
pub(crate) fn llama_server_launcher_name() -> &'static str {
    if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

/// 解压 llama-server 运行时归档（tar.gz）到版本目录。
/// 归档由本仓打包发布（平铺 launcher + 共享库）；仍按防御式解压：
/// 拒绝绝对路径与 `..` 分量，防路径穿越。
fn extract_runtime_archive(archive: &Path, dest_dir: &Path) -> Result<()> {
    let file = std::fs::File::open(archive)
        .with_context(|| format!("打开运行时归档失败: {}", archive.display()))?;
    let gz = flate2::read::GzDecoder::new(std::io::BufReader::with_capacity(1024 * 1024, file));
    let mut ar = tar::Archive::new(gz);
    for entry in ar
        .entries()
        .with_context(|| format!("读取运行时归档目录失败: {}", archive.display()))?
    {
        let mut entry = entry.with_context(|| "读取归档条目失败")?;
        // 先克隆出路径字符串（entry.path() 借用 entry，unpack 需可变借用）
        let name = entry
            .path()
            .with_context(|| "读取归档条目路径失败")?
            .to_string_lossy()
            .to_string();
        if name.starts_with('/') || name.split('/').any(|seg| seg == "..") {
            bail!("运行时归档含非法路径条目: {name}");
        }
        entry
            .unpack_in(dest_dir)
            .with_context(|| format!("解压归档条目失败: {name}"))?;
    }
    Ok(())
}

fn manifest_entry() -> manifest::ManifestFile {
    manifest::ManifestFile {
        name: "manifest.json".to_string(),
        size: 0, // 清单大小未知，跳过尺寸校验
        sha256: String::new(),
        kind: Some(manifest::AssetKind::DecisionConfig),
        quant: None,
        platform: None,
    }
}

/// 量化档位解析：默认取配置；低配（可用内存低于阈值）自动降 q4_k_s
/// 配置可用性校验（启动装配闸与面板保存共用）：
/// local 需下载源；remote 需可解析 URL（容器内可用默认发现地址）；
/// 容器内显式 local 非法。
pub fn validate_config(cfg: &DecisionModelConfig) -> Result<(), String> {
    match resolve_deploy(cfg)? {
        Deploy::Local => {
            let sources =
                !cfg.modelscope_repo.trim().is_empty() || !cfg.github_release_url.trim().is_empty();
            if sources {
                Ok(())
            } else {
                Err("local 模式需要配置下载源（modelscope_repo / github_release_url）".to_string())
            }
        }
        Deploy::Remote { .. } => Ok(()),
    }
}

/// 是否运行在容器内（docker 部署仅允许 remote 模式）
pub fn is_in_container() -> bool {
    std::path::Path::new("/.dockerenv").exists() || std::path::Path::new("/.containerenv").exists()
}

/// 解析后的部署形态
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Deploy {
    /// 下载模型并自启动 llama-server
    Local,
    /// 访问外部端点（base_url 已含协议与端口）
    Remote { base_url: String },
}

/// 部署模式解析：显式配置优先；缺省按环境（容器内 → remote，本地 → local）。
/// docker 内显式 local 拒绝（每容器重复下载自部署是被禁止的形态）；
/// remote 缺 URL 时容器内用 docker 网络发现默认地址，其余环境报错。
fn resolve_deploy(cfg: &DecisionModelConfig) -> Result<Deploy, String> {
    let in_docker = is_in_container();
    let want_remote = match cfg.mode {
        Some(crate::config::DecisionModelMode::Remote) => true,
        Some(crate::config::DecisionModelMode::Local) => false,
        None => in_docker,
    };
    if want_remote {
        let url = cfg
            .remote_url
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty());
        let base_url = match url {
            Some(u) => u.trim_end_matches('/').to_string(),
            None if in_docker => {
                warn!(
                    "remote 模式未配置 remote_url，使用 docker 网络发现默认地址 {}（compose 服务名约定）",
                    crate::config::DECISION_MODEL_DOCKER_DEFAULT_URL
                );
                crate::config::DECISION_MODEL_DOCKER_DEFAULT_URL.to_string()
            }
            None => return Err("remote 模式需要配置 remote_url".to_string()),
        };
        return Ok(Deploy::Remote { base_url });
    }
    if in_docker {
        return Err(
            "docker 部署仅支持 remote 模式（mode=remote + remote_url）：容器内禁止重复下载与自部署"
                .to_string(),
        );
    }
    Ok(Deploy::Local)
}

/// remote 模式的项目契约参数（端点须服务项目专用模型；letter ids 与校准温度
/// 是模型绑定常量，与发布仓 decision_config.json 一致）
fn project_default_params() -> DecisionModelParams {
    DecisionModelParams::parse(
        br#"{"format":"StartLux-Decision-v1","letter_token_ids":[32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,52,53,54,55,56,57],"temperature_by_type":{"choice":1.3742,"noul":1.3742,"score":1.3742},"max_options_per_pass":26,"wide_choice":{"group":25,"keep":3,"residual":0.001}}"#,
    )
    .expect("项目契约参数必须可解析")
}

fn resolve_quant(cfg: &DecisionModelConfig) -> String {
    let configured = cfg.quant.trim().to_ascii_lowercase();
    let configured = if crate::config::DECISION_MODEL_QUANTS.contains(&configured.as_str()) {
        configured
    } else {
        warn!(
            "decision_model.quant={:?} 不在支持列表，回退 q5_k_m",
            cfg.quant
        );
        "q5_k_m".to_string()
    };
    if cfg.low_memory_threshold_mb > 0
        && let Some(avail_mb) = sys_memory::available_memory_mb()
        && avail_mb < cfg.low_memory_threshold_mb
        && configured != "q4_k_s"
    {
        info!(
            "系统可用内存 {avail_mb}MB 低于阈值 {}MB，决策模型降档 q5_k_m -> q4_k_s",
            cfg.low_memory_threshold_mb
        );
        return "q4_k_s".to_string();
    }
    configured
}

// ============================================================================
// 计量（scenario.rs 模式：模块级原子计数 + snapshot 只读聚合）
// ============================================================================

static DECISION_TICKS: AtomicU64 = AtomicU64::new(0);
static DECISION_TAKEN: AtomicU64 = AtomicU64::new(0);
static FALLBACK_LOW_CONF: AtomicU64 = AtomicU64::new(0);
static FALLBACK_INELIGIBLE: AtomicU64 = AtomicU64::new(0);
static FALLBACK_ERROR: AtomicU64 = AtomicU64::new(0);
static CALLS: AtomicU64 = AtomicU64::new(0);
static CALL_FAILURES: AtomicU64 = AtomicU64::new(0);
static CALL_MS_TOTAL: AtomicU64 = AtomicU64::new(0);
static COGNITION_MS_TOTAL: AtomicU64 = AtomicU64::new(0);
static COGNITION_MS_N: AtomicU64 = AtomicU64::new(0);
/// act1 置信度直方图（10 桶：[0,0.1) ... [0.9,1.0]）
static CONF_BUCKETS: [AtomicU64; 10] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const ZERO: AtomicU64 = AtomicU64::new(0);
    [ZERO; 10]
};

/// 决策模型路径指标写入辅助（runtime/decision.rs 调用）
pub mod metrics {
    use super::*;

    pub fn record_tick_attempt() {
        DECISION_TICKS.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_taken() {
        DECISION_TAKEN.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_fallback_low_conf() {
        FALLBACK_LOW_CONF.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_fallback_ineligible() {
        FALLBACK_INELIGIBLE.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_fallback_error() {
        FALLBACK_ERROR.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_call(ok: bool, elapsed_ms: u64) {
        CALLS.fetch_add(1, Ordering::Relaxed);
        if !ok {
            CALL_FAILURES.fetch_add(1, Ordering::Relaxed);
        }
        CALL_MS_TOTAL.fetch_add(elapsed_ms, Ordering::Relaxed);
    }

    pub fn record_cognition(elapsed_ms: u64) {
        COGNITION_MS_TOTAL.fetch_add(elapsed_ms, Ordering::Relaxed);
        COGNITION_MS_N.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_act1_confidence(confidence: f64) {
        let bucket = (confidence.clamp(0.0, 1.0) * 10.0) as usize;
        let bucket = bucket.min(9);
        CONF_BUCKETS[bucket].fetch_add(1, Ordering::Relaxed);
    }

    /// /api/v1/metrics 的 decision_model 小节
    pub fn snapshot() -> serde_json::Value {
        let ticks = DECISION_TICKS.load(Ordering::Relaxed);
        let taken = DECISION_TAKEN.load(Ordering::Relaxed);
        let calls = CALLS.load(Ordering::Relaxed);
        let call_ms = CALL_MS_TOTAL.load(Ordering::Relaxed);
        let cog_n = COGNITION_MS_N.load(Ordering::Relaxed);
        let cog_ms = COGNITION_MS_TOTAL.load(Ordering::Relaxed);
        serde_json::json!({
            "decision_ticks": ticks,
            "decision_taken": taken,
            "fallback_low_confidence": FALLBACK_LOW_CONF.load(Ordering::Relaxed),
            "fallback_ineligible": FALLBACK_INELIGIBLE.load(Ordering::Relaxed),
            "fallback_error": FALLBACK_ERROR.load(Ordering::Relaxed),
            "take_rate": pct(taken, ticks),
            "calls": calls,
            "call_failures": CALL_FAILURES.load(Ordering::Relaxed),
            "avg_call_ms": call_ms.checked_div(calls).unwrap_or(0),
            "cognition_calls": cog_n,
            "avg_cognition_ms": cog_ms.checked_div(cog_n).unwrap_or(0),
            "act1_confidence_buckets": CONF_BUCKETS
                .iter()
                .map(|b| b.load(Ordering::Relaxed))
                .collect::<Vec<_>>(),
        })
    }

    fn pct(part: u64, total: u64) -> f64 {
        if total == 0 {
            0.0
        } else {
            (part as f64 / total as f64 * 100.0 * 10.0).round() / 10.0
        }
    }
}

#[cfg(test)]
mod tests {

    fn cfg_with(
        mode: Option<crate::config::DecisionModelMode>,
        url: Option<&str>,
    ) -> DecisionModelConfig {
        DecisionModelConfig {
            mode,
            remote_url: url.map(|u| u.to_string()),
            ..DecisionModelConfig::default()
        }
    }

    #[test]
    fn resolve_deploy_env_defaults_and_rules() {
        use crate::config::DecisionModelMode as M;
        // 本地环境：缺省 local；显式 remote 需 URL
        assert!(matches!(
            super::resolve_deploy(&cfg_with(None, None)),
            Ok(super::Deploy::Local)
        ));
        assert!(super::resolve_deploy(&cfg_with(Some(M::Remote), None)).is_err());
        assert!(matches!(
            super::resolve_deploy(&cfg_with(Some(M::Remote), Some("http://h:8081/"))),
            Ok(super::Deploy::Remote { ref base_url }) if base_url == "http://h:8081"
        ));
        // docker 内显式 local 非法（容器内仅远程）——通过 validate_config 断言
        // （resolve_deploy 依赖真实 /.dockerenv，无法在单测中切换容器环境）
        // 本地环境 local 无下载源 → validate_config 拒绝
        let no_sources = DecisionModelConfig {
            modelscope_repo: String::new(),
            github_release_url: String::new(),
            ..cfg_with(None, None)
        };
        assert!(super::validate_config(&no_sources).is_err());
        // remote 有 URL → validate_config 通过（无需下载源）
        let remote_ok = cfg_with(Some(M::Remote), Some("http://h:8081"));
        assert!(super::validate_config(&remote_ok).is_ok());
    }

    #[test]
    fn project_default_params_match_contract() {
        let p = super::project_default_params();
        assert_eq!(p.letter_token_ids[0], 32);
        assert_eq!(p.letter_token_ids[25], 57);
    }
    use super::*;

    /// 构造测试归档（files: 相对路径 -> 内容），返回归档路径
    fn build_test_archive(dir: &std::path::Path, files: &[(&str, &[u8])]) -> std::path::PathBuf {
        let archive = dir.join("runtime.tar.gz");
        let f = std::fs::File::create(&archive).expect("创建归档");
        let enc = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
        let mut builder = tar::Builder::new(enc);
        for (name, content) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, name, *content)
                .expect("写入归档条目");
        }
        builder.into_inner().expect("封口").finish().expect("flush");
        archive
    }

    #[test]
    fn extract_runtime_archive_roundtrip_and_traversal_guard() {
        let dir = tempfile::tempdir().expect("临时目录");
        let archive = build_test_archive(
            dir.path(),
            &[("llama-server", b"launcher"), ("libggml.so", b"lib")],
        );
        let dest = tempfile::tempdir().expect("目标目录");
        extract_runtime_archive(&archive, dest.path()).expect("解压成功");
        let launcher = dest.path().join("llama-server");
        assert_eq!(std::fs::read(&launcher).expect("读回"), b"launcher");
        assert!(dest.path().join("libggml.so").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&launcher)
                .expect("元数据")
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "归档内 0o755 权限应被保留");
        }

        // 路径穿越条目：整包拒绝（Builder 会拦 .. 路径，这里用 GnuHeader
        // 原始字节构造恶意归档来测解压端防御）
        let evil = dir.path().join("evil.tar.gz");
        {
            let f = std::fs::File::create(&evil).expect("创建恶意归档");
            let enc = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
            let mut builder = tar::Builder::new(enc);
            let content: &[u8] = b"boom";
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            header.as_gnu_mut().expect("gnu header").name = {
                let mut name = [0u8; 100];
                let evil = b"../evil.txt";
                name[..evil.len()].copy_from_slice(evil);
                name
            };
            builder.append(&header, content).expect("写入恶意条目");
            builder.into_inner().expect("封口").finish().expect("flush");
        }
        let dest2 = tempfile::tempdir().expect("目标目录");
        assert!(
            extract_runtime_archive(&evil, dest2.path()).is_err(),
            "含 .. 分量的归档必须被拒绝"
        );
        assert!(
            !dest2
                .path()
                .parent()
                .expect("parent")
                .join("evil.txt")
                .exists(),
            "穿越文件不得落盘"
        );
    }

    #[test]
    fn llama_server_launcher_name_by_target_os() {
        // 与 cfg!(windows) 一致的目标平台命名（编译期确定，测试固化契约）
        if cfg!(windows) {
            assert_eq!(llama_server_launcher_name(), "llama-server.exe");
        } else {
            assert_eq!(llama_server_launcher_name(), "llama-server");
        }
    }

    #[test]
    fn resolve_quant_default_and_low_memory_downgrade() {
        let mut cfg = DecisionModelConfig::default();
        assert_eq!(resolve_quant(&cfg), "q5_k_m");

        // 非法档位回退
        cfg.quant = "q3_k_x".into();
        assert_eq!(resolve_quant(&cfg), "q5_k_m");

        // 低配降档：阈值置 0 关闭
        cfg.quant = "q8_0".into();
        cfg.low_memory_threshold_mb = 0;
        assert_eq!(resolve_quant(&cfg), "q8_0");
    }

    #[test]
    fn metrics_snapshot_shape() {
        metrics::record_tick_attempt();
        metrics::record_tick_attempt();
        metrics::record_taken();
        metrics::record_act1_confidence(0.95);
        metrics::record_call(true, 42);
        let snap = metrics::snapshot();
        assert_eq!(snap["decision_ticks"], 2);
        assert_eq!(snap["decision_taken"], 1);
        assert_eq!(snap["take_rate"], 50.0);
        assert_eq!(snap["calls"], 1);
        assert_eq!(snap["avg_call_ms"], 42);
        let buckets = snap["act1_confidence_buckets"].as_array().expect("桶");
        assert_eq!(buckets.len(), 10);
        assert_eq!(buckets[9], 1);
    }

    #[tokio::test]
    async fn manager_without_sources_fails_fast() {
        let (tx, _rx) = broadcast::channel(4);
        let cfg = DecisionModelConfig {
            enabled: true,
            modelscope_repo: String::new(),
            github_release_url: String::new(),
            ..Default::default()
        };
        let mgr = DecisionModelManager::new(cfg, tx);
        assert!(mgr.is_enabled(), "默认启用");
        assert!(matches!(mgr.status().await, LifecycleStatus::NotInstalled));
        assert!(mgr.ensure_installed().await.is_err(), "无下载源应失败");
        assert!(matches!(mgr.status().await, LifecycleStatus::Failed { .. }));
        assert!(mgr.ready_install().await.is_err(), "未就绪时热路径必须拒绝");
    }
}
