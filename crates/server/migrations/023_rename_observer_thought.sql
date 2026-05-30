-- Rename observer_thought to reflector_thought (Observer role absorbed into ReflectorSoul)
ALTER TABLE agent_action_logs RENAME COLUMN observer_thought TO reflector_thought;
