use std::collections::HashMap;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use cyber_jianghu_protocol::GovernanceTopic;
use sqlx::{PgPool, Postgres, Row};
use uuid::Uuid;

use super::types::{ProposalEvidence, ProposalStage, ProposalStatus, VoteChoice};

// ---------------------------------------------------------------------------
// Row types (sqlx::FromRow)
// ---------------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
#[allow(dead_code)]
struct ProposalRow {
    id: Uuid,
    agent_id: Uuid,
    tick_id: i64,
    proposed_action_type: String,
    rationale: String,
    action_data: serde_json::Value,
    governance_topics: serde_json::Value,
    topic_confidence: serde_json::Value,
    created_at: DateTime<Utc>,
}

#[derive(Debug, sqlx::FromRow)]
#[allow(dead_code)]
struct GroupRow {
    id: Uuid,
    similarity_key: String,
    primary_soul: Option<String>,
    co_reviewers: serde_json::Value,
    governance_topics: serde_json::Value,
    status: String,
    stage: Option<String>,
    votes: serde_json::Value,
    final_decision: Option<String>,
    dissent_log: serde_json::Value,
    generated_config: Option<serde_json::Value>,
    actions_version: Option<String>,
    proposal_ids: serde_json::Value,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, sqlx::FromRow)]
struct VoteRow {
    soul: String,
    role: String,
    vote: String,
    rationale: String,
    evidence_refs: serde_json::Value,
    created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PendingGroup {
    pub id: Uuid,
    pub similarity_key: String,
    pub primary_soul: Option<String>,
    pub governance_topics: Vec<GovernanceTopic>,
    pub proposal_count: usize,
    pub stage: ProposalStage,
    pub created_at: DateTime<Utc>,
}

/// soul_review_votes 表的行映射
///
/// 每条记录表示某 soul 在管道某阶段对某 group 的裁决。
/// role 取值："primary"（伏羲初审）/ "co_reviewer"（神农轩辕）/ "primary_final"（伏羲终审）。
#[derive(Debug, Clone)]
pub struct GroupVote {
    pub soul: String,
    pub role: String,
    /// 投票选择（与 votes 表 vote 字符串 "approve"/"reject"/"abstain" 对齐）
    ///
    /// 注：类型必须为 VoteChoice（而非 ProposalStatus）。
    /// votes 表存的是 "approve"/"reject" 字符串，与 VoteChoice serde 对齐；
    /// 若误用 ProposalStatus，反序列化会全部 fallback 到 Error，破坏阶段 3 终审输入。
    pub vote: VoteChoice,
    pub rationale: String,
    pub evidence_refs: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct GroupFull {
    pub id: Uuid,
    pub similarity_key: String,
    pub primary_soul: Option<String>,
    pub co_reviewers: Vec<String>,
    pub governance_topics: Vec<GovernanceTopic>,
    pub status: ProposalStatus,
    pub stage: ProposalStage,
    pub votes: Vec<GroupVote>,
    pub final_decision: Option<String>,
    pub dissent_log: Vec<serde_json::Value>,
    pub generated_config: Option<serde_json::Value>,
    pub actions_version: Option<String>,
    pub proposal_ids: Vec<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// ProposalStore
// ---------------------------------------------------------------------------

pub struct ProposalStore {
    pool: PgPool,
}

impl ProposalStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn insert_proposal(&self, evidence: &ProposalEvidence) -> Result<Uuid> {
        let row = sqlx::query_as::<Postgres, ProposalRow>(
            "INSERT INTO action_evolution_proposals \
             (agent_id, tick_id, proposed_action_type, rationale, action_data, \
              governance_topics, topic_confidence) \
             VALUES ($1,$2,$3,$4,$5,$6,$7) \
             RETURNING *",
        )
        .bind(evidence.agent_id)
        .bind(evidence.tick_id)
        .bind(&evidence.proposed_action_type)
        .bind(&evidence.rationale)
        .bind(&evidence.action_data)
        .bind(
            serde_json::to_value(&evidence.governance_topics)
                .context("serialize governance_topics")?,
        )
        .bind(
            serde_json::to_value(&evidence.topic_confidence)
                .context("serialize topic_confidence")?,
        )
        .fetch_one(&self.pool)
        .await
        .context("insert proposal")?;

        Ok(row.id)
    }

    /// 创建或追加 proposal_group（按 similarity_key = `action:{action_type}` 去重）
    ///
    /// # ON CONFLICT 重置语义
    ///
    /// 已 `stage=done` 的 group 接收新 proposal 时（同名 action 重新提议），
    /// CASE 表达式重置 stage='awaiting_fuxi_initial' + status='pending_review'，
    /// 重启审议管道。否则新 proposal 会被静默吞掉（done group 不再被轮询）。
    ///
    /// 未 done 的 group 不重置 stage（避免打断进行中的审议）。
    pub async fn upsert_proposal_group(
        &self,
        similarity_key: &str,
        proposal_id: Uuid,
        governance_topics: &[GovernanceTopic],
        primary_soul: Option<&str>,
    ) -> Result<Uuid> {
        let topics_val = serde_json::to_value(governance_topics).context("serialize topics")?;
        let pid_json = serde_json::to_value(proposal_id).context("serialize proposal_id")?;

        let row = sqlx::query_as::<Postgres, GroupRow>(
            "INSERT INTO action_evolution_proposal_groups \
             (similarity_key, proposal_ids, governance_topics, primary_soul) \
             VALUES ($1, jsonb_build_array($2), $3, $4) \
             ON CONFLICT (similarity_key) DO UPDATE SET \
             proposal_ids = action_evolution_proposal_groups.proposal_ids || EXCLUDED.proposal_ids, \
             governance_topics = ( \
                 SELECT jsonb_agg(DISTINCT elem) \
                 FROM jsonb_array_elements( \
                     action_evolution_proposal_groups.governance_topics || EXCLUDED.governance_topics \
                 ) AS elem \
             ), \
             status = CASE WHEN action_evolution_proposal_groups.stage = 'done' \
                           THEN 'pending_review' \
                           ELSE action_evolution_proposal_groups.status END, \
             stage = CASE WHEN action_evolution_proposal_groups.stage = 'done' \
                          THEN 'awaiting_fuxi_initial' \
                          ELSE action_evolution_proposal_groups.stage END, \
             updated_at = NOW() \
             RETURNING *",
        )
        .bind(similarity_key)
        .bind(pid_json)
        .bind(topics_val)
        .bind(primary_soul)
        .fetch_one(&self.pool)
        .await
        .context("upsert proposal group")?;

        Ok(row.id)
    }

    pub async fn get_pending_groups(&self) -> Result<Vec<PendingGroup>> {
        let rows = sqlx::query_as::<Postgres, GroupRow>(
            "SELECT * FROM action_evolution_proposal_groups \
             WHERE status IN ('pending_review', 'under_review') ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await
        .context("query pending groups")?;

        rows.into_iter()
            .map(|r| {
                let proposal_ids: Vec<Uuid> =
                    serde_json::from_value(r.proposal_ids).context("deserialize proposal_ids")?;
                let governance_topics: Vec<GovernanceTopic> =
                    serde_json::from_value(r.governance_topics).context("deserialize topics")?;
                let stage = ProposalStage::from_db_str(r.stage.as_deref().unwrap_or(""));
                Ok(PendingGroup {
                    id: r.id,
                    similarity_key: r.similarity_key,
                    primary_soul: r.primary_soul,
                    governance_topics,
                    proposal_count: proposal_ids.len(),
                    stage,
                    created_at: r.created_at,
                })
            })
            .collect()
    }

    pub async fn get_group(&self, group_id: Uuid) -> Result<Option<GroupFull>> {
        let row = sqlx::query_as::<Postgres, GroupRow>(
            "SELECT * FROM action_evolution_proposal_groups WHERE id = $1",
        )
        .bind(group_id)
        .fetch_optional(&self.pool)
        .await
        .context("query group")?;

        let row = match row {
            Some(r) => r,
            None => return Ok(None),
        };

        let votes = self.get_group_votes(group_id).await?;

        let governance_topics: Vec<GovernanceTopic> =
            serde_json::from_value(row.governance_topics).context("deserialize topics")?;
        let proposal_ids: Vec<Uuid> =
            serde_json::from_value(row.proposal_ids).context("deserialize proposal_ids")?;
        let dissent_log: Vec<serde_json::Value> =
            serde_json::from_value(row.dissent_log).context("deserialize dissent_log")?;
        let co_reviewers: Vec<String> =
            serde_json::from_value(row.co_reviewers).context("deserialize co_reviewers")?;
        let status = ProposalStatus::from_db_str(&row.status);
        let stage = ProposalStage::from_db_str(row.stage.as_deref().unwrap_or(""));

        Ok(Some(GroupFull {
            id: row.id,
            similarity_key: row.similarity_key,
            primary_soul: row.primary_soul,
            co_reviewers,
            governance_topics,
            status,
            stage,
            votes,
            final_decision: row.final_decision,
            dissent_log,
            generated_config: row.generated_config,
            actions_version: row.actions_version,
            proposal_ids,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }))
    }

    async fn get_group_votes(&self, group_id: Uuid) -> Result<Vec<GroupVote>> {
        let rows = sqlx::query_as::<Postgres, VoteRow>(
            "SELECT soul, role, vote, rationale, evidence_refs, created_at \
             FROM soul_review_votes WHERE proposal_group_id = $1 ORDER BY created_at",
        )
        .bind(group_id)
        .fetch_all(&self.pool)
        .await
        .context("query group votes")?;

        rows.into_iter()
            .map(|r| {
                let evidence_refs: Vec<String> =
                    serde_json::from_value(r.evidence_refs).context("deserialize evidence_refs")?;
                // votes 表 vote 字段存 "approve"/"reject"/"abstain"，与 VoteChoice serde 对齐
                let vote: VoteChoice = serde_json::from_value(serde_json::Value::String(r.vote))
                    .unwrap_or(VoteChoice::Reject);
                Ok(GroupVote {
                    soul: r.soul,
                    role: r.role,
                    vote,
                    rationale: r.rationale,
                    evidence_refs,
                    created_at: r.created_at,
                })
            })
            .collect()
    }

    pub async fn get_proposal(&self, proposal_id: Uuid) -> Result<Option<ProposalEvidence>> {
        let row = sqlx::query(
            r"SELECT agent_id, tick_id, proposed_action_type, rationale, action_data,
                      governance_topics, topic_confidence
               FROM action_evolution_proposals WHERE id = $1",
        )
        .bind(proposal_id)
        .fetch_optional(&self.pool)
        .await?;

        let Some(r) = row else { return Ok(None) };

        let action_data: serde_json::Value = r.get(4);
        let governance_topics: Vec<GovernanceTopic> =
            serde_json::from_value(r.get::<serde_json::Value, _>(5))
                .context("反序列化 governance_topics 失败")?;
        let topic_confidence: HashMap<GovernanceTopic, f64> =
            serde_json::from_value(r.get::<serde_json::Value, _>(6))
                .context("反序列化 topic_confidence 失败")?;

        Ok(Some(ProposalEvidence {
            agent_id: r.get(0),
            tick_id: r.get(1),
            proposed_action_type: r.get(2),
            rationale: r.get(3),
            action_data,
            governance_topics,
            topic_confidence,
        }))
    }

    /// 更新管道阶段（awaiting_fuxi_initial / awaiting_peer / awaiting_fuxi_final / done）
    pub async fn update_group_stage(&self, group_id: Uuid, stage: ProposalStage) -> Result<()> {
        sqlx::query(
            "UPDATE action_evolution_proposal_groups \
             SET stage = $2, updated_at = NOW() WHERE id = $1",
        )
        .bind(group_id)
        .bind(stage.as_str())
        .execute(&self.pool)
        .await
        .context("update group stage")?;

        Ok(())
    }

    /// 同时更新状态与阶段
    pub async fn update_group_status_and_stage(
        &self,
        group_id: Uuid,
        status: ProposalStatus,
        stage: ProposalStage,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE action_evolution_proposal_groups \
             SET status = $2, stage = $3, updated_at = NOW() WHERE id = $1",
        )
        .bind(group_id)
        .bind(status.to_string())
        .bind(stage.as_str())
        .execute(&self.pool)
        .await
        .context("update group status and stage")?;

        Ok(())
    }

    /// 强制关闭超时且**尚未进入审议管道**的 group
    ///
    /// # 为什么仅关闭 awaiting_fuxi_initial
    ///
    /// group 在三阶段管道中流转时 status 始终为 pending_review（仅 stage 推进）。
    /// 若 close_stale_groups 不区分 stage，会误关闭正在 awaiting_peer / awaiting_fuxi_final
    /// 中流转的 group，丢失已完成的初审/同辈审议结果。
    ///
    /// awaiting_fuxi_initial 卡住意味着数据陈旧或 LLM 持续失败——重启管道比保留更合理。
    /// 已进入审议管道的 group 由轮询任务自动重试当前阶段，无需强制关闭。
    pub async fn close_stale_groups(&self, timeout_secs: u64) -> Result<u64> {
        let timeout_i64 = timeout_secs as i64;
        let result = sqlx::query(
            "UPDATE action_evolution_proposal_groups \
             SET status = 'closed_rejected', final_decision = 'timeout', stage = 'done', updated_at = NOW() \
             WHERE status IN ('pending_review', 'under_review') \
             AND stage = 'awaiting_fuxi_initial' \
             AND created_at < NOW() - make_interval(secs => $1)",
        )
        .bind(timeout_i64)
        .execute(&self.pool)
        .await
        .context("close stale groups")?;

        Ok(result.rows_affected())
    }

    pub async fn write_dissent_log(
        &self,
        group_id: Uuid,
        dissent: serde_json::Value,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE action_evolution_proposal_groups \
             SET dissent_log = dissent_log || $2, updated_at = NOW() \
             WHERE id = $1",
        )
        .bind(group_id)
        .bind(dissent)
        .execute(&self.pool)
        .await
        .context("append dissent log")?;

        Ok(())
    }

    /// 持久化单条投票记录到 soul_review_votes 表
    pub async fn write_vote(
        &self,
        proposal_group_id: Uuid,
        soul: &str,
        role: &str,
        vote: &str,
        rationale: &str,
        evidence_refs: &[String],
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO soul_review_votes \
             (proposal_group_id, soul, role, vote, rationale, evidence_refs) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(proposal_group_id)
        .bind(soul)
        .bind(role)
        .bind(vote)
        .bind(rationale)
        .bind(serde_json::to_value(evidence_refs).context("serialize evidence_refs")?)
        .execute(&self.pool)
        .await
        .context("insert vote")?;

        Ok(())
    }
}
