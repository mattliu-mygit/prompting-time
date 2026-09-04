CREATE TABLE staged_provider_events_v6 (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    conversation_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('message', 'progress', 'tool', 'diagnostic')),
    content TEXT NOT NULL,
    mutation_state TEXT CHECK (
        mutation_state IS NULL OR mutation_state IN ('none_observed', 'observed', 'unknown')
    ),
    overflowed_kind TEXT CHECK (
        overflowed_kind IS NULL OR overflowed_kind IN ('message', 'progress', 'tool')
    ),
    created_at INTEGER NOT NULL,
    CHECK (
        (overflowed_kind IS NOT NULL AND kind = 'diagnostic' AND mutation_state = 'unknown')
        OR
        (
            overflowed_kind IS NULL
            AND (
                (kind = 'tool' AND mutation_state IS NOT NULL)
                OR (kind IN ('message', 'progress') AND mutation_state IS NULL)
            )
        )
    ),
    FOREIGN KEY (run_id, conversation_id) REFERENCES provider_runs(id, conversation_id) ON DELETE CASCADE,
    FOREIGN KEY (agent_id, run_id) REFERENCES agent_nodes(id, run_id) ON DELETE CASCADE
);

-- Retain the longest receipt-order prefix that leaves room for one compact overflow marker.
WITH ranked AS (
    SELECT *,
           row_number() OVER (PARTITION BY run_id ORDER BY sequence, id) AS row_number,
           sum(length(CAST(content AS BLOB))) OVER (
               PARTITION BY run_id ORDER BY sequence, id
           ) AS cumulative_bytes
    FROM staged_provider_events
)
INSERT INTO staged_provider_events_v6 (
    sequence, id, conversation_id, run_id, agent_id, kind, content,
    mutation_state, overflowed_kind, created_at
)
SELECT sequence, id, conversation_id, run_id, agent_id, kind, content,
       mutation_state, NULL, created_at
FROM ranked
WHERE row_number <= 256
  AND cumulative_bytes <= 8388556;

-- Replace all omitted evidence for each run with exactly one bounded marker. The marker reuses
-- the first omitted row's identity and receipt position, so ordering remains deterministic.
WITH ranked AS (
    SELECT *,
           row_number() OVER (PARTITION BY run_id ORDER BY sequence, id) AS row_number,
           sum(length(CAST(content AS BLOB))) OVER (
               PARTITION BY run_id ORDER BY sequence, id
           ) AS cumulative_bytes
    FROM staged_provider_events
), omitted AS (
    SELECT *,
           row_number() OVER (PARTITION BY run_id ORDER BY sequence, id) AS omitted_number
    FROM ranked
    WHERE row_number > 256
       OR cumulative_bytes > 8388556
)
INSERT INTO staged_provider_events_v6 (
    sequence, id, conversation_id, run_id, agent_id, kind, content,
    mutation_state, overflowed_kind, created_at
)
SELECT sequence, id, conversation_id, run_id, agent_id, 'diagnostic',
       'Provider output omitted: staged queue limit exceeded', 'unknown',
       CASE kind
           WHEN 'message' THEN 'message'
           WHEN 'tool' THEN 'tool'
           ELSE 'progress'
       END,
       created_at
FROM omitted
WHERE omitted_number = 1;

UPDATE provider_runs
SET mutation_state = 'unknown'
WHERE id IN (
    SELECT DISTINCT run_id
    FROM staged_provider_events_v6
    WHERE overflowed_kind IS NOT NULL
);

DROP TABLE staged_provider_events;
ALTER TABLE staged_provider_events_v6 RENAME TO staged_provider_events;

CREATE INDEX idx_staged_provider_events_run_sequence
ON staged_provider_events(run_id, sequence);
