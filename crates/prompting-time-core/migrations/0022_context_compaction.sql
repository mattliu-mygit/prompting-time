ALTER TABLE events ADD COLUMN compaction_key TEXT;
CREATE UNIQUE INDEX idx_events_compaction_key
ON events(conversation_id, compaction_key) WHERE compaction_key IS NOT NULL;
