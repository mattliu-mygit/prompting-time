ALTER TABLE provider_sessions ADD COLUMN native_group_id TEXT;

ALTER TABLE staged_provider_events ADD COLUMN payload_json TEXT
    CHECK (payload_json IS NULL OR json_valid(payload_json));
