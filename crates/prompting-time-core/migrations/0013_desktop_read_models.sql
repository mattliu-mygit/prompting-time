ALTER TABLE events ADD COLUMN role TEXT CHECK (role IS NULL OR role IN ('user', 'assistant'));
ALTER TABLE agent_nodes ADD COLUMN depth INTEGER NOT NULL DEFAULT 0 CHECK (depth >= 0);
ALTER TABLE provider_runs ADD COLUMN agent_total_count INTEGER NOT NULL DEFAULT 0 CHECK (agent_total_count >= 0);
ALTER TABLE provider_runs ADD COLUMN active_descendant_count INTEGER NOT NULL DEFAULT 0 CHECK (active_descendant_count >= 0);
ALTER TABLE provider_runs ADD COLUMN waiting_agent_count INTEGER NOT NULL DEFAULT 0 CHECK (waiting_agent_count >= 0);
ALTER TABLE provider_runs ADD COLUMN active_agent_count INTEGER NOT NULL DEFAULT 0 CHECK (active_agent_count >= 0);
ALTER TABLE provider_runs ADD COLUMN failed_agent_count INTEGER NOT NULL DEFAULT 0 CHECK (failed_agent_count >= 0);
ALTER TABLE provider_runs ADD COLUMN interrupted_agent_count INTEGER NOT NULL DEFAULT 0 CHECK (interrupted_agent_count >= 0);
ALTER TABLE approvals ADD COLUMN question_count INTEGER NOT NULL DEFAULT 0 CHECK (question_count >= 0);

UPDATE events SET role = 'assistant' WHERE kind = 'message';

WITH RECURSIVE tree(id, depth) AS (
    SELECT id, 0 FROM agent_nodes WHERE parent_id IS NULL
    UNION ALL
    SELECT child.id, tree.depth + 1
    FROM agent_nodes AS child JOIN tree ON child.parent_id = tree.id
)
UPDATE agent_nodes SET depth = (SELECT depth FROM tree WHERE tree.id = agent_nodes.id);

UPDATE provider_runs SET
    agent_total_count = (SELECT count(*) FROM agent_nodes WHERE run_id = provider_runs.id),
    active_descendant_count = (
        SELECT count(*) FROM agent_nodes
        WHERE run_id = provider_runs.id AND parent_id IS NOT NULL
          AND status IN ('queued', 'running', 'waiting')
    ),
    waiting_agent_count = (
        SELECT count(*) FROM agent_nodes WHERE run_id = provider_runs.id AND status = 'waiting'
    ),
    active_agent_count = (
        SELECT count(*) FROM agent_nodes
        WHERE run_id = provider_runs.id AND status IN ('queued', 'running')
    ),
    failed_agent_count = (
        SELECT count(*) FROM agent_nodes WHERE run_id = provider_runs.id AND status = 'failed'
    ),
    interrupted_agent_count = (
        SELECT count(*) FROM agent_nodes WHERE run_id = provider_runs.id AND status = 'interrupted'
    );

CREATE TRIGGER rollup_agent_insert AFTER INSERT ON agent_nodes BEGIN
    UPDATE provider_runs SET
        agent_total_count = agent_total_count + 1,
        active_descendant_count = active_descendant_count + (NEW.parent_id IS NOT NULL AND NEW.status IN ('queued', 'running', 'waiting')),
        waiting_agent_count = waiting_agent_count + (NEW.status = 'waiting'),
        active_agent_count = active_agent_count + (NEW.status IN ('queued', 'running')),
        failed_agent_count = failed_agent_count + (NEW.status = 'failed'),
        interrupted_agent_count = interrupted_agent_count + (NEW.status = 'interrupted')
    WHERE id = NEW.run_id;
END;

CREATE TRIGGER rollup_agent_update AFTER UPDATE OF status, parent_id ON agent_nodes BEGIN
    UPDATE provider_runs SET
        active_descendant_count = active_descendant_count
            - (OLD.parent_id IS NOT NULL AND OLD.status IN ('queued', 'running', 'waiting'))
            + (NEW.parent_id IS NOT NULL AND NEW.status IN ('queued', 'running', 'waiting')),
        waiting_agent_count = waiting_agent_count - (OLD.status = 'waiting') + (NEW.status = 'waiting'),
        active_agent_count = active_agent_count - (OLD.status IN ('queued', 'running')) + (NEW.status IN ('queued', 'running')),
        failed_agent_count = failed_agent_count - (OLD.status = 'failed') + (NEW.status = 'failed'),
        interrupted_agent_count = interrupted_agent_count - (OLD.status = 'interrupted') + (NEW.status = 'interrupted')
    WHERE id = NEW.run_id;
END;

CREATE TRIGGER rollup_agent_delete AFTER DELETE ON agent_nodes BEGIN
    UPDATE provider_runs SET
        agent_total_count = agent_total_count - 1,
        active_descendant_count = active_descendant_count - (OLD.parent_id IS NOT NULL AND OLD.status IN ('queued', 'running', 'waiting')),
        waiting_agent_count = waiting_agent_count - (OLD.status = 'waiting'),
        active_agent_count = active_agent_count - (OLD.status IN ('queued', 'running')),
        failed_agent_count = failed_agent_count - (OLD.status = 'failed'),
        interrupted_agent_count = interrupted_agent_count - (OLD.status = 'interrupted')
    WHERE id = OLD.run_id;
END;

CREATE INDEX idx_runs_conversation_latest
ON provider_runs(conversation_id, created_at DESC, id DESC);

CREATE INDEX idx_approvals_conversation_status_created
ON approvals(run_id, status, created_at DESC, id DESC);

CREATE INDEX idx_agents_run_page
ON agent_nodes(run_id, created_at, id);

CREATE INDEX idx_agents_recovery_depth
ON agent_nodes(depth DESC, created_at, id)
WHERE status IN ('queued', 'running', 'waiting');

CREATE TABLE approval_questions (
    approval_id TEXT NOT NULL REFERENCES approvals(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    header TEXT NOT NULL,
    question TEXT NOT NULL,
    options_json TEXT,
    is_other INTEGER NOT NULL CHECK (is_other IN (0, 1)),
    is_secret INTEGER NOT NULL CHECK (is_secret IN (0, 1)),
    source_bytes INTEGER NOT NULL CHECK (source_bytes >= 0),
    header_bytes INTEGER NOT NULL CHECK (header_bytes >= 0),
    question_bytes INTEGER NOT NULL CHECK (question_bytes >= 0),
    PRIMARY KEY (approval_id, ordinal)
) WITHOUT ROWID;

UPDATE approvals
SET question_count = json_array_length(request_json, '$.questions')
WHERE json_valid(request_json)
  AND json_type(request_json, '$.questions') = 'array';

INSERT INTO approval_questions (
    approval_id, ordinal, header, question, options_json, is_other, is_secret,
    source_bytes, header_bytes, question_bytes
)
SELECT approvals.id,
       CAST(input_question.key AS INTEGER),
       substr(json_extract(input_question.value, '$.header'), 1, 256),
       substr(json_extract(input_question.value, '$.question'), 1, 2048),
       CASE WHEN length(CAST(json_extract(input_question.value, '$.options') AS BLOB)) <= 4096
            THEN json_extract(input_question.value, '$.options') END,
       COALESCE(json_extract(input_question.value, '$.isOther'), 0),
       COALESCE(json_extract(input_question.value, '$.isSecret'), 0),
       length(CAST(input_question.value AS BLOB)),
       length(CAST(json_extract(input_question.value, '$.header') AS BLOB)),
       length(CAST(json_extract(input_question.value, '$.question') AS BLOB))
FROM approvals
JOIN json_each(approvals.request_json, '$.questions') AS input_question
WHERE json_valid(approvals.request_json)
  AND json_type(approvals.request_json, '$.questions') = 'array';
