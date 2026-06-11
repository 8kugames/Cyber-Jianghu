// ============================================================================
// 本地嵌入模型 (bge-small-zh-v1.5)
// ============================================================================
// 使用 candle-transformers 进行本地向量嵌入
// 模型: BAAI/bge-small-zh-v1.5 (512 维, ~100MB)
// ============================================================================

use anyhow::{Context, Result};
use candle_core::{Device, Tensor};
use candle_transformers::models::bert::{BertModel, Config};
use std::path::PathBuf;
use tokenizers::Tokenizer;

/// 本地嵌入器配置
#[derive(Debug, Clone)]
pub struct LocalEmbedderConfig {
    /// 模型目录
    pub model_dir: PathBuf,
    /// 输出维度（验证用）
    pub expected_dim: usize,
}

impl LocalEmbedderConfig {
    /// 使用默认路径创建配置
    ///
    /// 默认路径: {data_base_dir}/models/BAAI/bge-small-zh-v1.5
    pub fn default_path() -> Self {
        Self {
            model_dir: default_data_dir()
                .join("models")
                .join("BAAI")
                .join("bge-small-zh-v1.5"),
            expected_dim: 512,
        }
    }

    /// 从指定目录创建配置
    pub fn from_dir(model_dir: PathBuf) -> Self {
        Self {
            model_dir,
            expected_dim: 512,
        }
    }

    /// 检查模型文件是否齐全
    pub fn is_model_available(&self) -> bool {
        self.model_dir.join("config.json").exists()
            && self.model_dir.join("model.safetensors").exists()
            && self.model_dir.join("tokenizer.json").exists()
    }
}

impl Default for LocalEmbedderConfig {
    fn default() -> Self {
        Self::default_path()
    }
}

/// 获取默认数据目录
///
/// 优先级: CYBER_JIANGHU_DATA_DIR 环境变量 > ~/.cyber-jianghu
fn default_data_dir() -> PathBuf {
    std::env::var("CYBER_JIANGHU_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("cyber-jianghu")
        })
}

/// 本地嵌入器
///
/// 使用 bge-small-zh-v1.5 模型进行文本向量化
pub struct LocalEmbedder {
    /// BERT 模型
    model: BertModel,
    /// 分词器
    tokenizer: Tokenizer,
    /// 期望的输出维度
    expected_dim: usize,
    /// 设备（CPU 或 GPU）
    device: Device,
}

impl LocalEmbedder {
    /// 使用默认配置加载
    pub fn load() -> Result<Self> {
        Self::load_with_config(LocalEmbedderConfig::default())
    }

    /// 使用自定义配置加载
    pub fn load_with_config(config: LocalEmbedderConfig) -> Result<Self> {
        // 选择设备
        let device = if candle_core::utils::cuda_is_available() {
            Device::new_cuda(0).context("Failed to create CUDA device")?
        } else if candle_core::utils::metal_is_available() {
            Device::new_metal(0).context("Failed to create Metal device")?
        } else {
            Device::Cpu
        };

        // 加载配置
        let config_path = config.model_dir.join("config.json");
        let config_content = std::fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read config from {:?}", config_path))?;
        let bert_config: Config =
            serde_json::from_str(&config_content).context("Failed to parse BERT config")?;

        // 加载模型权重
        let weights_path = config.model_dir.join("model.safetensors");
        let vb = unsafe {
            candle_nn::VarBuilder::from_mmaped_safetensors(
                &[&weights_path],
                candle_core::DType::F32,
                &device,
            )
            .context("Failed to load model weights")?
        };

        // 创建模型
        let model = BertModel::load(vb, &bert_config).context("Failed to create BERT model")?;

        // 加载分词器
        let tokenizer_path = config.model_dir.join("tokenizer.json");
        let tokenizer = Tokenizer::from_file(&tokenizer_path).map_err(|e| {
            anyhow::anyhow!("Failed to load tokenizer from {:?}: {}", tokenizer_path, e)
        })?;

        Ok(Self {
            model,
            tokenizer,
            expected_dim: config.expected_dim,
            device,
        })
    }

    /// 对单个文本进行嵌入
    pub fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let embeddings = self.embed_batch(&[text])?;
        embeddings
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No embedding returned"))
    }

    /// 批量嵌入
    pub fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let mut results = Vec::with_capacity(texts.len());

        for text in texts {
            // 分词
            let encoding = self
                .tokenizer
                .encode(*text, true)
                .map_err(|e| anyhow::anyhow!("Tokenization failed: {}", e))?;

            let input_ids = encoding.get_ids();
            let attention_mask = encoding.get_attention_mask();

            // 转换为 Tensor
            let input_ids_tensor = Tensor::new(
                input_ids.iter().map(|&id| id as i64).collect::<Vec<_>>(),
                &self.device,
            )?
            .unsqueeze(0)?;

            let attention_mask_tensor = Tensor::new(
                attention_mask.iter().map(|&m| m as f32).collect::<Vec<_>>(),
                &self.device,
            )?
            .unsqueeze(0)?;

            // 创建 token_type_ids（单句输入全零）
            let token_type_ids = Tensor::zeros(
                input_ids_tensor.shape().dims(),
                input_ids_tensor.dtype(),
                &self.device,
            )?;

            // 模型推理
            let embeddings = self.model.forward(
                &input_ids_tensor,
                &token_type_ids,
                Some(&attention_mask_tensor),
            )?;

            // BGE 模型使用 [CLS] token 作为句子表示
            let cls_embedding = embeddings.get(0)?.get(0)?;

            // 转换为 Vec<f32>
            let embedding_vec = cls_embedding.to_vec1::<f32>()?;

            // 验证维度
            if embedding_vec.len() != self.expected_dim {
                anyhow::bail!(
                    "Embedding dimension mismatch: expected {}, got {}",
                    self.expected_dim,
                    embedding_vec.len()
                );
            }

            // L2 归一化
            let normalized = Self::l2_normalize(&embedding_vec);
            results.push(normalized);
        }

        Ok(results)
    }

    /// L2 归一化
    fn l2_normalize(vec: &[f32]) -> Vec<f32> {
        let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm < 1e-10 {
            return vec.to_vec();
        }
        vec.iter().map(|x| x / norm).collect()
    }

    /// 获取嵌入维度
    pub fn embedding_dim(&self) -> usize {
        self.expected_dim
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default_path() {
        let config = LocalEmbedderConfig::default();
        assert!(
            config.model_dir.ends_with("bge-small-zh-v1.5"),
            "model_dir 应以 bge-small-zh-v1.5 结尾, 实际: {}",
            config.model_dir.display()
        );
    }

    #[test]
    fn test_config_is_model_available_missing() {
        let config = LocalEmbedderConfig::from_dir(PathBuf::from("/nonexistent"));
        assert!(!config.is_model_available());
    }
}
