-- 010_action_evolution.sql
CREATE TABLE IF NOT EXISTS action_evolution_proposals (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id            UUID NOT NULL REFERENCES agents(agent_id) ON DELETE CASCADE,
    tick_id             BIGINT NOT NULL,
    actor_arity         SMALLINT NOT NULL DEFAULT 1,
    target_arity        TEXT NOT NULL DEFAULT 'zero_to_many',
    tick_span           SMALLINT NOT NULL DEFAULT 0,
    phase_count         SMALLINT NOT NULL DEFAULT 1,
    protocol_kind       TEXT NOT NULL DEFAULT 'none',
    state_transition_count SMALLINT NOT NULL DEFAULT 1,
    effect_refs         JSONB NOT NULL DEFAULT '[]',
    requirement_refs    JSONB NOT NULL DEFAULT '[]',
    proposed_action_type TEXT NOT NULL,
    rationale           TEXT NOT NULL,
    governance_topics   JSONB NOT NULL DEFAULT '[]',
    topic_confidence    JSONB NOT NULL DEFAULT '{}',
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS action_evolution_proposal_groups (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    similarity_key      TEXT NOT NULL UNIQUE,
    primary_soul        TEXT,
    co_reviewers        JSONB NOT NULL DEFAULT '[]',
    governance_topics   JSONB NOT NULL DEFAULT '[]',
    status              TEXT NOT NULL DEFAULT 'pending_review',
    votes               JSONB NOT NULL DEFAULT '[]',
    final_decision      TEXT,
    dissent_log         JSONB NOT NULL DEFAULT '[]',
    generated_config    JSONB,
    actions_version     TEXT,
    proposal_ids        JSONB NOT NULL DEFAULT '[]',
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS soul_review_votes (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    proposal_group_id   UUID NOT NULL REFERENCES action_evolution_proposal_groups(id),
    soul                TEXT NOT NULL,
    role                TEXT NOT NULL,
    vote                TEXT NOT NULL,
    rationale           TEXT NOT NULL DEFAULT '',
    evidence_refs       JSONB NOT NULL DEFAULT '[]',
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(proposal_group_id, soul)
);

CREATE INDEX IF NOT EXISTS idx_proposals_agent_id ON action_evolution_proposals(agent_id);
CREATE INDEX IF NOT EXISTS idx_proposals_created_at ON action_evolution_proposals(created_at);
CREATE INDEX IF NOT EXISTS idx_proposal_groups_status ON action_evolution_proposal_groups(status);
CREATE INDEX IF NOT EXISTS idx_proposal_groups_similarity ON action_evolution_proposal_groups(similarity_key);
CREATE INDEX IF NOT EXISTS idx_proposal_groups_primary_soul ON action_evolution_proposal_groups(primary_soul);
CREATE INDEX IF NOT EXISTS idx_votes_group ON soul_review_votes(proposal_group_id);

COMMENT ON TABLE action_evolution_proposals IS '动作演化提案原始证据';
COMMENT ON TABLE action_evolution_proposal_groups IS '动作演化提案组（治理状态机主载体）';
COMMENT ON TABLE soul_review_votes IS '三皇投票记录';
