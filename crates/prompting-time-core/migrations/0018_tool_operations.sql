-- Display detail remains on canonical events. Legacy rows have no operation identity.
ALTER TABLE events ADD COLUMN operation_json TEXT;
ALTER TABLE events ADD COLUMN operation_revision INTEGER NOT NULL DEFAULT 0;
ALTER TABLE events ADD COLUMN native_turn_id TEXT;
CREATE UNIQUE INDEX idx_events_tool_operation
ON events(run_id, agent_id, coalesce(native_turn_id, ''), native_item_id)
WHERE operation_json IS NOT NULL;
