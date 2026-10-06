// ============================================================================
// 决策模型资产清单（manifest.json）解析与 sha256 校验
// ============================================================================
//
// manifest.json 与模型文件同仓分发（ModelScope / GitHub Release 双源），
// 描述一个模型版本的文件集合：每个文件的名字、字节数、sha256、类别。
// 下载完成后逐文件校验 sha256，任一不符即整体判失败（对齐自更新模块
// "缺 digest 拒装"的 fail-safe 风格）。

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

/// 资产类别：gguf 权重 / 决策配置（letter token ids 与温度）/ llama-server
/// 推理运行时（随权重同仓分发，按平台自动下载）。
/// manifest.json 自身不列入 files（下载即得，无需自校验）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Gguf,
    DecisionConfig,
    LlamaServer,
}

/// 清单中的单个文件条目
///
/// schema 宽容兼容两种发布形态：
/// - 字节数键名 `size` 或 `bytes`（serde alias）；
/// - `kind`/`quant`/`platform` 可省略，按文件名推断（`*.gguf` → Gguf 且从文件名
///   尾段取档位，如 `-Q5_K_M.gguf` → `q5_k_m`；`decision_config.json` →
///   DecisionConfig；`llama-server-{platform}.tar.gz` → LlamaServer）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestFile {
    /// 文件名（相对版本子目录）
    pub name: String,
    /// 字节数
    #[serde(alias = "bytes")]
    pub size: u64,
    /// sha256 十六进制小写
    pub sha256: String,
    /// 资产类别（None = 解析后按文件名推断）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<AssetKind>,
    /// gguf 专属：量化档位（q8_0/q6_k/q5_k_m/q4_k_s，小写；None = gguf 按文件名推断）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quant: Option<String>,
    /// llama-server 专属：目标平台（如 "macos-arm64"/"linux-x86_64"/"windows-x86_64"；
    /// None = llama-server 按文件名推断）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
}

/// llama-server 分发文件名前缀（推断与查找的锚点）
pub const LLAMA_SERVER_NAME_PREFIX: &str = "llama-server-";

/// manifest 文件名的安全相对路径判定：允许子目录（"runtime-b11408/x.tar.gz"），
/// 拒绝绝对路径、`..`/`.` 分量、空段、反斜杠与冒号（盘符/NTFS 数据流）
fn is_safe_rel_path(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && !name.contains('\\')
        && !name.contains(':')
        && name
            .split('/')
            .all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

impl ManifestFile {
    /// 类别/档位/平台归一：显式声明优先，缺省按文件名推断；无法判定的条目报错
    /// （fail-fast，避免下载了用不上的文件或静默丢档位）。
    fn resolve_inferred(self) -> Result<Self> {
        let name = self.name.clone();
        // 类别推断一律看 basename（允许 "llama-server-b11408/xxx.tar.gz" 形态的
        // 子目录名；目录前缀随构建号演化，不参与类别判定）
        let basename = name.rsplit('/').next().unwrap_or(&name).to_string();
        let kind = match self.kind {
            Some(k) => k,
            None if basename.ends_with(".gguf") => AssetKind::Gguf,
            None if basename == "decision_config.json" => AssetKind::DecisionConfig,
            None if basename.starts_with(LLAMA_SERVER_NAME_PREFIX) && name.ends_with(".tar.gz") => {
                AssetKind::LlamaServer
            }
            None => bail!("manifest 文件 {name} 类别不明（kind 缺省且无法从文件名推断）"),
        };
        // quant / platform 按类别归一（互斥字段：gguf 只带 quant，
        // llama-server 只带 platform；显式声明优先，缺省按文件名推断）
        let (quant, platform): (Option<String>, Option<String>) = match kind {
            AssetKind::DecisionConfig => {
                if self.quant.is_some() {
                    bail!("manifest 文件 {name} 为决策配置，不应携带 quant");
                }
                if self.platform.is_some() {
                    bail!("manifest 文件 {name} 为决策配置，不应携带 platform");
                }
                (None, None)
            }
            AssetKind::Gguf => {
                if self.platform.is_some() {
                    bail!("manifest 文件 {name} 为 gguf 权重，不应携带 platform");
                }
                let q = match self.quant {
                    Some(q) => q.to_ascii_lowercase(),
                    None => {
                        let stem = basename.strip_suffix(".gguf").unwrap_or(&basename);
                        let tail = stem.rsplit('-').next().unwrap_or_default();
                        let q = tail.to_ascii_lowercase();
                        if crate::config::DECISION_MODEL_QUANTS.contains(&q.as_str()) {
                            q
                        } else {
                            bail!("manifest 文件 {name} 无法从文件名推断量化档位（尾段 {tail:?}）")
                        }
                    }
                };
                (Some(q), None)
            }
            AssetKind::LlamaServer => {
                if self.quant.is_some() {
                    bail!("manifest 文件 {name} 为 llama-server，不应携带 quant");
                }
                // llama-server 资产统一为 tar.gz 运行时归档（launcher + 共享库，
                // 解压到版本目录）；平台取自文件名中段
                if !name.ends_with(".tar.gz") {
                    bail!("manifest 文件 {name} 为 llama-server，必须是 .tar.gz 运行时归档");
                }
                let p = match self.platform {
                    Some(p) => p,
                    None => {
                        // "llama-server-macos-arm64.tar.gz"（含子目录前缀时取 basename）
                        let stem = basename
                            .strip_prefix(LLAMA_SERVER_NAME_PREFIX)
                            .unwrap_or(&basename)
                            .strip_suffix(".tar.gz")
                            .unwrap_or(&basename);
                        if stem.is_empty() {
                            bail!("manifest 文件 {name} 无法从文件名推断平台")
                        }
                        stem.to_ascii_lowercase()
                    }
                };
                (None, Some(p))
            }
        };
        Ok(Self {
            name,
            size: self.size,
            sha256: self.sha256,
            kind: Some(kind),
            quant,
            platform,
        })
    }
}

/// 模型版本清单
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelManifest {
    /// 版本号（同时是版本子目录名，如 "model-v1"）
    pub version: String,
    /// 文件清单
    pub files: Vec<ManifestFile>,
}

impl ModelManifest {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let manifest: ModelManifest = serde_json::from_slice(bytes)
            .with_context(|| "manifest.json 解析失败（非合法 JSON 或 schema 不符）")?;
        // kind/quant/platform 逐条目归一（推断失败即整体拒收）
        let mut files = Vec::with_capacity(manifest.files.len());
        for f in manifest.files {
            files.push(f.resolve_inferred()?);
        }
        let manifest = ModelManifest {
            version: manifest.version,
            files,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    /// 从版本目录加载本地 manifest.json
    pub fn load_from_dir(dir: &Path) -> Result<Self> {
        let path = dir.join("manifest.json");
        let bytes = std::fs::read(&path)
            .with_context(|| format!("读取 manifest.json 失败: {}", path.display()))?;
        Self::parse(&bytes)
    }

    fn validate(&self) -> Result<()> {
        if self.version.trim().is_empty() {
            bail!("manifest.version 不能为空");
        }
        if self.version.contains("..") || self.version.contains('/') {
            bail!("manifest.version 含非法路径字符: {}", self.version);
        }
        if self.files.is_empty() {
            bail!("manifest.files 不能为空");
        }
        let mut seen = std::collections::HashSet::new();
        for f in &self.files {
            if !is_safe_rel_path(&f.name) {
                bail!("manifest 文件名含非法路径字符: {}", f.name);
            }
            if f.sha256.len() != 64 || !f.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
                bail!("manifest 文件 {} 的 sha256 不是 64 位十六进制", f.name);
            }
            if !seen.insert(f.name.clone()) {
                bail!("manifest 文件名重复: {}", f.name);
            }
        }
        Ok(())
    }

    /// 找到指定量化档位的 gguf 条目（quant 小写匹配）
    pub fn gguf_for_quant(&self, quant: &str) -> Option<&ManifestFile> {
        let quant = quant.to_ascii_lowercase();
        self.files
            .iter()
            .find(|f| f.kind == Some(AssetKind::Gguf) && f.quant.as_deref() == Some(quant.as_str()))
    }

    /// 决策配置文件条目（decision_config.json）
    pub fn decision_config(&self) -> Option<&ManifestFile> {
        self.files
            .iter()
            .find(|f| f.kind == Some(AssetKind::DecisionConfig))
    }

    /// 本平台 llama-server 条目（platform 小写匹配，如 "macos-arm64"）
    pub fn llama_server_for_platform(&self, platform: &str) -> Option<&ManifestFile> {
        let platform = platform.to_ascii_lowercase();
        self.files.iter().find(|f| {
            f.kind == Some(AssetKind::LlamaServer)
                && f.platform.as_deref() == Some(platform.as_str())
        })
    }

    /// 本版本需要的全部文件条目（给定量化档位 + 本平台推理运行时）
    pub fn required_files(&self, quant: &str, platform: &str) -> Vec<&ManifestFile> {
        let mut out: Vec<&ManifestFile> = Vec::new();
        if let Some(cfg) = self.decision_config() {
            out.push(cfg);
        }
        if let Some(gguf) = self.gguf_for_quant(quant) {
            out.push(gguf);
        }
        if let Some(bin) = self.llama_server_for_platform(platform) {
            out.push(bin);
        }
        out
    }
}

/// 校验本地文件 sha256 是否与清单一致（流式读取，GB 级权重不整载内存；
/// 不一致删除损坏文件并返回 Err）
pub fn verify_file_sha256(path: &Path, expected_sha256: &str) -> Result<()> {
    use std::io::Read;
    let expected = expected_sha256.trim().to_ascii_lowercase();
    if expected.is_empty() {
        return Ok(()); // 无清单摘要的条目（manifest.json 自身）跳过校验
    }
    let file = std::fs::File::open(path)
        .with_context(|| format!("打开待校验文件失败: {}", path.display()))?;
    let mut reader = std::io::BufReader::with_capacity(1024 * 1024, file);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader
            .read(&mut buf)
            .with_context(|| format!("读取待校验文件失败: {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual = hex::encode(hasher.finalize());
    if actual != expected {
        std::fs::remove_file(path).ok();
        bail!(
            "sha256 校验不符: {} 期望 {expected} 实际 {actual}（已删除损坏文件）",
            path.display()
        );
    }
    Ok(())
}

/// 增量流式 sha256 状态（下载时边收边算，避免二次读盘）
#[derive(Debug)]
pub struct Sha256Stream {
    hasher: Sha256,
    count: u64,
}

impl Default for Sha256Stream {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256Stream {
    pub fn new() -> Self {
        Self {
            hasher: Sha256::new(),
            count: 0,
        }
    }

    pub fn update(&mut self, chunk: &[u8]) {
        self.hasher.update(chunk);
        self.count += chunk.len() as u64;
    }

    pub fn finalize_hex(self) -> String {
        hex::encode(self.hasher.finalize())
    }

    pub fn bytes_written(&self) -> u64 {
        self.count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;

    fn sample_manifest_json() -> &'static str {
        r#"{
          "version": "model-v1",
          "files": [
            {"name": "decision_config.json", "size": 476, "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "kind": "decision_config"},
            {"name": "decision-2b-Q5_K_M.gguf", "size": 2480000000, "sha256": "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB", "kind": "gguf", "quant": "q5_k_m"},
            {"name": "decision-2b-Q4_K_S.gguf", "size": 1900000000, "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc", "kind": "gguf", "quant": "q4_k_s"}
          ]
        }"#
    }

    #[test]
    fn parse_ok_and_quant_lookup() {
        let m = ModelManifest::parse(sample_manifest_json().as_bytes()).expect("合法清单");
        assert_eq!(m.version, "model-v1");
        let gguf = m.gguf_for_quant("q5_k_m").expect("应有 q5_k_m");
        assert_eq!(gguf.name, "decision-2b-Q5_K_M.gguf");
        assert!(m.gguf_for_quant("q3_k_l").is_none(), "未列出的档位不存在");
        assert_eq!(
            m.decision_config().map(|f| f.name.as_str()),
            Some("decision_config.json")
        );
        // 无 llama-server 条目的清单：required_files 只含配置 + 权重
        assert_eq!(m.required_files("q4_k_s", "macos-arm64").len(), 2);
    }

    #[test]
    fn parse_rejects_bad_fields() {
        let bad = r#"{"version": "", "files": [{"name": "a.gguf", "size": 1, "sha256": "aa", "kind": "gguf"}]}"#;
        assert!(
            ModelManifest::parse(bad.as_bytes()).is_err(),
            "空版本号拒绝"
        );
        let bad2 = r#"{"version": "model-v1", "files": []}"#;
        assert!(
            ModelManifest::parse(bad2.as_bytes()).is_err(),
            "空文件清单拒绝"
        );
        let bad3 = r#"{"version": "model-v1", "files": [{"name": "a.gguf", "size": 1, "sha256": "zz", "kind": "gguf"}]}"#;
        assert!(
            ModelManifest::parse(bad3.as_bytes()).is_err(),
            "非法 sha256 拒绝"
        );
        let bad4 = r#"{"version": "../evil", "files": [{"name": "a.gguf", "size": 1, "sha256": "aa", "kind": "gguf"}]}"#;
        assert!(
            ModelManifest::parse(bad4.as_bytes()).is_err(),
            "路径穿越版本号拒绝"
        );
        // 类别不明的文件名（kind 缺省且无法推断）
        let bad5 = r#"{"version": "model-v1", "files": [{"name": "readme.txt", "size": 1, "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}"#;
        assert!(
            ModelManifest::parse(bad5.as_bytes()).is_err(),
            "类别不明条目拒绝"
        );
        // gguf 文件名尾段不是已知档位
        let bad6 = r#"{"version": "model-v1", "files": [{"name": "model-Q9_X.gguf", "size": 1, "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}"#;
        assert!(
            ModelManifest::parse(bad6.as_bytes()).is_err(),
            "未知档位尾段拒绝"
        );
    }

    /// 黄金样本：ModelScope 仓线上 manifest 全量真身（2026-10-05 model-v1 发布，
    /// 子目录形态 llama_server 条目 + 真实 sha256）——解析失败会导致双源均不可
    /// 装，此测试防回归；更新线上清单时须同步本样本。
    #[test]
    fn golden_parses_production_modelscope_manifest() {
        let production = r#"{
          "model": "Cyber-Jianghu-Decision-2B",
          "display_name": "江湖策",
          "version": "model-v1",
          "license": "Apache-2.0",
          "base_model": "Qwen/Qwen3.5-2B",
          "files": [
            {
              "name": "Cyber-Jianghu-Decision-2B-Q5_K_M.gguf",
              "bytes": 1411120576,
              "sha256": "270d3f8c897fb9eb31abf83f3ff9ec82f3f5b9c199758f13d842439cd1350219"
            },
            {
              "name": "Cyber-Jianghu-Decision-2B-Q4_K_S.gguf",
              "bytes": 1212054976,
              "sha256": "aa731cc964b0df0c810eddb1da965a9170a0af54fe86e34544d581140e255c30"
            },
            {
              "name": "decision_config.json",
              "bytes": 476,
              "sha256": "16715d136dca492e754687545af385b527ea9e759e949aea7b8592d53b495280"
            },
            {
              "name": "llama-server-b11408/llama-server-macos-arm64.tar.gz",
              "size": 11642568,
              "sha256": "0c0ca702724a87b95acbb1156d66166414f3e5b3f186390bdd349de640f823e1",
              "kind": "llama_server",
              "platform": "macos-arm64"
            },
            {
              "name": "llama-server-b11408/llama-server-macos-x86_64.tar.gz",
              "size": 11197586,
              "sha256": "947eaa0c853b1d6d4b027c9b6b304df9d91060fd4a8bc088236d7f67f48ce3a4",
              "kind": "llama_server",
              "platform": "macos-x86_64"
            },
            {
              "name": "llama-server-b11408/llama-server-linux-x86_64.tar.gz",
              "size": 16914868,
              "sha256": "c173304dc6d86be7d9d907b0ca197526e233807ac383c1ed167353a537a4e632",
              "kind": "llama_server",
              "platform": "linux-x86_64"
            },
            {
              "name": "llama-server-b11408/llama-server-linux-arm64.tar.gz",
              "size": 12941263,
              "sha256": "39876da89c369092d87c2a67deff3e68cd0de9ed34421b25255ac70ef36b5504",
              "kind": "llama_server",
              "platform": "linux-arm64"
            },
            {
              "name": "llama-server-b11408/llama-server-windows-x86_64.tar.gz",
              "size": 19558153,
              "sha256": "194b8ee3ec1331c2255e8992703bdbffab7809c646eea8bc3de8593e53e86a78",
              "kind": "llama_server",
              "platform": "windows-x86_64"
            }
          ]
        }"#;
        let m = ModelManifest::parse(production.as_bytes()).expect("生产 manifest 必须可解析");
        assert_eq!(m.version, "model-v1");
        let q5 = m.gguf_for_quant("q5_k_m").expect("推断出 q5_k_m 档位");
        assert_eq!(q5.name, "Cyber-Jianghu-Decision-2B-Q5_K_M.gguf");
        assert_eq!(q5.size, 1411120576, "bytes 别名映射到 size");
        let cfg = m.decision_config().expect("推断出决策配置");
        assert_eq!(cfg.size, 476);
        assert!(cfg.quant.is_none(), "决策配置不携带档位");
        // llama-server：子目录名 + 显式 platform 匹配
        let mac = m
            .llama_server_for_platform("macos-arm64")
            .expect("macos-arm64 运行时条目");
        assert_eq!(
            mac.name,
            "llama-server-b11408/llama-server-macos-arm64.tar.gz"
        );
        assert_eq!(mac.platform.as_deref(), Some("macos-arm64"));
        assert_eq!(
            m.llama_server_for_platform("linux-x86_64")
                .expect("linux-x86_64 运行时条目")
                .size,
            16914868
        );
        // required_files = 决策配置 + 权重 + 本平台运行时归档 = 3
        assert_eq!(m.required_files("q4_k_s", "macos-arm64").len(), 3);
        assert_eq!(m.required_files("q5_k_m", "windows-x86_64").len(), 3);
        // 未发布平台干净返回 None（装配端告警降级）
        assert!(m.llama_server_for_platform("freebsd-x86_64").is_none());
        // 未发布的档位干净失败（调用方回退 LLM 路径并给出可用档位清单）
        assert!(m.gguf_for_quant("q8_0").is_none());
    }

    /// llama-server 归档：平台从文件名推断（无显式 platform 字段），查找大小写不敏感
    #[test]
    fn llama_server_archive_platform_inference() {
        let m = r#"{"version": "v", "files": [
            {"name": "llama-server-b11408/llama-server-macos-arm64.tar.gz", "size": 10, "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
            {"name": "llama-server-b11408/llama-server-windows-x86_64.tar.gz", "size": 10, "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}
        ]}"#;
        let manifest = ModelManifest::parse(m.as_bytes()).expect("归档条目可解析");
        let mac = manifest
            .llama_server_for_platform("MACOS-ARM64")
            .expect("大小写不敏感");
        assert_eq!(mac.platform.as_deref(), Some("macos-arm64"));
        assert!(
            manifest.llama_server_for_platform("linux-x86_64").is_none(),
            "未发布平台返回 None"
        );
    }

    /// llama-server 资产必须为 tar.gz 归档；裸二进制名 / 携带 quant 拒绝
    #[test]
    fn llama_server_rejects_non_archive() {
        let bad = r#"{"version": "v", "files": [
            {"name": "llama-server-macos-arm64", "size": 10, "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}
        ]}"#;
        assert!(
            ModelManifest::parse(bad.as_bytes()).is_err(),
            "裸二进制 llama-server 条目拒绝"
        );
        let bad2 = r#"{"version": "v", "files": [
            {"name": "llama-server-b11408/llama-server-macos-arm64.tar.gz", "size": 10, "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "quant": "q5_k_m"}
        ]}"#;
        assert!(
            ModelManifest::parse(bad2.as_bytes()).is_err(),
            "llama-server 携带 quant 拒绝"
        );
    }

    #[test]
    fn verify_file_sha256_roundtrip() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("f.bin");
        std::fs::write(&path, b"hello decision model").expect("写入");
        let expect = hex::encode(Sha256::digest(b"hello decision model"));
        assert!(verify_file_sha256(&path, &expect).is_ok());
        // 校验失败应删除损坏文件
        std::fs::write(&path, b"tampered").expect("覆写");
        assert!(verify_file_sha256(&path, &expect).is_err());
        assert!(!path.exists(), "校验失败后文件应被删除");
    }

    #[test]
    fn sha256_stream_matches_oneshot() {
        let mut st = Sha256Stream::new();
        st.update(b"hello ");
        st.update(b"world");
        assert_eq!(st.bytes_written(), 11);
        assert_eq!(
            st.finalize_hex(),
            hex::encode(Sha256::digest(b"hello world"))
        );
    }
}
