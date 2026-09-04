-- Rebuild the event sequence once so pre-0013 user messages become first-class timeline events.
-- Historical message and event sequences were independent, so equal timestamps use a stable
-- user-before-provider tie rule. Existing v13 user events are replaced from their message row.
CREATE TABLE event_user_backfill_guard (
    valid INTEGER NOT NULL CHECK (valid = 1)
);

INSERT INTO event_user_backfill_guard (valid)
SELECT (
    (SELECT count(*) FROM messages WHERE role = 'user' AND run_id IS NOT NULL)
    =
    (SELECT count(*)
     FROM messages
     JOIN provider_runs ON provider_runs.id = messages.run_id
     JOIN agent_nodes ON agent_nodes.run_id = provider_runs.id AND agent_nodes.parent_id IS NULL
     WHERE messages.role = 'user' AND messages.run_id IS NOT NULL)
);

CREATE TABLE events_v14 (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    conversation_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('message', 'tool', 'progress', 'diagnostic', 'lifecycle')),
    content TEXT NOT NULL,
    payload_json TEXT CHECK (payload_json IS NULL OR json_valid(payload_json)),
    created_at INTEGER NOT NULL,
    native_item_id TEXT,
    role TEXT CHECK (role IS NULL OR role IN ('user', 'assistant')),
    FOREIGN KEY (run_id, conversation_id) REFERENCES provider_runs(id, conversation_id) ON DELETE CASCADE,
    FOREIGN KEY (agent_id, run_id) REFERENCES agent_nodes(id, run_id) ON DELETE CASCADE
);

INSERT INTO events_v14 (
    id, conversation_id, run_id, agent_id, kind, content, payload_json, created_at,
    native_item_id, role
)
SELECT id, conversation_id, run_id, agent_id, kind, content, payload_json, created_at,
       native_item_id, role
FROM (
    SELECT events.id,
           events.conversation_id,
           events.run_id,
           events.agent_id,
           events.kind,
           events.content,
           events.payload_json,
           events.created_at,
           events.native_item_id,
           events.role,
           1 AS tie_rank,
           events.sequence AS source_sequence
    FROM events
    WHERE events.role IS NULL OR events.role <> 'user'

    UNION ALL

    SELECT messages.id,
           messages.conversation_id,
           messages.run_id,
           agent_nodes.id,
           'message',
           messages.content,
           NULL,
           messages.created_at,
           NULL,
           'user',
           0,
           messages.sequence
    FROM messages
    JOIN provider_runs ON provider_runs.id = messages.run_id
    JOIN agent_nodes ON agent_nodes.run_id = provider_runs.id AND agent_nodes.parent_id IS NULL
    WHERE messages.role = 'user' AND messages.run_id IS NOT NULL
)
ORDER BY created_at, tie_rank, source_sequence, id;

DROP TABLE events;
ALTER TABLE events_v14 RENAME TO events;
DROP TABLE event_user_backfill_guard;

CREATE INDEX idx_events_conversation_sequence ON events(conversation_id, sequence);
CREATE INDEX idx_events_run_sequence ON events(run_id, sequence DESC);
CREATE UNIQUE INDEX idx_events_native_message
ON events(run_id, agent_id, native_item_id)
WHERE kind = 'message' AND native_item_id IS NOT NULL;

ALTER TABLE approvals
ADD COLUMN conversation_id TEXT REFERENCES conversations(id) ON DELETE CASCADE;

UPDATE approvals
SET conversation_id = (
    SELECT provider_runs.conversation_id
    FROM provider_runs
    WHERE provider_runs.id = approvals.run_id
);

CREATE TRIGGER approval_conversation_required_on_insert
BEFORE INSERT ON approvals
WHEN NEW.conversation_id IS NULL
  OR NOT EXISTS (
      SELECT 1 FROM provider_runs
      WHERE provider_runs.id = NEW.run_id
        AND provider_runs.conversation_id = NEW.conversation_id
  )
BEGIN
    SELECT RAISE(ABORT, 'approval conversation ownership mismatch');
END;

CREATE TRIGGER approval_conversation_required_on_update
BEFORE UPDATE OF conversation_id, run_id ON approvals
WHEN NEW.conversation_id IS NULL
  OR NOT EXISTS (
      SELECT 1 FROM provider_runs
      WHERE provider_runs.id = NEW.run_id
        AND provider_runs.conversation_id = NEW.conversation_id
  )
BEGIN
    SELECT RAISE(ABORT, 'approval conversation ownership mismatch');
END;

DROP INDEX idx_approvals_pending;
DROP INDEX idx_approvals_conversation_status_created;

CREATE INDEX idx_approvals_conversation_pending
ON approvals(conversation_id, created_at DESC, id DESC)
WHERE status = 'pending';

CREATE INDEX idx_approvals_conversation_history
ON approvals(conversation_id, created_at DESC, id DESC)
WHERE status <> 'pending';
