CREATE TABLE approvals_v9 (
    id TEXT PRIMARY KEY NOT NULL,
    run_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    provider TEXT NOT NULL CHECK (provider IN ('codex', 'claude')),
    provider_request_id TEXT,
    operation TEXT NOT NULL,
    scope TEXT NOT NULL,
    request_json TEXT CHECK (request_json IS NULL OR json_valid(request_json)),
    status TEXT NOT NULL CHECK (
        status IN ('pending', 'approved', 'denied', 'answered', 'cancelled', 'failed')
    ),
    resolution_json TEXT CHECK (resolution_json IS NULL OR json_valid(resolution_json)),
    response_intent_json TEXT CHECK (
        response_intent_json IS NULL OR json_valid(response_intent_json)
    ),
    response_intent_status TEXT CHECK (
        response_intent_status IS NULL
        OR response_intent_status IN ('recorded', 'acknowledged', 'rejected', 'dispatch_unknown')
    ),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK (
        (status = 'pending' AND resolution_json IS NULL)
        OR
        (
            status != 'pending'
            AND resolution_json IS NOT NULL
            AND (
                (status = 'answered' AND json_extract(resolution_json, '$.kind') IN ('answer', 'answers'))
                OR (status != 'answered' AND json_extract(resolution_json, '$.kind') = status)
            )
        )
    ),
    CHECK (
        (response_intent_json IS NULL AND response_intent_status IS NULL)
        OR (response_intent_json IS NOT NULL AND response_intent_status IS NOT NULL)
    ),
    CHECK (
        response_intent_status != 'acknowledged'
        OR (status != 'pending' AND resolution_json = response_intent_json)
    ),
    FOREIGN KEY (agent_id, run_id) REFERENCES agent_nodes(id, run_id) ON DELETE CASCADE
);

INSERT INTO approvals_v9 (
    id, run_id, agent_id, provider, provider_request_id, operation, scope, request_json,
    status, resolution_json, response_intent_json, response_intent_status, created_at, updated_at
)
SELECT
    id, run_id, agent_id, provider, provider_request_id, operation, scope, NULL,
    status, resolution_json, response_intent_json, response_intent_status, created_at, updated_at
FROM approvals;

DROP TABLE approvals;
ALTER TABLE approvals_v9 RENAME TO approvals;

CREATE INDEX idx_approvals_pending
ON approvals(status, created_at)
WHERE status = 'pending';

CREATE UNIQUE INDEX idx_approvals_provider_request
ON approvals(run_id, provider_request_id)
WHERE provider_request_id IS NOT NULL;
