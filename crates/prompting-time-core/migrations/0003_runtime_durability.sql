ALTER TABLE provider_runs
ADD COLUMN fallback_from_run_id TEXT REFERENCES provider_runs(id) ON DELETE CASCADE;

CREATE UNIQUE INDEX idx_runs_fallback_from ON provider_runs(fallback_from_run_id);

CREATE TABLE approvals_next (
    id TEXT PRIMARY KEY NOT NULL,
    run_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    provider TEXT NOT NULL CHECK (provider IN ('codex', 'claude')),
    provider_request_id TEXT,
    operation TEXT NOT NULL,
    scope TEXT NOT NULL,
    status TEXT NOT NULL CHECK (
        status IN ('pending', 'approved', 'denied', 'answered', 'cancelled', 'failed')
    ),
    resolution_json TEXT CHECK (resolution_json IS NULL OR json_valid(resolution_json)),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK (
        (status = 'pending' AND resolution_json IS NULL)
        OR
        (
            status != 'pending'
            AND resolution_json IS NOT NULL
            AND (
                (status = 'answered' AND json_extract(resolution_json, '$.kind') = 'answer')
                OR (status != 'answered' AND json_extract(resolution_json, '$.kind') = status)
            )
        )
    ),
    FOREIGN KEY (agent_id, run_id) REFERENCES agent_nodes(id, run_id) ON DELETE CASCADE
);

INSERT INTO approvals_next (
    id, run_id, agent_id, provider, provider_request_id, operation, scope,
    status, resolution_json, created_at, updated_at
)
SELECT
    id, run_id, agent_id, provider, provider_request_id, operation, scope,
    status,
    CASE decision
        WHEN 'approved' THEN '{"kind":"approved"}'
        WHEN 'denied' THEN '{"kind":"denied"}'
        ELSE NULL
    END,
    created_at, updated_at
FROM approvals;

DROP TABLE approvals;
ALTER TABLE approvals_next RENAME TO approvals;

CREATE INDEX idx_approvals_pending
ON approvals(status, created_at)
WHERE status = 'pending';

CREATE UNIQUE INDEX idx_approvals_provider_request
ON approvals(run_id, provider_request_id)
WHERE provider_request_id IS NOT NULL;
