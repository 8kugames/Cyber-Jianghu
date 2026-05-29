// ============================================================================
// ConversationHistory — LLM 长窗口对话持久层
// ============================================================================
//
// 管理 Agent 与 LLM 的多轮对话历史：
// - 保留完整 user/assistant 轮次
// - 估算 token 数，超过阈值触发 summary 压缩
// - SQLite 持久化，agent 重启后恢复
//
// 压缩策略：
// - token_count > max_tokens * summary_trigger_ratio 时触发
// - 调用方生成 summary 后 replace_with_summary()
// - 保留最近 N 轮完整对话，旧轮次压缩为摘要文本
// ============================================================================

use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;
use tracing::{debug, info};

/// 对话轮次
#[derive(Debug, Clone)]
pub struct ConversationTurn {
    pub tick_id: i64,
    pub user: String,
    pub assistant: String,
    pub reasoning_content: Option<String>,
}

/// 对话历史管理器
pub struct ConversationHistory {
    /// SQLite 连接
    conn: Connection,
    /// System message (persona + static rules)
    system_message: String,
    /// 旧轮次压缩摘要
    summary: Option<String>,
    /// 完整对话轮次（内存）
    turns: Vec<ConversationTurn>,
    /// 估算 token 数
    token_count: usize,
    /// 上下文窗口上限
    max_tokens: usize,
    /// Summary 后保留最近 N 轮
    keep_recent_turns: usize,
    /// Summary 触发比例
    summary_trigger_ratio: f64,
}

impl ConversationHistory {
    /// 创建/加载对话历史
    ///
    /// 如果 DB 文件已存在，加载历史轮次和摘要。
    pub fn new(
        db_path: &Path,
        system_message: &str,
        max_tokens: usize,
        keep_recent_turns: usize,
        summary_trigger_ratio: f64,
    ) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(db_path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS conv_turns (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tick_id INTEGER NOT NULL,
                user_content TEXT NOT NULL,
                assistant_content TEXT NOT NULL,
                reasoning_content TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS conv_summary (
                id INTEGER PRIMARY KEY CHECK(id = 1),
                content TEXT NOT NULL,
                replaces_up_to INTEGER NOT NULL,
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )?;
        // 迁移：旧表可能没有 reasoning_content 列
        conn.execute_batch(
            "ALTER TABLE conv_turns ADD COLUMN reasoning_content TEXT",
        )
        .ok(); // 列已存在时忽略错误

        let mut history = Self {
            conn,
            system_message: system_message.to_string(),
            summary: None,
            turns: Vec::new(),
            token_count: 0,
            max_tokens,
            keep_recent_turns: keep_recent_turns.max(1),
            summary_trigger_ratio: summary_trigger_ratio.clamp(0.3, 0.95),
        };

        history.load_from_db()?;
        history.token_count = history.estimate_tokens();

        info!(
            "对话历史已加载: {} 轮, summary={}, 估算tokens={}/{}",
            history.turns.len(),
            history.summary.is_some(),
            history.token_count,
            history.max_tokens,
        );

        Ok(history)
    }

    /// 从 DB 恢复历史
    fn load_from_db(&mut self) -> Result<()> {
        // 加载摘要
        let result =
            self.conn
                .query_row("SELECT content FROM conv_summary WHERE id = 1", [], |row| {
                    row.get::<_, String>(0)
                });
        if let Ok(content) = result {
            self.summary = Some(content);
        }

        // 加载轮次
        let mut stmt = self.conn.prepare(
            "SELECT tick_id, user_content, assistant_content, reasoning_content FROM conv_turns ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ConversationTurn {
                tick_id: row.get(0)?,
                user: row.get(1)?,
                assistant: row.get(2)?,
                reasoning_content: row.get(3)?,
            })
        })?;

        self.turns.clear();
        for turn in rows {
            self.turns.push(turn?);
        }

        Ok(())
    }

    /// 添加一轮对话
    pub fn push_turn(
        &mut self,
        tick_id: i64,
        user: String,
        assistant: String,
        reasoning_content: Option<String>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO conv_turns (tick_id, user_content, assistant_content, reasoning_content) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![tick_id, user, assistant, reasoning_content],
        )?;

        let turn_tokens = estimate_tokens(&user) + estimate_tokens(&assistant);
        self.token_count += turn_tokens;

        self.turns.push(ConversationTurn {
            tick_id,
            user,
            assistant,
            reasoning_content,
        });

        debug!(
            "对话历史: +turn tick={}, total={}轮, tokens≈{}/{}",
            tick_id,
            self.turns.len(),
            self.token_count,
            self.max_tokens,
        );

        Ok(())
    }

    /// 是否需要触发 summary 压缩
    pub fn needs_summary(&self) -> bool {
        let threshold = (self.max_tokens as f64 * self.summary_trigger_ratio) as usize;
        self.token_count > threshold && self.turns.len() > self.keep_recent_turns
    }

    /// 生成 summary prompt 供 LLM 压缩旧轮次
    pub fn generate_summary_prompt(&self) -> String {
        let turns_to_compress = self.turns.len().saturating_sub(self.keep_recent_turns);
        if turns_to_compress == 0 {
            return String::new();
        }

        let turns_text: Vec<String> = self.turns[..turns_to_compress]
            .iter()
            .map(|t| {
                format!(
                    "[Tick {}]\n输入: {}\n决策: {}",
                    t.tick_id,
                    &t.user,
                    &t.assistant,
                )
            })
            .collect();

        let existing = self.summary.as_deref().unwrap_or("");

        format!(
            "请将以下AI角色的近期对话历史压缩为简洁摘要，\
             保留关键决策、关系变化、重要事件、位置移动、物品变动。\n\n\
             当前摘要:\n{}\n\n\
             新增对话:\n{}\n\n\
             直接输出摘要文本，不要输出JSON或额外格式。",
            existing,
            turns_text.join("\n\n"),
        )
    }

    /// 用 summary 替换旧轮次
    pub fn replace_with_summary(&mut self, summary: String) -> Result<()> {
        let turns_to_compress = self.turns.len().saturating_sub(self.keep_recent_turns);
        if turns_to_compress == 0 {
            return Ok(());
        }

        // 找到要删除的最大 ID
        let replaces_up_to: i64 = self
            .conn
            .query_row(
                "SELECT id FROM conv_turns ORDER BY id ASC LIMIT 1 OFFSET ?1",
                rusqlite::params![turns_to_compress as i64],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or(0)
            - 1;

        if replaces_up_to > 0 {
            self.conn.execute(
                "DELETE FROM conv_turns WHERE id <= ?1",
                rusqlite::params![replaces_up_to],
            )?;
        }

        self.conn.execute(
            "INSERT OR REPLACE INTO conv_summary (id, content, replaces_up_to, updated_at) \
             VALUES (1, ?1, ?2, datetime('now'))",
            rusqlite::params![summary, replaces_up_to],
        )?;

        // 保留最近 N 轮
        let kept: Vec<ConversationTurn> = self.turns.drain(turns_to_compress..).collect();
        self.turns = kept;
        self.summary = Some(summary);
        self.token_count = self.estimate_tokens();

        info!(
            "对话历史已压缩: 保留 {} 轮, tokens≈{}",
            self.turns.len(),
            self.token_count,
        );

        Ok(())
    }

    /// summary 生成失败时的降级路径：直接截断到 keep_recent_turns
    ///
    /// 避免对话轮次因 summary LLM 调用持续失败而无限堆积。
    /// 同步清理 SQLite 中被截断的 turns 行。
    pub fn force_truncate_to_recent(&mut self) -> Result<()> {
        if self.turns.len() <= self.keep_recent_turns {
            return Ok(());
        }

        let turns_to_compress = self.turns.len().saturating_sub(self.keep_recent_turns);

        // 找到要删除的最大 ID
        let replaces_up_to: i64 = self
            .conn
            .query_row(
                "SELECT id FROM conv_turns ORDER BY id ASC LIMIT 1 OFFSET ?1",
                rusqlite::params![turns_to_compress.saturating_sub(1)],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if replaces_up_to > 0 {
            self.conn.execute(
                "DELETE FROM conv_turns WHERE id <= ?1",
                rusqlite::params![replaces_up_to],
            )?;
        }

        // 保留最近 N 轮
        let kept: Vec<ConversationTurn> = self.turns.drain(turns_to_compress..).collect();
        self.turns = kept;

        // 保留已有 summary 或设置降级标记
        self.summary = self
            .summary
            .take()
            .or_else(|| Some("[历史对话因压缩失败被截断，仅保留近期对话]".to_string()));
        self.token_count = self.estimate_tokens();

        info!(
            "对话历史强制截断: 保留 {} 轮, tokens≈{}",
            self.turns.len(),
            self.token_count,
        );

        Ok(())
    }

    /// 更新上下文窗口上限（模型切换后调用）
    pub fn update_max_tokens(&mut self, max_tokens: usize) {
        self.max_tokens = max_tokens;
    }

    /// 更新 system message (persona 变更时调用)
    pub fn update_system_message(&mut self, msg: &str) {
        self.system_message = msg.to_string();
        self.token_count = self.estimate_tokens();
    }

    /// 清空对话历史 (rebirth 时调用)
    pub fn clear(&mut self) -> Result<()> {
        self.conn.execute("DELETE FROM conv_turns", [])?;
        self.conn.execute("DELETE FROM conv_summary", [])?;
        self.turns.clear();
        self.summary = None;
        self.token_count = self.estimate_tokens();
        info!("对话历史已清空");
        Ok(())
    }

    // ========================================================================
    // Accessors
    // ========================================================================

    pub fn get_system_message(&self) -> &str {
        &self.system_message
    }

    pub fn get_summary(&self) -> Option<&str> {
        self.summary.as_deref()
    }

    pub fn get_turns(&self) -> &[ConversationTurn] {
        &self.turns
    }

    pub fn turn_count(&self) -> usize {
        self.turns.len()
    }

    pub fn estimated_tokens(&self) -> usize {
        self.token_count
    }

    // ========================================================================
    // Private
    // ========================================================================

    fn estimate_tokens(&self) -> usize {
        let mut count = estimate_tokens(&self.system_message);
        if let Some(ref s) = self.summary {
            count += estimate_tokens(s);
        }
        for turn in &self.turns {
            count += estimate_tokens(&turn.user);
            count += estimate_tokens(&turn.assistant);
        }
        count
    }
}

/// 粗略估算文本 token 数
///
/// 中文 ~1.5 tokens/char，ASCII ~0.25 tokens/char
fn estimate_tokens(text: &str) -> usize {
    let mut cn = 0usize;
    let mut ascii = 0usize;
    for ch in text.chars() {
        if ch.is_ascii() {
            ascii += 1;
        } else {
            cn += 1;
        }
    }
    (cn as f64 * 1.5 + ascii as f64 * 0.25) as usize
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_tokens() {
        // 纯中文
        let cn_tokens = estimate_tokens("你好世界");
        assert!(cn_tokens > 0, "Chinese tokens should be > 0");

        // 纯 ASCII
        let ascii_tokens = estimate_tokens("hello world");
        assert!(ascii_tokens > 0, "ASCII tokens should be > 0");

        // 中文 token 密度应高于 ASCII
        assert!(cn_tokens > ascii_tokens);
    }

    #[test]
    fn test_push_and_needs_summary() {
        let dir = std::env::temp_dir().join("conv_test_push");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let db_path = dir.join("test.db");
        let mut history = ConversationHistory::new(
            &db_path, "system", 100, // low max_tokens for testing
            2,   // keep 2 turns
            0.8, // trigger at 80%
        )
        .unwrap();

        // Add turns until summary needed
        for i in 0..10 {
            history
                .push_turn(i, "用户消息".repeat(20), "助手回复".repeat(10), None)
                .unwrap();
        }

        assert!(
            history.needs_summary(),
            "Should need summary after 10 turns"
        );
        assert_eq!(history.turn_count(), 10);
    }

    #[test]
    fn test_replace_with_summary() {
        let dir = std::env::temp_dir().join("conv_test_summary");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let db_path = dir.join("test.db");
        let mut history = ConversationHistory::new(
            &db_path, "system", 10000, 2, // keep 2 turns
            0.8,
        )
        .unwrap();

        for i in 0..5 {
            history
                .push_turn(i, format!("用户消息 {}", i), format!("助手回复 {}", i), None)
                .unwrap();
        }

        let summary_prompt = history.generate_summary_prompt();
        assert!(summary_prompt.contains("用户消息"));

        history
            .replace_with_summary("这是摘要".to_string())
            .unwrap();

        assert_eq!(history.turn_count(), 2);
        assert_eq!(history.get_summary(), Some("这是摘要"));
    }

    #[test]
    fn test_persistence() {
        let dir = std::env::temp_dir().join("conv_test_persist");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let db_path = dir.join("test.db");

        // Write
        {
            let mut history = ConversationHistory::new(&db_path, "sys", 10000, 2, 0.8).unwrap();
            history
                .push_turn(1, "hello".to_string(), "world".to_string(), None)
                .unwrap();
            history
                .push_turn(2, "foo".to_string(), "bar".to_string(), None)
                .unwrap();
        }

        // Reload
        let history = ConversationHistory::new(&db_path, "sys", 10000, 2, 0.8).unwrap();
        assert_eq!(history.turn_count(), 2);
    }

    #[test]
    fn test_clear() {
        let dir = std::env::temp_dir().join("conv_test_clear");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let db_path = dir.join("test.db");
        let mut history = ConversationHistory::new(&db_path, "sys", 10000, 2, 0.8).unwrap();
        history
            .push_turn(1, "a".to_string(), "b".to_string(), None)
            .unwrap();

        history.clear().unwrap();
        assert_eq!(history.turn_count(), 0);
        assert!(history.get_summary().is_none());
    }
}
