// ============================================================================
// 决策模型下载器：ModelScope 主源 + GitHub Release 备源，HTTP Range 断点续传
// ============================================================================
//
// 下载流程（单文件）：
//   1. 按"ModelScope resolve → GitHub latest/download"顺序构造候选 URL；
//   2. 已有部分文件则先流式哈希既有前缀，带 Range: bytes={n}- 续传；
//      服务端不支持 Range（返回 200）时静默从头重下；
//   3. 边收边写 + 边算 sha256，完成后校验清单摘要，不符即删档换下一源；
//   4. 全部候选源失败才返回 Err（上层记 Failed 状态并回退 LLM 路径）。
//
// 版本目录：install_dir/{manifest.version}/（如 model-v1/）。升级 = 下载新版本
// 目录后原子改写 current.json 指针；旧目录保留供回滚（由 current.json 指回即可）。

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use super::manifest::{ManifestFile, Sha256Stream};

/// 下载进度（广播给 SSE 订阅者）
#[derive(Debug, Clone, serde::Serialize)]
pub struct DownloadProgress {
    pub file: String,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    /// 完成标志（该文件 sha256 已校验通过）
    pub done: bool,
}

/// 下载源配置（来自 DecisionModelConfig）
#[derive(Debug, Clone, Default)]
pub struct DownloadSource {
    /// ModelScope 模型仓 "owner/repo"（主源，空串跳过）
    pub modelscope_repo: String,
    /// GitHub Releases 基址（备源，空串跳过）
    pub github_release_base: String,
}

/// 构造候选下载 URL（ModelScope 优先，GitHub Release 兜底）
///
/// `filename` 可含子目录（如 "llama-server-b11408/x.tar.gz"）：
/// - ModelScope 按完整相对路径解析（仓目录结构即下载路径）；
/// - GitHub Release 资产恒为平铺，取 basename 拼接。
pub fn candidate_urls(source: &DownloadSource, filename: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let repo = source.modelscope_repo.trim().trim_end_matches('/');
    if !repo.is_empty() {
        urls.push(format!(
            "https://www.modelscope.cn/models/{repo}/resolve/master/{filename}"
        ));
    }
    let gh = source.github_release_url_trimmed();
    if !gh.is_empty() {
        let basename = filename.rsplit('/').next().unwrap_or(filename);
        urls.push(format!("{gh}/latest/download/{basename}"));
    }
    urls
}

impl DownloadSource {
    fn github_release_url_trimmed(&self) -> String {
        self.github_release_base
            .trim()
            .trim_end_matches('/')
            .to_string()
    }

    pub fn is_empty(&self) -> bool {
        self.modelscope_repo.trim().is_empty() && self.github_release_base.trim().is_empty()
    }
}

/// 下载器（持有带超时的 HTTP 客户端）
pub struct Downloader {
    http: reqwest::Client,
}

impl Default for Downloader {
    fn default() -> Self {
        Self::new()
    }
}

impl Downloader {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .user_agent(format!("cyber-jianghu-agent/{}", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15))
            // 单连接读超时：大文件慢速网络下以进度为准，这里只防死链
            .read_timeout(Duration::from_secs(120))
            .build()
            .expect("构建决策模型下载 reqwest client 失败");
        Self { http }
    }

    /// 下载单个文件到 dest，逐源尝试，断点续传，sha256 终验。
    ///
    /// `progress` 以 (文件名, 已下载, 总量, 是否完成) 上报；内部按 1MB 步进节流，
    /// 上层再决定是否广播。
    pub async fn download_file(
        &self,
        urls: &[String],
        dest: &Path,
        entry: &ManifestFile,
        mut progress: impl FnMut(DownloadProgress),
    ) -> Result<u64> {
        if urls.is_empty() {
            bail!("无可用下载源（modelscope_repo 与 github_release_url 均未配置）");
        }
        let dest = PathBuf::from(dest);
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("创建下载目录失败: {}", parent.display()))?;
        }

        let mut last_err: Option<anyhow::Error> = None;
        for url in urls {
            match self.try_one(url, &dest, entry, &mut progress).await {
                Ok(bytes) => {
                    info!(
                        "决策模型文件下载完成: {} ({} bytes) <- {}",
                        entry.name, bytes, url
                    );
                    return Ok(bytes);
                }
                Err(e) => {
                    warn!("决策模型下载源失败: {} -> {:#}", url, e);
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow!("下载 {} 失败：无可用源", entry.name)))
            .with_context(|| format!("文件 {} 全部下载源均失败", entry.name))
    }

    async fn try_one(
        &self,
        url: &str,
        dest: &Path,
        entry: &ManifestFile,
        progress: &mut impl FnMut(DownloadProgress),
    ) -> Result<u64> {
        // 既有内容：可校验且一致则跳过下载；否则作为续传前缀
        let existing = tokio::fs::read(dest).await.ok();
        let expect_sha = entry.sha256.trim();
        if let Some(bytes) = existing.as_ref()
            && !expect_sha.is_empty()
        {
            let actual = hex::encode(Sha256::digest(bytes));
            if actual == expect_sha.to_ascii_lowercase() {
                progress(DownloadProgress {
                    file: entry.name.clone(),
                    downloaded_bytes: entry.size,
                    total_bytes: entry.size,
                    done: true,
                });
                return Ok(bytes.len() as u64);
            }
        }
        let resume_from = existing.as_ref().map(|b| b.len() as u64).unwrap_or(0);

        let mut req = self.http.get(url);
        if resume_from > 0 {
            req = req.header("Range", format!("bytes={resume_from}-"));
        }
        let resp = req
            .send()
            .await
            .with_context(|| format!("请求失败: {url}"))?;
        let status = resp.status();
        if !status.is_success() {
            bail!("HTTP {status}");
        }

        // 续传协商：请求了 Range 但服务端返回 200（不支持），从头重下
        let appending = status == reqwest::StatusCode::PARTIAL_CONTENT && resume_from > 0;
        if resume_from > 0 && !appending {
            warn!(
                "服务端不支持 Range 续传（HTTP 200），从头重下: {}",
                entry.name
            );
        }
        let start_offset = if appending { resume_from } else { 0 };

        let mut hasher = Sha256Stream::new();
        let mut buffer: Vec<u8> = Vec::with_capacity(1024 * 1024);
        if appending && let Some(bytes) = existing.as_ref() {
            hasher.update(bytes);
        }

        let mut stream = resp.bytes_stream();
        let mut next_progress_at = 1024u64 * 1024; // 1MB 步进上报
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.with_context(|| format!("读取响应流中断: {url}"))?;
            hasher.update(&chunk);
            buffer.extend_from_slice(&chunk);
            if buffer.len() >= 1024 * 1024 {
                append_chunk(dest, &mut buffer, start_offset).await?;
            }
            if hasher.bytes_written() >= next_progress_at {
                next_progress_at += 1024 * 1024;
                progress(DownloadProgress {
                    file: entry.name.clone(),
                    downloaded_bytes: hasher.bytes_written(),
                    total_bytes: entry.size,
                    done: false,
                });
            }
        }
        append_chunk(dest, &mut buffer, start_offset).await?;

        // hasher 已计入续传前缀（appending 时 update 过 existing），不再叠加 start_offset
        let total = hasher.bytes_written();
        if entry.size > 0 && total != entry.size {
            tokio::fs::remove_file(dest).await.ok();
            bail!("文件大小不符: 期望 {} 实际 {}", entry.size, total);
        }
        // 无清单摘要的条目（manifest.json 自身）跳过 sha 校验
        if !expect_sha.is_empty() {
            let actual = hasher.finalize_hex();
            if actual != expect_sha.to_ascii_lowercase() {
                tokio::fs::remove_file(dest).await.ok();
                bail!("sha256 校验不符: 期望 {} 实际 {}", entry.sha256, actual);
            }
        }
        progress(DownloadProgress {
            file: entry.name.clone(),
            downloaded_bytes: total,
            total_bytes: entry.size,
            done: true,
        });
        Ok(total)
    }
}

/// 把缓冲内容落到文件：append 模式从文件末尾追加，overwrite 模式覆盖写。
async fn append_chunk(dest: &Path, buffer: &mut Vec<u8>, start_offset: u64) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    if buffer.is_empty() {
        return Ok(());
    }
    if start_offset > 0 {
        // 续传：从既有前缀末尾追加
        let mut f = tokio::fs::OpenOptions::new()
            .append(true)
            .open(dest)
            .await
            .with_context(|| format!("打开续传目标失败: {}", dest.display()))?;
        f.write_all(buffer)
            .await
            .with_context(|| format!("写入续传数据失败: {}", dest.display()))?;
        f.flush().await.ok();
    } else {
        let mut f = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(dest)
            .await
            .with_context(|| format!("打开下载目标失败: {}", dest.display()))?;
        f.write_all(buffer)
            .await
            .with_context(|| format!("写入下载数据失败: {}", dest.display()))?;
        f.flush().await.ok();
    }
    buffer.clear();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_urls_order_and_shape() {
        let src = DownloadSource {
            modelscope_repo: "8kugames/CyberJianghu-Decision-2B".into(),
            github_release_base: "https://github.com/8kugames/CyberJianghu-Decision-2B/releases/"
                .into(),
        };
        let urls = candidate_urls(&src, "manifest.json");
        assert_eq!(
            urls[0],
            "https://www.modelscope.cn/models/8kugames/CyberJianghu-Decision-2B/resolve/master/manifest.json"
        );
        assert_eq!(
            urls[1],
            "https://github.com/8kugames/CyberJianghu-Decision-2B/releases/latest/download/manifest.json"
        );
    }

    #[test]
    fn candidate_urls_subdir_name() {
        let src = DownloadSource {
            modelscope_repo: "o/repo".into(),
            github_release_base: "https://github.com/o/repo/releases".into(),
        };
        let urls = candidate_urls(&src, "llama-server-b11408/llama-server-macos-arm64.tar.gz");
        assert_eq!(
            urls[0],
            "https://www.modelscope.cn/models/o/repo/resolve/master/llama-server-b11408/llama-server-macos-arm64.tar.gz"
        );
        // GitHub 资产平铺：仅取 basename
        assert_eq!(
            urls[1],
            "https://github.com/o/repo/releases/latest/download/llama-server-macos-arm64.tar.gz"
        );
    }

    #[test]
    fn candidate_urls_skip_empty_source() {
        let src = DownloadSource::default();
        assert!(candidate_urls(&src, "a.gguf").is_empty());
        assert!(src.is_empty());
        let only_gh = DownloadSource {
            modelscope_repo: String::new(),
            github_release_base: "https://github.com/o/r/releases".into(),
        };
        assert_eq!(candidate_urls(&only_gh, "a.gguf").len(), 1);
    }

    /// 本地 HTTP 服务模拟：200/206/坏 sha 三种形态，验证续传与换源。
    #[tokio::test]
    async fn download_resume_and_fallback() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let payload: Vec<u8> = (0u32..64_000).map(|i| (i % 251) as u8).collect();
        use sha2::{Digest, Sha256};
        let sha = hex::encode(Sha256::digest(&payload));
        let entry = ManifestFile {
            name: "model.gguf".into(),
            size: payload.len() as u64,
            sha256: sha.clone(),
            kind: Some(super::super::manifest::AssetKind::Gguf),
            quant: Some("q5_k_m".into()),
            platform: None,
        };

        async fn serve(
            payload: Vec<u8>,
            mode: &'static str,
        ) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
            use tokio::net::TcpListener;
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
            let addr = listener.local_addr().expect("addr");
            let handle = tokio::spawn(async move {
                loop {
                    let Ok((mut sock, _)) = listener.accept().await else {
                        break;
                    };
                    let payload = payload.clone();
                    tokio::spawn(async move {
                        // 读全请求头（TCP 分段下一次 read 可能只收到部分）
                        let mut head: Vec<u8> = Vec::new();
                        let mut buf = [0u8; 1024];
                        loop {
                            match sock.read(&mut buf).await {
                                Ok(0) | Err(_) => break,
                                Ok(n) => {
                                    head.extend_from_slice(&buf[..n]);
                                    if head.windows(4).any(|w| w == b"\r\n\r\n") {
                                        break;
                                    }
                                }
                            }
                        }
                        let req = String::from_utf8_lossy(&head).to_string();
                        let range = req
                            .lines()
                            .find(|l| l.to_lowercase().starts_with("range:"))
                            .and_then(|l| l.split_once(':'))
                            .and_then(|(_, v)| v.trim().strip_prefix("bytes="))
                            .and_then(|v| v.split('-').next())
                            .and_then(|s| s.parse::<u64>().ok());
                        let head_body = match (mode, range) {
                            ("206", Some(start)) => {
                                let body = &payload[start as usize..];
                                format!(
                                    "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\n\r\n",
                                    body.len()
                                )
                                .into_bytes()
                                .into_iter()
                                .chain(body.iter().copied())
                                .collect::<Vec<u8>>()
                            }
                            ("200", _) => format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                                payload.len()
                            )
                            .into_bytes()
                            .into_iter()
                            .chain(payload.iter().copied())
                            .collect::<Vec<u8>>(),
                            ("404", _) => {
                                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec()
                            }
                            _ => format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                                payload.len()
                            )
                            .into_bytes()
                            .into_iter()
                            .chain(payload.iter().copied())
                            .collect::<Vec<u8>>(),
                        };
                        let _ = sock.write_all(&head_body).await;
                        let _ = sock.flush().await;
                    });
                }
            });
            (addr, handle)
        }

        let dir = tempfile::tempdir().expect("临时目录");
        let dest = dir.path().join("model.gguf");
        let dl = Downloader::new();

        // 1) 206 续传：预置前缀，下载补齐并校验通过
        std::fs::write(&dest, &payload[..20_000]).expect("写前缀");
        let (addr, h) = serve(payload.clone(), "206").await;
        let urls = vec![format!("http://{addr}/model.gguf")];
        let got = dl
            .download_file(&urls, &dest, &entry, |_| {})
            .await
            .expect("续传下载成功");
        assert_eq!(got, payload.len() as u64);
        assert_eq!(std::fs::read(&dest).expect("读回"), payload, "内容一致");

        // 2) 200 不支持 Range：从头重下也能成功
        std::fs::write(&dest, &payload[..5_000]).expect("写脏前缀");
        let (addr, h2) = serve(payload.clone(), "200").await;
        let urls = vec![format!("http://{addr}/model.gguf")];
        dl.download_file(&urls, &dest, &entry, |_| {})
            .await
            .expect("200 全量重下成功");
        assert_eq!(std::fs::read(&dest).expect("读回"), payload);

        // 3) 首源 404 → 次源成功（双源 fallback）
        std::fs::remove_file(&dest).ok();
        let (bad, hb) = serve(vec![], "404").await;
        let (good, hg) = serve(payload.clone(), "206").await;
        let urls = vec![format!("http://{bad}/m"), format!("http://{good}/m")];
        dl.download_file(&urls, &dest, &entry, |_| {})
            .await
            .expect("备源兜底成功");
        assert_eq!(std::fs::read(&dest).expect("读回"), payload);

        // 4) sha256 不符 → 报错且删除残档
        let wrong_entry = ManifestFile {
            sha256: "0".repeat(64),
            ..entry.clone()
        };
        let (addr, hw) = serve(payload.clone(), "206").await;
        let urls = vec![format!("http://{addr}/m")];
        assert!(
            dl.download_file(&urls, &dest, &wrong_entry, |_| {})
                .await
                .is_err()
        );
        assert!(!dest.exists(), "校验失败后残档应被删除");

        for h in [h, h2, hb, hg, hw] {
            h.abort();
        }
    }
}
