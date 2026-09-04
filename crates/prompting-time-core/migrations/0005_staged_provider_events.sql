CREATE TABLE staged_provider_events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    conversation_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('message', 'progress', 'tool')),
    content TEXT NOT NULL,
    mutation_state TEXT CHECK (
        mutation_state IS NULL OR mutation_state IN ('none_observed', 'observed', 'unknown')
    ),
    created_at INTEGER NOT NULL,
    CHECK (
        (kind = 'tool' AND mutation_state IS NOT NULL)
        OR (kind != 'tool' AND mutation_state IS NULL)
    ),
    FOREIGN KEY (run_id, conversation_id) REFERENCES provider_runs(id, conversation_id) ON DELETE CASCADE,
    FOREIGN KEY (agent_id, run_id) REFERENCES agent_nodes(id, run_id) ON DELETE CASCADE
);

CREATE INDEX idx_staged_provider_events_run_sequence
ON staged_provider_events(run_id, sequence);
