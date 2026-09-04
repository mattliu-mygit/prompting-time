ALTER TABLE events ADD COLUMN native_item_id TEXT;

CREATE UNIQUE INDEX idx_events_native_message
ON events(run_id, agent_id, native_item_id)
WHERE kind = 'message' AND native_item_id IS NOT NULL;

ALTER TABLE staged_provider_events ADD COLUMN native_item_id TEXT;

CREATE UNIQUE INDEX idx_staged_provider_events_native_message
ON staged_provider_events(run_id, agent_id, native_item_id)
WHERE kind = 'message' AND native_item_id IS NOT NULL;
