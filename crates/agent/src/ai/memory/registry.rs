// ============================================================================
// 全局记忆注册表
// ============================================================================
//
// 管理多个 Agent 的记忆数据库，支持跨 Agent 查询
// 用于 OpenClaw 获取历史上所有 Agent 的生平汇总
// ============================================================================

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use super::store::ClientMemory;

// ============================================================================
// Agent 元数据
// ============================================================================

/// Agent 生涯信息
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentLifetime {
    /// Agent ID
    pub agent_id: Uuid,
    /// 数据库文件路径
    pub db_path: PathBuf,
    /// 记忆总数
    pub memory_count: usize,
    /// 最早记忆时间（tick_id）
    pub earliest_tick: Option<i64>,
    /// 最晚记忆时间（tick_id）
    pub latest_tick: Option<i64>,
}

// ============================================================================
// 全局记忆报告
// ============================================================================

/// 全局记忆报告（包含所有 Agent 的历史）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GlobalMemoryReport {
    /// 生成时间
    pub generated_at: chrono::DateTime<chrono::Utc>,
    /// Agent 总数
    pub total_agents: usize,
    /// 记忆总数
    pub total_memories: usize,
    /// 所有 Agent 的生涯信息
    pub agents: Vec<AgentLifetime>,
    /// 所有 Agent 的重要记忆（按重要性排序）
    pub top_memories: Vec<ClientMemory>,
    /// 指定时间范围内的所有记忆
    pub memories_in_range: Vec<ClientMemory>,
}

// ============================================================================
// 全局记忆注册表
// ============================================================================

/// 全局记忆注册表
///
/// 扫描目录下的所有 agent 数据库，支持跨 Agent 查询
pub struct GlobalMemoryRegistry {
    /// 数据库目录
    db_dir: PathBuf,
    /// 已发现的 Agent 列表（缓存）
    agents: Vec<AgentLifetime>,
}

impl GlobalMemoryRegistry {
    /// 创建新的全局注册表
    pub fn new(db_dir: &Path) -> Self {
        Self {
            db_dir: db_dir.to_path_buf(),
            agents: Vec::new(),
        }
    }

    /// 使用默认目录创建
    pub fn with_default_dir() -> Self {
        let db_dir = Self::default_db_dir();
        Self::new(Path::new(&db_dir))
    }

    /// 扫描目录，发现所有 Agent 数据库
    pub fn scan(&mut self) -> Result<usize> {
        self.agents.clear();

        if !self.db_dir.exists() {
            return Ok(0);
        }

        let entries =
            std::fs::read_dir(&self.db_dir).context("Failed to read database directory")?;

        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(filename) = path.file_name() {
                let filename = filename.to_string_lossy();
                if filename.starts_with("agent_") && filename.ends_with(".db") {
                    // 从文件名提取 agent_id: agent_{uuid}.db
                    if let Some(agent_id_str) = filename
                        .strip_prefix("agent_")
                        .and_then(|s| s.strip_suffix(".db"))
                        && let Ok(agent_id) = Uuid::parse_str(agent_id_str)
                            && let Ok(lifetime) = self.inspect_agent_db(&path, agent_id) {
                                self.agents.push(lifetime);
                            }
                }
            }
        }

        // 按 earliest_tick 排序（最早的在前）
        self.agents.sort_by(|a, b| {
            a.earliest_tick
                .unwrap_or(i64::MAX)
                .cmp(&b.earliest_tick.unwrap_or(i64::MAX))
        });

        Ok(self.agents.len())
    }

    /// 检查单个 Agent 数据库，提取元数据
    fn inspect_agent_db(&self, db_path: &Path, agent_id: Uuid) -> Result<AgentLifetime> {
        let conn = Connection::open(db_path).context("Failed to open agent database")?;

        let memory_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM client_memories", [], |row| row.get(0))
            .unwrap_or(0);

        let earliest_tick: Option<i64> = conn
            .query_row("SELECT MIN(tick_id) FROM client_memories", [], |row| {
                row.get(0)
            })
            .ok()
            .flatten();

        let latest_tick: Option<i64> = conn
            .query_row("SELECT MAX(tick_id) FROM client_memories", [], |row| {
                row.get(0)
            })
            .ok()
            .flatten();

        Ok(AgentLifetime {
            agent_id,
            db_path: db_path.to_path_buf(),
            memory_count: memory_count as usize,
            earliest_tick,
            latest_tick,
        })
    }

    /// 获取所有 Agent 列表
    pub fn agents(&self) -> &[AgentLifetime] {
        &self.agents
    }

    /// 获取 Agent 总数
    pub fn agent_count(&self) -> usize {
        self.agents.len()
    }

    /// 获取记忆总数（所有 Agent）
    pub fn total_memory_count(&self) -> usize {
        self.agents.iter().map(|a| a.memory_count).sum()
    }

    /// 从指定 Agent 数据库读取记忆
    fn read_memories_from_db(&self, db_path: &Path, limit: usize) -> Result<Vec<ClientMemory>> {
        let conn = Connection::open(db_path).context("Failed to open agent database")?;

        let mut stmt = conn
            .prepare(
                "SELECT id, agent_id, tick_id, event_type, content, metadata,
                    importance_score, sentiment_score, memory_type, is_confirmed,
                    created_at, updated_at
             FROM client_memories
             ORDER BY importance_score DESC, tick_id DESC
             LIMIT ?1",
            )
            .context("Failed to prepare query")?;

        let memories = stmt
            .query_map([limit as i64], |row| {
                Ok(ClientMemory {
                    id: Some(row.get(0)?),
                    agent_id: Uuid::parse_str(&row.get::<_, String>(1)?).unwrap_or_default(),
                    tick_id: row.get(2)?,
                    event_type: row.get(3)?,
                    content: row.get(4)?,
                    metadata: row
                        .get::<_, Option<String>>(5)?
                        .and_then(|s| serde_json::from_str(&s).ok())
                        .unwrap_or(serde_json::Value::Null),
                    importance_score: row.get(6)?,
                    sentiment_score: row.get(7)?,
                    memory_type: row.get(8)?,
                    is_confirmed: row.get(9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                })
            })
            .context("Failed to execute query")?;

        memories
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.into())
    }

    /// 从指定 Agent 数据库读取指定时间范围内的记忆
    fn read_memories_in_range(
        &self,
        db_path: &Path,
        tick_start: i64,
        tick_end: i64,
        limit: usize,
    ) -> Result<Vec<ClientMemory>> {
        let conn = Connection::open(db_path).context("Failed to open agent database")?;

        let mut stmt = conn
            .prepare(
                "SELECT id, agent_id, tick_id, event_type, content, metadata,
                    importance_score, sentiment_score, memory_type, is_confirmed,
                    created_at, updated_at
             FROM client_memories
             WHERE tick_id >= ?1 AND tick_id <= ?2
             ORDER BY importance_score DESC, tick_id DESC
             LIMIT ?3",
            )
            .context("Failed to prepare query")?;

        let memories = stmt
            .query_map([tick_start, tick_end, limit as i64], |row| {
                Ok(ClientMemory {
                    id: Some(row.get(0)?),
                    agent_id: Uuid::parse_str(&row.get::<_, String>(1)?).unwrap_or_default(),
                    tick_id: row.get(2)?,
                    event_type: row.get(3)?,
                    content: row.get(4)?,
                    metadata: row
                        .get::<_, Option<String>>(5)?
                        .and_then(|s| serde_json::from_str(&s).ok())
                        .unwrap_or(serde_json::Value::Null),
                    importance_score: row.get(6)?,
                    sentiment_score: row.get(7)?,
                    memory_type: row.get(8)?,
                    is_confirmed: row.get(9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                })
            })
            .context("Failed to execute query")?;

        memories
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.into())
    }

    /// 获取所有 Agent 最重要的 N 条记忆
    pub fn get_top_memories(&self, limit_per_agent: usize) -> Result<Vec<ClientMemory>> {
        let mut all_memories = Vec::new();

        for agent in &self.agents {
            if let Ok(memories) = self.read_memories_from_db(&agent.db_path, limit_per_agent) {
                all_memories.extend(memories);
            }
        }

        // 按重要性全局排序
        all_memories.sort_by(|a, b| {
            b.importance_score
                .partial_cmp(&a.importance_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(all_memories)
    }

    /// 获取指定时间范围内所有 Agent 的记忆
    pub fn get_memories_in_range(
        &self,
        tick_start: i64,
        tick_end: i64,
        limit: usize,
    ) -> Result<Vec<ClientMemory>> {
        let mut all_memories = Vec::new();

        for agent in &self.agents {
            // 先检查这个 agent 是否有在范围内的记忆
            if let (Some(earliest), Some(latest)) = (agent.earliest_tick, agent.latest_tick)
                && (latest < tick_start || earliest > tick_end) {
                    continue; // 跳过没有交集的 agent
                }

            if let Ok(memories) =
                self.read_memories_in_range(&agent.db_path, tick_start, tick_end, limit)
            {
                all_memories.extend(memories);
            }

            if all_memories.len() >= limit {
                break;
            }
        }

        // 按重要性排序
        all_memories.sort_by(|a, b| {
            b.importance_score
                .partial_cmp(&a.importance_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        all_memories.truncate(limit);
        Ok(all_memories)
    }

    /// 生成全局记忆报告
    ///
    /// `tick_end` - 当前 tick
    /// `hours` - 向前追溯的小时数
    /// `top_memories_limit` - 每个Agent最重要的记忆数量
    pub fn generate_report(
        &mut self,
        tick_end: i64,
        hours: u8,
        top_memories_limit: usize,
    ) -> Result<GlobalMemoryReport> {
        // 确保已扫描
        if self.agents.is_empty() {
            self.scan()?;
        }

        let tick_start = tick_end.saturating_sub(hours as i64 * 9);

        let memories_in_range = self.get_memories_in_range(tick_start, tick_end, 100)?;
        let top_memories = self.get_top_memories(top_memories_limit)?;

        Ok(GlobalMemoryReport {
            generated_at: chrono::Utc::now(),
            total_agents: self.agents.len(),
            total_memories: self.total_memory_count(),
            agents: self.agents.clone(),
            top_memories,
            memories_in_range,
        })
    }

    /// 获取默认数据库目录
    fn default_db_dir() -> String {
        if let Ok(home_dir) = std::env::var("HOME") {
            format!("{}/.cyber-jianghu", home_dir)
        } else {
            ".".to_string()
        }
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_registry_scan() {
        let temp_dir = TempDir::new().unwrap();
        let mut registry = GlobalMemoryRegistry::new(temp_dir.path());

        // 初始应该为空
        registry.scan().unwrap();
        assert_eq!(registry.agent_count(), 0);
    }

    #[test]
    fn test_registry_with_agents() {
        let temp_dir = TempDir::new().unwrap();

        // 创建两个 agent 的数据库
        let agent1_id = Uuid::new_v4();
        let agent2_id = Uuid::new_v4();

        {
            use crate::ai::memory::store::ClientMemory;
            use crate::ai::memory::store::MemoryStore;
            let store1 = MemoryStore::new(agent1_id, temp_dir.path()).unwrap();
            let store2 = MemoryStore::new(agent2_id, temp_dir.path()).unwrap();

            // 添加一些记忆
            let memory1 =
                ClientMemory::new(agent1_id, 1, "Agent1 记忆".to_string()).with_importance(0.8);
            let memory2 =
                ClientMemory::new(agent2_id, 2, "Agent2 记忆".to_string()).with_importance(0.9);

            store1.add_memory(&memory1).unwrap();
            store2.add_memory(&memory2).unwrap();
        }

        // 扫描
        let mut registry = GlobalMemoryRegistry::new(temp_dir.path());
        let count = registry.scan().unwrap();

        assert_eq!(count, 2);
        assert_eq!(registry.total_memory_count(), 2);

        // 获取重要记忆
        let top = registry.get_top_memories(10).unwrap();
        assert_eq!(top.len(), 2);
        // Agent2 的记忆更重要，应该排在前面
        assert_eq!(top[0].agent_id, agent2_id);
    }

    #[test]
    fn test_generate_report() {
        let temp_dir = TempDir::new().unwrap();

        // 创建 agent 数据库
        let agent_id = Uuid::new_v4();
        {
            use crate::ai::memory::store::ClientMemory;
            use crate::ai::memory::store::MemoryStore;
            let store = MemoryStore::new(agent_id, temp_dir.path()).unwrap();

            let memory =
                ClientMemory::new(agent_id, 5, "测试记忆".to_string()).with_importance(0.8);
            store.add_memory(&memory).unwrap();
        }

        let mut registry = GlobalMemoryRegistry::new(temp_dir.path());
        let report = registry.generate_report(10, 1, 10).unwrap();

        assert_eq!(report.total_agents, 1);
        assert_eq!(report.total_memories, 1);
        assert_eq!(report.agents.len(), 1);
    }
}
