// ============================================================================
// Agent 自更新模块（GitHub Release）
// ============================================================================
//
// 更新判定采用 digest 恒等比较：运行时对当前可执行文件计算 sha256，与最新
// release 中本平台资产的 `digest` 字段（GitHub 在资产上传时自动生成）对比。
// 不一致即视为需要更新。之所以不用版本号比较：release tag 取自 server crate
// 版本（见 release skill），而 agent crate 版本独立演进，两者不可比。
//
// 安全阀（fail-safe）：
// - 环境变量 CYBER_JIANGHU_SELF_UPDATE=0/false/off 强制禁用（hard disable）
// - 容器内（/.dockerenv 或 /.containerenv）拒绝 apply——容器层不可变，应换镜像
// - cargo 本地构建（exe 位于 target/ 下）拒绝 apply——保护开发构建
// - 资产缺少 digest 或下载内容校验不符一律拒绝安装
//
// 重启语义：apply_and_restart 在安装成功后自替换进程——unix 用 execve 同路径
// 重启（保留 argv/env），Windows 先 spawn 新进程再 exit。CLI 模式只安装不重启。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, RwLock};

use crate::config::UpdateConfig;

/// 硬禁用自更新的环境变量（值 0/false/off 时不做任何 GitHub 网络请求）
pub const ENV_SELF_UPDATE_DISABLE: &str = "CYBER_JIANGHU_SELF_UPDATE";

const GITHUB_API_BASE: &str = "https://api.github.com";
const STATE_FILE_NAME: &str = "update_state.json";
/// 后台任务首次检查前的等待（给启动流程让路，错峰避免雷群）
const INITIAL_DELAY_SECS: u64 = 90;
/// 检查间隔的下限钳位（防止配置过小造成热循环）
const MIN_CHECK_INTERVAL_SECS: u64 = 600;
/// 每次检查附加的随机抖动上限（错峰）
const JITTER_MAX_SECS: u64 = 300;
/// 元数据请求超时
const API_TIMEOUT_SECS: u64 = 15;
/// 资产下载超时（裸二进制约 20MB，按慢速网络留余量）
const DOWNLOAD_TIMEOUT_SECS: u64 = 600;

// ============================================================================
// GitHub API 数据模型
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub size: u64,
    /// GitHub 自动生成的资产摘要，形如 "sha256:<hex>"；缺失时拒绝安装
    #[serde(default)]
    pub digest: Option<String>,
    pub browser_download_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseInfo {
    pub tag_name: String,
    #[serde(default)]
    pub published_at: Option<String>,
    pub assets: Vec<ReleaseAsset>,
}

/// 最近一次检查到的 release 摘要（持久化到 update_state.json，供 status 透出）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LatestSummary {
    pub tag_name: String,
    #[serde(default)]
    pub published_at: Option<String>,
    pub asset_name: String,
    #[serde(default)]
    pub asset_digest: Option<String>,
    #[serde(default)]
    pub asset_size: u64,
}

// ============================================================================
// 持久化状态
// ============================================================================

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateState {
    #[serde(default)]
    pub last_check_unix: Option<i64>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub latest: Option<LatestSummary>,
    #[serde(default)]
    pub installed_tag: Option<String>,
    #[serde(default)]
    pub installed_digest: Option<String>,
    #[serde(default)]
    pub installed_at_unix: Option<i64>,
}

// ============================================================================
// 结果类型
// ============================================================================

#[derive(Debug, Clone)]
pub struct CheckResult {
    pub release_tag: String,
    pub asset_name: String,
    pub asset_digest: Option<String>,
    pub asset_url: String,
    pub current_digest: String,
    pub update_available: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ApplyOutcome {
    /// 当前二进制与最新资产一致，无需更新
    UpToDate,
    /// 新版本已安装到磁盘（等待重启生效）
    Installed { tag: String },
}

/// status 端点 / CLI 的完整状态视图
#[derive(Debug, Serialize)]
pub struct UpdateStatus {
    pub enabled: bool,
    pub auto_apply: bool,
    pub check_interval_secs: u64,
    pub repo: String,
    pub hard_disabled: bool,
    pub in_container: bool,
    pub dev_build: bool,
    pub current_version: String,
    pub current_digest: Option<String>,
    pub latest: Option<LatestSummary>,
    /// None = 尚未检查或 digest 信息不足，无法判定
    pub update_available: Option<bool>,
    pub last_check_unix: Option<i64>,
    pub last_error: Option<String>,
    pub installed_tag: Option<String>,
    pub installed_at_unix: Option<i64>,
}

// ============================================================================
// Updater
// ============================================================================

pub struct Updater {
    config: UpdateConfig,
    state: RwLock<UpdateState>,
    state_path: PathBuf,
    http: reqwest::Client,
    /// 当前 exe 的 sha256（每进程计算一次后缓存；安装新版本后置 None 失效）
    current_digest_cache: RwLock<Option<String>>,
    /// 进程内串行化 apply：后台任务与 HTTP/CLI 手动触发并发时，
    /// 避免对同一 release 重复下载与安装（跨进程并发依赖
    /// sha256 校验 + 同目录 rename 原子性，结果幂等）
    apply_lock: Mutex<()>,
}

impl Updater {
    pub fn new(config: UpdateConfig) -> Self {
        let state_path = crate::config::data_base_dir().join(STATE_FILE_NAME);
        let state = Self::load_state(&state_path).unwrap_or_default();
        Self::cleanup_windows_old();
        let http = reqwest::Client::builder()
            .user_agent(Self::user_agent())
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("构建 reqwest client 失败");
        Self {
            config,
            state: RwLock::new(state),
            state_path,
            http,
            current_digest_cache: RwLock::new(None),
            apply_lock: Mutex::new(()),
        }
    }

    pub fn config(&self) -> &UpdateConfig {
        &self.config
    }

    fn user_agent() -> String {
        format!("cyber-jianghu-agent/{}", env!("CARGO_PKG_VERSION"))
    }

    // ------------------------------------------------------------------
    // 环境探测
    // ------------------------------------------------------------------

    /// CYBER_JIANGHU_SELF_UPDATE=0/false/off 时硬禁用（不做任何更新网络请求）
    pub fn hard_disabled() -> bool {
        match std::env::var(ENV_SELF_UPDATE_DISABLE) {
            Ok(v) => matches!(v.trim().to_lowercase().as_str(), "0" | "false" | "off"),
            Err(_) => false,
        }
    }

    /// 容器内运行（docker/podman）。容器层不可变，自更新在重建容器后会回滚，
    /// 正确路径是更新镜像（scripts/deploy/build-agent-image.sh）。
    pub fn in_container() -> bool {
        Path::new("/.dockerenv").exists() || Path::new("/.containerenv").exists()
    }

    /// cargo 本地构建产物（exe 位于 target/<debug|release|triple>/ 下）。
    /// 自动替换会把开发中的本地构建静默降级为最新 release 二进制，必须跳过。
    pub fn is_dev_build(exe: &Path) -> bool {
        use std::path::Component;
        let comps: Vec<Component> = exe.components().collect();
        comps.windows(2).any(|w| {
            let cur = w[0].as_os_str().to_str().unwrap_or_default();
            let next = w[1].as_os_str().to_str().unwrap_or_default();
            if cur != "target" {
                return false;
            }
            // target/ 后紧跟 debug、release 或编译三元组（如 x86_64-unknown-linux-musl）
            next == "debug" || next == "release" || next.contains('-')
        })
    }

    // ------------------------------------------------------------------
    // 平台资产
    // ------------------------------------------------------------------

    /// CI 发布的 agent 资产命名（.github/workflows/ci.yml build matrix）。
    /// macOS x86_64 无发布产物，返回 None。
    pub fn platform_asset_name() -> Option<&'static str> {
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("linux", "x86_64") => Some("cyber-jianghu-agent-linux-x86_64"),
            ("linux", "aarch64") => Some("cyber-jianghu-agent-linux-arm64"),
            ("macos", "aarch64") => Some("cyber-jianghu-agent-macos-arm64"),
            ("windows", "x86_64") => Some("cyber-jianghu-agent-windows-x86_64.exe"),
            _ => None,
        }
    }

    fn pick_asset(release: &ReleaseInfo) -> Option<&ReleaseAsset> {
        let want = Self::platform_asset_name()?;
        release.assets.iter().find(|a| a.name == want)
    }

    // ------------------------------------------------------------------
    // 网络请求
    // ------------------------------------------------------------------

    async fn fetch_latest(&self) -> Result<ReleaseInfo> {
        let url = format!(
            "{GITHUB_API_BASE}/repos/{}/releases/latest",
            self.config.repo
        );
        let resp = self
            .http
            .get(&url)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .timeout(Duration::from_secs(API_TIMEOUT_SECS))
            .send()
            .await
            .with_context(|| format!("请求 GitHub Releases API 失败: {url}"))?;
        match resp.status() {
            s if s.is_success() => {}
            reqwest::StatusCode::NOT_FOUND => {
                bail!("仓库 {} 尚无正式 release（404）", self.config.repo)
            }
            s => bail!("GitHub API 返回 {s}（rate limit 或仓库不可达）"),
        }
        resp.json::<ReleaseInfo>()
            .await
            .context("解析 release 响应失败")
    }

    pub async fn check(&self) -> Result<CheckResult> {
        if Self::hard_disabled() {
            bail!("自更新已被环境变量 {ENV_SELF_UPDATE_DISABLE} 禁用");
        }
        let release = match self.fetch_latest().await {
            Ok(r) => r,
            Err(e) => {
                self.mutate_state(|s| s.last_error = Some(format!("{e:#}")))
                    .await;
                return Err(e);
            }
        };
        let asset = Self::pick_asset(&release).ok_or_else(|| {
            anyhow!(
                "release {} 不含当前平台资产 {}（tag 为 server 版本；本平台可能无发布产物）",
                release.tag_name,
                Self::platform_asset_name().unwrap_or("<不支持的平台>")
            )
        })?;
        let current_digest = self.current_digest().await?;
        let update_available = asset.digest.as_deref() != Some(current_digest.as_str());

        let summary = LatestSummary {
            tag_name: release.tag_name.clone(),
            published_at: release.published_at.clone(),
            asset_name: asset.name.clone(),
            asset_digest: asset.digest.clone(),
            asset_size: asset.size,
        };
        self.mutate_state(|s| {
            s.last_check_unix = Some(chrono::Utc::now().timestamp());
            s.last_error = None;
            s.latest = Some(summary);
        })
        .await;

        Ok(CheckResult {
            release_tag: release.tag_name.clone(),
            asset_name: asset.name.clone(),
            asset_digest: asset.digest.clone(),
            asset_url: asset.browser_download_url.clone(),
            current_digest,
            update_available,
        })
    }

    /// 下载资产到 exe 同目录的临时文件，流式校验 sha256。
    /// 校验失败立即删除临时文件并报错（fail-safe：绝不安装未验证内容）。
    async fn download_to_temp(&self, url: &str, expected_sha: &str) -> Result<PathBuf> {
        let exe = std::env::current_exe().context("定位当前可执行文件失败")?;
        let temp = Self::temp_download_path(&exe);
        let resp = self
            .http
            .get(url)
            .timeout(Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))
            .send()
            .await
            .with_context(|| format!("下载更新失败: {url}"))?;
        let status = resp.status();
        if !status.is_success() {
            bail!("下载更新失败: HTTP {status}");
        }

        let mut file = std::fs::File::create(&temp)
            .with_context(|| format!("创建临时文件失败: {}", temp.display()))?;
        let mut hasher = Sha256::new();
        let mut resp = resp;
        while let Some(chunk) = resp.chunk().await.context("下载中断")? {
            hasher.update(&chunk);
            file.write_all(&chunk)
                .with_context(|| format!("写入临时文件失败: {}", temp.display()))?;
        }
        file.flush().ok();

        let actual = format!("sha256:{}", hex::encode(hasher.finalize()));
        if actual != expected_sha {
            let _ = std::fs::remove_file(&temp);
            bail!("下载内容校验失败: 期望 {expected_sha}，实际 {actual}");
        }
        Ok(temp)
    }

    // ------------------------------------------------------------------
    // 安装与重启
    // ------------------------------------------------------------------

    fn temp_download_path(exe: &Path) -> PathBuf {
        let mut s = exe.as_os_str().to_os_string();
        s.push(".download");
        PathBuf::from(s)
    }

    /// 将已校验的下载文件替换到 exe 路径。
    /// - unix: 同目录 rename 原子替换 + 0755 权限
    /// - windows: 运行中 exe 不可删除但可改名——先改名旧文件为 *.exe.old，再移入新文件
    fn install(downloaded: &Path, exe: &Path) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(downloaded)
                .with_context(|| format!("读取下载文件元数据失败: {}", downloaded.display()))?
                .permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(downloaded, perms)
                .with_context(|| format!("设置执行权限失败: {}", downloaded.display()))?;
            std::fs::rename(downloaded, exe).with_context(|| {
                format!(
                    "替换二进制失败: {} -> {}",
                    downloaded.display(),
                    exe.display()
                )
            })?;
        }
        #[cfg(windows)]
        {
            let mut old = exe.as_os_str().to_os_string();
            old.push(".old");
            let old = PathBuf::from(old);
            // 清理上一次更新残留（不在运行，可安全删除）
            let _ = std::fs::remove_file(&old);
            if exe.exists() {
                std::fs::rename(exe, &old)
                    .with_context(|| format!("改名旧二进制失败: {}", exe.display()))?;
            }
            std::fs::rename(downloaded, exe)
                .with_context(|| format!("移入新二进制失败: {}", exe.display()))?;
        }
        Ok(())
    }

    /// 重启进程以加载新二进制。
    /// - unix: execve 同路径重启（保留 argv/env，进程号不变）
    /// - windows: spawn 新进程后 exit（旧 exe 已改名让位）
    pub fn restart_self() -> Result<()> {
        let exe = std::env::current_exe().context("定位当前可执行文件失败")?;
        let args: Vec<String> = std::env::args().skip(1).collect();

        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            tracing::info!("重启进程以加载更新: {}", exe.display());
            // exec 成功时不返回；返回即代表失败
            let err = std::process::Command::new(&exe).args(&args).exec();
            bail!("重启失败（exec 返回）: {err}");
        }
        #[cfg(windows)]
        {
            // 等待本进程释放端口后再拉起新进程，降低端口竞争
            std::thread::sleep(Duration::from_millis(300));
            tracing::info!("拉起新进程以加载更新: {}", exe.display());
            std::process::Command::new(&exe)
                .args(&args)
                .spawn()
                .with_context(|| format!("spawn 新进程失败: {}", exe.display()))?;
            std::process::exit(0);
        }
    }

    /// 安装新版本（不重启）。CLI 用此入口——CLI 进程重启自身会陷入循环。
    ///
    /// 进程内通过 apply_lock 串行化；持锁期间完成检查→下载→安装全流程，
    /// 并发触发方在后等到锁时会看到最新 digest（安装后已失效缓存），
    /// 从而正确返回 UpToDate 而非重复安装。
    pub async fn apply(&self) -> Result<ApplyOutcome> {
        let _guard = self.apply_lock.lock().await;
        if Self::hard_disabled() {
            bail!("自更新已被环境变量 {ENV_SELF_UPDATE_DISABLE} 禁用");
        }
        if Self::in_container() {
            bail!(
                "容器内运行（检测到 /.dockerenv 或 /.containerenv），二进制自更新会在容器重建后回滚；请通过更新镜像升级（scripts/deploy/build-agent-image.sh）"
            );
        }
        let exe = std::env::current_exe().context("定位当前可执行文件失败")?;
        if Self::is_dev_build(&exe) {
            bail!(
                "cargo 本地构建产物（{}），不执行自更新；请安装 release 版本或手动 cargo build",
                exe.display()
            );
        }

        let check = self.check().await?;
        if !check.update_available {
            return Ok(ApplyOutcome::UpToDate);
        }
        let expected_sha = check
            .asset_digest
            .clone()
            .ok_or_else(|| anyhow!("release 资产缺少 sha256 digest，拒绝安装（fail-safe）"))?;

        tracing::info!(
            "开始下载更新: {} {} ({})",
            check.release_tag,
            check.asset_name,
            check.asset_url
        );
        let temp = self
            .download_to_temp(&check.asset_url, &expected_sha)
            .await?;
        Self::install(&temp, &exe)?;
        // 磁盘上的 exe 已变为新版本：失效缓存，使后续 check/status 反映
        // 新文件的 digest（否则串行化的下一次 apply 会误判为仍需更新）
        *self.current_digest_cache.write().await = None;
        let tag = check.release_tag.clone();
        self.mutate_state(|s| {
            s.installed_tag = Some(tag.clone());
            s.installed_digest = Some(expected_sha);
            s.installed_at_unix = Some(chrono::Utc::now().timestamp());
        })
        .await;
        tracing::info!("更新已安装: {tag}，重启后生效");
        Ok(ApplyOutcome::Installed {
            tag: check.release_tag,
        })
    }

    /// 安装并重启（后台自动更新 / HTTP apply 用）
    pub async fn apply_and_restart(&self) -> Result<ApplyOutcome> {
        let outcome = self.apply().await?;
        if matches!(outcome, ApplyOutcome::Installed { .. }) {
            Self::restart_self()?;
        }
        Ok(outcome)
    }

    /// 清理 Windows 上一次更新残留的 *.exe.old（尽力而为）
    fn cleanup_windows_old() {
        #[cfg(windows)]
        {
            if let Ok(exe) = std::env::current_exe() {
                let mut old = exe.as_os_str().to_os_string();
                old.push(".old");
                let _ = std::fs::remove_file(PathBuf::from(old));
            }
        }
        #[cfg(not(windows))]
        {}
    }

    // ------------------------------------------------------------------
    // digest 计算
    // ------------------------------------------------------------------

    async fn current_digest(&self) -> Result<String> {
        if let Some(d) = self.current_digest_cache.read().await.clone() {
            return Ok(d);
        }
        let d = tokio::task::spawn_blocking(|| {
            let exe = std::env::current_exe().context("定位当前可执行文件失败")?;
            Self::sha256_file(&exe).with_context(|| format!("计算哈希失败: {}", exe.display()))
        })
        .await
        .context("哈希任务 join 失败")??;
        *self.current_digest_cache.write().await = Some(d.clone());
        Ok(d)
    }

    fn sha256_file(path: &Path) -> Result<String> {
        let mut f = std::fs::File::open(path)
            .with_context(|| format!("打开文件失败: {}", path.display()))?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 65536];
        loop {
            let n = f
                .read(&mut buf)
                .with_context(|| format!("读取文件失败: {}", path.display()))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
    }

    // ------------------------------------------------------------------
    // 状态持久化
    // ------------------------------------------------------------------

    fn load_state(path: &Path) -> Option<UpdateState> {
        let content = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&content).ok()
    }

    async fn mutate_state<F: FnOnce(&mut UpdateState)>(&self, f: F) {
        let mut guard = self.state.write().await;
        f(&mut guard);
        // 持久化失败不影响更新流程，仅记录（状态文件只是可观测性辅助）
        if let Ok(json) = serde_json::to_string_pretty(&*guard) {
            if let Some(parent) = self.state_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let tmp = self
                .state_path
                .with_extension(format!("json.tmp.{}", std::process::id()));
            if std::fs::write(&tmp, &json).is_ok() {
                let _ = std::fs::rename(&tmp, &self.state_path);
            } else {
                tracing::warn!("自更新状态写入失败: {}", self.state_path.display());
            }
        }
    }

    /// 聚合状态视图（供 GET /api/v1/update/status）
    pub async fn status(&self) -> UpdateStatus {
        let state = self.state.read().await.clone();
        let exe = std::env::current_exe().ok();
        let current_digest = self.current_digest().await.ok();
        let update_available = match (&state.latest, &current_digest) {
            (Some(latest), Some(cur)) => latest.asset_digest.as_deref().map(|d| d != cur),
            _ => None,
        };
        UpdateStatus {
            enabled: self.config.enabled,
            auto_apply: self.config.auto_apply,
            check_interval_secs: self.config.check_interval_secs,
            repo: self.config.repo.clone(),
            hard_disabled: Self::hard_disabled(),
            in_container: Self::in_container(),
            dev_build: exe.as_deref().is_some_and(Self::is_dev_build),
            current_version: env!("CARGO_PKG_VERSION").to_string(),
            current_digest,
            latest: state.latest,
            update_available,
            last_check_unix: state.last_check_unix,
            last_error: state.last_error,
            installed_tag: state.installed_tag,
            installed_at_unix: state.installed_at_unix,
        }
    }
}

// ============================================================================
// 后台自动更新循环
// ============================================================================

fn jitter() -> Duration {
    use rand::RngExt;
    Duration::from_secs(rand::rng().random_range(0..=JITTER_MAX_SECS))
}

/// 后台自动更新任务：周期检查 latest release，发现 digest 不一致即下载安装并重启。
/// 硬禁用 / 配置关闭 / 容器内运行时静默退出（原因记入日志）。
pub async fn run_background(updater: Arc<Updater>) {
    if Updater::hard_disabled() {
        tracing::info!("自更新被 {ENV_SELF_UPDATE_DISABLE} 硬禁用，后台任务不启动");
        return;
    }
    if !updater.config.enabled {
        tracing::info!("自动更新已关闭（agent.yaml update.enabled=false）");
        return;
    }
    if Updater::in_container() {
        tracing::info!("容器内运行，自更新后台任务不启动（请通过更新镜像升级）");
        return;
    }
    if std::env::current_exe()
        .ok()
        .as_deref()
        .is_some_and(Updater::is_dev_build)
    {
        tracing::info!("cargo 本地构建产物，自更新后台任务不启动");
        return;
    }
    let interval = Duration::from_secs(
        updater
            .config
            .check_interval_secs
            .max(MIN_CHECK_INTERVAL_SECS),
    );
    tracing::info!(
        "自动更新已启用: repo={}, 检查间隔 {}s, auto_apply={}",
        updater.config.repo,
        interval.as_secs(),
        updater.config.auto_apply
    );

    tokio::time::sleep(Duration::from_secs(INITIAL_DELAY_SECS) + jitter()).await;
    loop {
        match updater.check().await {
            Ok(r) if r.update_available => {
                if updater.config.auto_apply {
                    // unix: 成功时 exec 不返回；返回即失败
                    if let Err(e) = updater.apply_and_restart().await {
                        tracing::warn!("自动更新失败: {e:#}");
                    }
                } else {
                    tracing::info!(
                        "发现新版本 {}（auto_apply=false，仅提示；可通过 CLI `cyber-jianghu-agent update` 或 POST /api/v1/update/apply 手动安装）",
                        r.release_tag
                    );
                }
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("更新检查失败: {e:#}"),
        }
        tokio::time::sleep(interval + jitter()).await;
    }
}

// ============================================================================
// CLI 入口
// ============================================================================

/// 从 agent.yaml 读取 update 段（文件缺失 / 段缺失 / 解析失败一律回退默认值）
pub fn load_update_config() -> UpdateConfig {
    let path = crate::config::config_dir().join("agent.yaml");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return UpdateConfig::default();
    };
    let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(&content) else {
        return UpdateConfig::default();
    };
    value
        .get("update")
        .cloned()
        .and_then(|v| serde_yaml::from_value::<UpdateConfig>(v).ok())
        .unwrap_or_default()
}

/// `cyber-jianghu-agent update [--check-only]`：手动检查 / 安装。
/// CLI 只安装不重启（重启 CLI 自身会陷入执行循环）。
pub async fn run_cli_update(check_only: bool) -> Result<()> {
    let config = load_update_config();
    println!("agent 版本: {}", env!("CARGO_PKG_VERSION"));
    println!(
        "更新源: {GITHUB_API_BASE}/repos/{}/releases/latest",
        config.repo
    );

    let updater = Updater::new(config);
    if check_only {
        match updater.check().await {
            Ok(r) if r.update_available => {
                println!(
                    "发现新版本: {}（资产 {}，当前二进制与最新资产不一致）",
                    r.release_tag, r.asset_name
                );
                Ok(())
            }
            Ok(_) => {
                println!("已是最新（当前二进制与最新 release 资产一致）");
                Ok(())
            }
            Err(e) => Err(e),
        }
    } else {
        match updater.apply().await {
            Ok(ApplyOutcome::UpToDate) => {
                println!("已是最新，无需更新");
                Ok(())
            }
            Ok(ApplyOutcome::Installed { tag }) => {
                println!("已安装 {tag}；重启 agent 后生效");
                Ok(())
            }
            Err(e) => Err(e),
        }
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
#[path = "updater_tests.rs"]
mod tests;
