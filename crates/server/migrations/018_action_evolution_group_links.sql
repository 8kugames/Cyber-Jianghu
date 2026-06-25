-- 018_action_evolution_group_links.sql
-- 为 action_evolution proposal/group 建立真实关联表，消除 JSONB proposal_ids 伪关联

CREATE TABLE IF NOT EXISTS action_evolution_group_proposals (
    proposal_group_id UUID NOT NULL REFERENCES action_evolution_proposal_groups(id) ON DELETE CASCADE,
    proposal_id       UUID NOT NULL REFERENCES action_evolution_proposals(id) ON DELETE CASCADE,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (proposal_group_id, proposal_id),
    UNIQUE (proposal_id)
);

CREATE INDEX IF NOT EXISTS idx_action_evolution_group_proposals_group_id
    ON action_evolution_group_proposals(proposal_group_id);

INSERT INTO action_evolution_group_proposals (proposal_group_id, proposal_id)
SELECT
    groups.id,
    proposal.id
FROM action_evolution_proposal_groups AS groups
CROSS JOIN LATERAL jsonb_array_elements(COALESCE(groups.proposal_ids, '[]'::jsonb)) AS proposal_ref(value)
JOIN action_evolution_proposals AS proposal
    ON proposal.id = (proposal_ref.value #>> '{}')::uuid
ON CONFLICT (proposal_id) DO NOTHING;

COMMENT ON TABLE action_evolution_group_proposals
    IS '动作演化 proposal 与 proposal_group 的真实关联表（双外键约束）';
