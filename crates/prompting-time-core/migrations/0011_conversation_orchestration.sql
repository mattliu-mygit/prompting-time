CREATE TABLE conversation_settings (
    conversation_id TEXT PRIMARY KEY NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    objective TEXT NOT NULL,
    constraints_json TEXT NOT NULL CHECK (json_valid(constraints_json)),
    routing_profile TEXT NOT NULL CHECK (routing_profile IN ('balanced', 'best_fit', 'usage_balance'))
);

ALTER TABLE provider_sessions
ADD COLUMN context_through_sequence INTEGER NOT NULL DEFAULT 0
CHECK (context_through_sequence >= 0);

ALTER TABLE provider_runs ADD COLUMN handoff_rendered TEXT;
ALTER TABLE provider_runs ADD COLUMN handoff_hash TEXT;
ALTER TABLE provider_runs ADD COLUMN context_through_sequence INTEGER
CHECK (context_through_sequence IS NULL OR context_through_sequence >= 0);
ALTER TABLE provider_runs ADD COLUMN application_managed INTEGER NOT NULL DEFAULT 0
CHECK (application_managed IN (0, 1));
ALTER TABLE provider_runs ADD COLUMN fallback_pending INTEGER NOT NULL DEFAULT 0
CHECK (fallback_pending IN (0, 1));

CREATE UNIQUE INDEX idx_runs_one_application_turn
ON provider_runs(conversation_id)
WHERE application_managed = 1
AND (status IN ('queued', 'running', 'waiting') OR fallback_pending = 1);

CREATE TABLE submitted_commands (
    command_id TEXT PRIMARY KEY NOT NULL,
    request_hash TEXT NOT NULL,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    run_id TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL,
    FOREIGN KEY (run_id, conversation_id) REFERENCES provider_runs(id, conversation_id) ON DELETE CASCADE
);

CREATE INDEX idx_messages_conversation_sequence
ON messages(conversation_id, sequence);

ALTER TABLE messages ADD COLUMN native_item_id TEXT;

CREATE UNIQUE INDEX idx_messages_native_item
ON messages(run_id, native_item_id)
WHERE role = 'assistant' AND native_item_id IS NOT NULL;
