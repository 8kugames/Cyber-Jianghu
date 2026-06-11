// ============================================================================
// 模型下载（reqwest + SHA256 校验）
// ============================================================================
// 从 HuggingFace Hub 下载 bge-small-zh-v1.5 模型文件
// 优先 reqwest（项目已有，musl 兼容），无 hf-hub 依赖
// ============================================================================

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::Path;

/// bge-small-zh-v1.5 需要下载的文件列表
const MODEL_FILES: &[&str] = &[
    "config.json",
    "tokenizer.json",
    "model.safetensors",
];

/// HuggingFace 仓库 ID
const REPO_ID: &str = "BAAI/bge-small-zh-v1.5";

/// 构建文件下载 URL
fn file_url(mirror: &str, file: &str) -> String {
    format!("{}/{}/resolve/main/{}", mirror, REPO_ID, file)
}

/// 构建 SHA256 校验文件 URL
fn sha256_url(mirror: &str, file: &str) -> String {
    format!("{}.sha256", file_url(mirror, file))
}

/// 下载并校验单个文件
async fn download_file(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    expected_sha256: Option<&str>,
) -> Result<()> {
    tracing::info!("下载: {}", url);

    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("下载失败: {}", url))?;

    if !resp.status().is_success() {
        anyhow::bail!("下载失败 HTTP {}: {}", resp.status(), url);
    }

    let bytes = resp
        .bytes()
        .await
        .with_context(|| format!("读取响应体失败: {}", url))?;

    // SHA256 校验
    if let Some(expected) = expected_sha256 {
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let actual = hex::encode(hasher.finalize());
        if actual != expected {
            anyhow::bail!(
                "SHA256 校验失败: {} (期望: {}, 实际: {})",
                dest.display(),
                expected,
                actual
            );
        }
        tracing::info!("SHA256 校验通过: {}", dest.display());
    }

    // 写入文件
    std::fs::write(dest, &bytes)
        .with_context(|| format!("写入文件失败: {}", dest.display()))?;

    Ok(())
}

/// 从 HuggingF 下载 SHA256 校验值
///
/// HuggingF Hub 为每个文件提供 .sha256 伴随文件
/// 格式: "<hash>  <filename>"
async fn fetch_sha256(client: &reqwest::Client, url: &str) -> Option<String> {
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let text = resp.text().await.ok()?;
    // 格式: "<hash>  <filename>" 或纯 hash
    text.split_whitespace()
        .next()
        .map(|h| h.to_lowercase())
}

/// 下载 bge-small-zh-v1.5 模型到指定目录
///
/// 使用 reqwest（项目已有依赖，musl 兼容）下载，SHA256 校验
pub async fn download_model(model_dir: &Path, mirror: &str) -> Result<()> {
    // 创建目录
    std::fs::create_dir_all(model_dir)
        .with_context(|| format!("创建模型目录失败: {}", model_dir.display()))?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .context("创建 HTTP 客户端失败")?;

    for file in MODEL_FILES {
        let url = file_url(mirror, file);
        let dest = model_dir.join(file);

        // 跳过已存在的文件
        if dest.exists() {
            tracing::info!("文件已存在，跳过: {}", dest.display());
            continue;
        }

        let sha256_file_url = sha256_url(mirror, file);
        let expected_sha256 = fetch_sha256(&client, &sha256_file_url).await;

        if expected_sha256.is_some() {
            tracing::info!("获取到 SHA256 校验值: {}", file);
        } else {
            // 部分 HF 镜像不提供 .sha256 文件，此处为已知降级路径
            tracing::warn!("未获取到 SHA256 校验值，跳过校验（已知：部分镜像不提供 .sha256）: {}", file);
        }

        download_file(&client, &url, &dest, expected_sha256.as_deref()).await?;
    }

    tracing::info!("模型下载完成: {}", model_dir.display());
    Ok(())
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_url_format() {
        let url = file_url("https://hf-mirror.com", "config.json");
        assert_eq!(
            url,
            "https://hf-mirror.com/BAAI/bge-small-zh-v1.5/resolve/main/config.json"
        );
    }

    #[test]
    fn test_sha256_url_format() {
        let url = sha256_url("https://hf-mirror.com", "model.safetensors");
        assert_eq!(
            url,
            "https://hf-mirror.com/BAAI/bge-small-zh-v1.5/resolve/main/model.safetensors.sha256"
        );
    }
}
