-- JSONB attribute key rename: hunger→satiation, thirst→hydration

UPDATE agent_states
SET attributes = (
    SELECT jsonb_object_agg(
        CASE WHEN key = 'hunger' THEN 'satiation'
             WHEN key = 'thirst' THEN 'hydration'
             ELSE key
        END,
        value
    )
    FROM jsonb_each(attributes)
)
WHERE attributes ? 'hunger' OR attributes ? 'thirst';
