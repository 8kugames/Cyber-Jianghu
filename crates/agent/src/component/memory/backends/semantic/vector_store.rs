// ============================================================================
// HNSW 向量存储
// ============================================================================
// 设计文档: (项目根)/docs/superpowers/specs/2025-03-15-semantic-memory-and-forgetting-design.md
//
// 使用 instant-distance 实现 HNSW 近似最近邻搜索
// ============================================================================

use anyhow::{Context, Result};
use rusqlite::{Connection, Row};
use std::cell::Cell;
use std::collections::HashMap;

/// 向量点（用于 instant-distance）
#[derive(Clone, Debug)]
pub struct VectorPoint {
    /// 数据库 ID
    pub id: i64,
    /// 向量数据
    pub vector: Vec<f32>,
}

impl instant_distance::Point for VectorPoint {
    fn distance(&self, other: &Self) -> f32 {
        // 使用余弦距离 (1.0 - cosine_similarity)
        // 注意：instant-distance 需要距离越小越相似
        // instant-distance 的 Cosine metric 已经处理了这部分逻辑，但 Point trait 需要实现 distance
        // 这里我们手动实现余弦距离

        let dot_product: f32 = self
            .vector
            .iter()
            .zip(&other.vector)
            .map(|(a, b)| a * b)
            .sum();
        let norm_a: f32 = self.vector.iter().map(|a| a * a).sum::<f32>().sqrt();
        let norm_b: f32 = other.vector.iter().map(|b| b * b).sum::<f32>().sqrt();

        if norm_a == 0.0 || norm_b == 0.0 {
            return 1.0; // 最大距离
        }

        1.0 - (dot_product / (norm_a * norm_b))
    }
}

/// HNSW 向量存储
///
/// 内存中的 HNSW 索引，支持快速向量检索
/// 采用延迟重建策略：add()只更新points，search()时才重建索引
pub struct HnswVectorStore {
    /// HNSW 索引
    index: Option<instant_distance::HnswMap<VectorPoint, ()>>,
    /// 数据库 ID 到向量点的映射
    points: HashMap<i64, VectorPoint>,
    /// 向量维度
    dimension: usize,
    /// 索引是否需要重建（add/remove后标记），使用 Cell 实现内部可变性
    needs_rebuild: Cell<bool>,
}

impl HnswVectorStore {
    /// 创建新的空向量存储
    pub fn new(dimension: usize) -> Self {
        Self {
            index: None,
            points: HashMap::new(),
            dimension,
            needs_rebuild: Cell::new(false),
        }
    }

    /// 从数据库加载现有向量
    pub fn load_from_db(&mut self, conn: &Connection) -> Result<()> {
        let mut stmt = conn
            .prepare(
                "SELECT id, embedding FROM client_memories
                 WHERE embedding IS NOT NULL AND is_archived = FALSE
                 ORDER BY id",
            )
            .context("Failed to prepare query")?;

        let rows = stmt
            .query_map([], |row: &Row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Option<Vec<u8>>>(1)?))
            })
            .context("Failed to execute query")?;

        let mut points = Vec::new();

        for row_result in rows {
            let (id, blob) = row_result?;
            let Some(blob) = blob else { continue };
            let vector = self.decode_vector(&blob)?;

            if vector.len() != self.dimension {
                tracing::warn!(
                    "Skipping memory {} with wrong dimension: {} (expected {})",
                    id,
                    vector.len(),
                    self.dimension
                );
                continue;
            }

            let point = VectorPoint { id, vector };
            self.points.insert(id, point.clone());
            points.push(point);
        }

        if points.is_empty() {
            self.index = None;
            tracing::info!("No vectors found in database");
            return Ok(());
        }

        // 构建 HNSW 索引
        let hnsw = instant_distance::Builder::default().build(points, vec![(); self.points.len()]);

        self.index = Some(hnsw);
        self.needs_rebuild.set(false);
        tracing::info!("Loaded {} vectors from database", self.points.len());

        Ok(())
    }

    /// 重建索引（add/remove后标记需要重建，search时自动触发）
    pub fn rebuild_index(&mut self) {
        if self.points.is_empty() {
            self.index = None;
            self.needs_rebuild.set(false);
            return;
        }

        let points: Vec<VectorPoint> = self.points.values().cloned().collect();
        let hnsw = instant_distance::Builder::default().build(points, vec![(); self.points.len()]);
        self.index = Some(hnsw);
        self.needs_rebuild.set(false);
    }

    /// 添加新向量（延迟重建策略：只更新points，search时才重建索引）
    pub fn add(&mut self, id: i64, vector: Vec<f32>) -> Result<()> {
        if vector.len() != self.dimension {
            anyhow::bail!(
                "Vector dimension mismatch: {} (expected {})",
                vector.len(),
                self.dimension
            );
        }

        if self.points.contains_key(&id) {
            tracing::warn!("Vector {} already exists, updating", id);
        }

        let point = VectorPoint { id, vector };
        self.points.insert(id, point);
        self.needs_rebuild.set(true);

        tracing::debug!(
            "Added new vector to HNSW store, total points: {} (rebuild pending)",
            self.points.len()
        );
        Ok(())
    }

    /// 搜索最近邻（触发延迟索引重建）
    pub fn search(&mut self, query_vector: &[f32], limit: usize) -> Result<Vec<(i64, f32)>> {
        if self.needs_rebuild.get() {
            self.rebuild_index();
        }

        let Some(index) = &self.index else {
            return Ok(Vec::new());
        };

        if query_vector.len() != self.dimension {
            anyhow::bail!(
                "Query vector dimension mismatch: {} (expected {})",
                query_vector.len(),
                self.dimension
            );
        }

        let query_point = VectorPoint {
            id: 0,
            vector: query_vector.to_vec(),
        };

        let mut search = instant_distance::Search::default();
        let results = index.search(&query_point, &mut search);

        let mut final_results = Vec::new();
        for item in results.take(limit) {
            final_results.push((item.point.id, item.distance));
        }

        Ok(final_results)
    }

    /// 移除向量
    pub fn remove(&mut self, id: i64) -> bool {
        let removed = self.points.remove(&id).is_some();
        if removed {
            self.needs_rebuild.set(true);
        }
        removed
    }

    /// 获取向量数量
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// 检查是否为空
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// 清空索引
    pub fn clear(&mut self) {
        self.index = None;
        self.points.clear();
        self.needs_rebuild.set(false);
    }

    /// 检查索引是否需要重建（供外部调用，无需 &mut）
    pub fn needs_rebuild(&self) -> bool {
        self.needs_rebuild.get()
    }

    /// 将向量编码为 BLOB
    pub fn encode_vector(vector: &[f32]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(vector.len() * 4);
        for &v in vector {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        bytes
    }

    /// 解码二进制向量
    fn decode_vector(&self, blob: &[u8]) -> Result<Vec<f32>> {
        if !blob.len().is_multiple_of(4) {
            return Err(anyhow::anyhow!("Invalid blob length: {}", blob.len()));
        }

        let count = blob.len() / 4;
        let mut vector = Vec::with_capacity(count);

        for chunk in blob.chunks(4) {
            let bytes: [u8; 4] = chunk.try_into().context("Invalid chunk")?;
            vector.push(f32::from_le_bytes(bytes));
        }

        Ok(vector)
    }

    /// 获取向量维度
    pub fn dimension(&self) -> usize {
        self.dimension
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode_vector() {
        let original = vec![0.1_f32, 0.2, 0.3, 0.4, 0.5];
        let encoded = HnswVectorStore::encode_vector(&original);
        let store = HnswVectorStore::new(5);
        let decoded = store.decode_vector(&encoded).unwrap();

        assert_eq!(original.len(), decoded.len());
        for (a, b) in original.iter().zip(decoded.iter()) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn test_new_store_is_empty() {
        let store = HnswVectorStore::new(512);
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn test_add_and_len() {
        let mut store = HnswVectorStore::new(3);
        store.add(1, vec![0.1, 0.2, 0.3]).unwrap();
        store.add(2, vec![0.4, 0.5, 0.6]).unwrap();

        assert_eq!(store.len(), 2);
        assert!(!store.is_empty());
    }

    #[test]
    fn test_remove() {
        let mut store = HnswVectorStore::new(3);
        store.add(1, vec![0.1, 0.2, 0.3]).unwrap();

        assert!(store.remove(1));
        assert!(store.is_empty());
        assert!(!store.remove(999)); // 不存在的 ID
    }

    #[test]
    fn test_dimension_mismatch() {
        let mut store = HnswVectorStore::new(512);
        let result = store.add(1, vec![0.1; 128]);
        assert!(result.is_err());
    }

    #[test]
    fn test_rebuild_and_search() {
        let mut store = HnswVectorStore::new(3);

        // 添加几个向量
        store.add(1, vec![1.0, 0.0, 0.0]).unwrap();
        store.add(2, vec![0.0, 1.0, 0.0]).unwrap();
        store.add(3, vec![0.0, 0.0, 1.0]).unwrap();

        // 重建索引
        store.rebuild_index();

        // 搜索与 [1, 0, 0] 最相似的向量
        let results = store.search(&[1.0, 0.0, 0.0], 2).unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].0, 1); // 第一个应该是 ID=1
    }
}
