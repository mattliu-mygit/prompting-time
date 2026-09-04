DROP INDEX idx_runs_one_application_turn;

-- Task 9 briefly persisted a Boolean fallback reservation without enough intent to
-- resume it. Release any such unrecoverable legacy reservation before replacing
-- the index; the primary failure remains durable and visible to the user.
UPDATE provider_runs SET fallback_pending = 0 WHERE fallback_pending = 1;

CREATE UNIQUE INDEX idx_runs_one_application_turn
ON provider_runs(conversation_id)
WHERE application_managed = 1 AND status IN ('queued', 'running', 'waiting');

ALTER TABLE provider_runs ADD COLUMN turn_prompt TEXT;

ALTER TABLE agent_nodes ADD COLUMN provider_native_id TEXT;
ALTER TABLE agent_nodes ADD COLUMN provider_native_path TEXT;
ALTER TABLE agent_nodes ADD COLUMN summary TEXT;

UPDATE agent_nodes
SET provider_native_id = (
    SELECT provider_runs.native_session_id
    FROM provider_runs
    WHERE provider_runs.id = agent_nodes.run_id
)
WHERE parent_id IS NULL
  AND provider_native_id IS NULL
  AND EXISTS (
      SELECT 1 FROM provider_runs
      WHERE provider_runs.id = agent_nodes.run_id
        AND provider_runs.native_session_id IS NOT NULL
  );

CREATE UNIQUE INDEX idx_agent_nodes_native_identity
ON agent_nodes(run_id, provider_native_id)
WHERE provider_native_id IS NOT NULL;

ALTER TABLE routing_decisions ADD COLUMN reason TEXT;
ALTER TABLE routing_decisions ADD COLUMN task_kind TEXT;

UPDATE routing_decisions
SET reason = json_extract(details_json, '$.reason'),
    task_kind = json_extract(details_json, '$.taskKind');
