-- The public hierarchy lives on conversations. Only observed children carry an
-- execution reference; root executions already belong to their run's conversation.
ALTER TABLE conversations ADD COLUMN observed_agent_id TEXT
REFERENCES agent_nodes(id) ON DELETE CASCADE;
ALTER TABLE conversations ADD COLUMN parent_id TEXT
REFERENCES conversations(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
CHECK ((parent_id IS NULL) = (observed_agent_id IS NULL))
CHECK (parent_id IS NULL OR workspace_id IS NULL);

CREATE UNIQUE INDEX idx_conversations_observed_agent ON conversations(observed_agent_id)
WHERE observed_agent_id IS NOT NULL;
CREATE INDEX idx_conversations_children
ON conversations(parent_id, created_at DESC, id DESC) WHERE status = 'active';
CREATE INDEX idx_events_agent_sequence ON events(agent_id, sequence DESC);
CREATE INDEX idx_approvals_agent_status_created
ON approvals(agent_id, status, created_at DESC, id DESC);

-- Refuse malformed history before publishing any hierarchy. UNION terminates
-- even on legacy cycles; every execution must be reachable from a valid root.
CREATE TABLE conversation_ancestry_guard (valid INTEGER NOT NULL CHECK (valid = 1));
WITH RECURSIVE rooted(id, run_id) AS (
    SELECT agent.id, agent.run_id FROM agent_nodes AS agent
    JOIN provider_runs AS run ON run.id = agent.run_id AND run.provider = agent.provider
    JOIN conversations AS owner ON owner.id = run.conversation_id
    WHERE agent.parent_id IS NULL
    UNION
    SELECT child.id, child.run_id FROM agent_nodes AS child
    JOIN rooted ON rooted.id = child.parent_id AND rooted.run_id = child.run_id
    JOIN provider_runs AS run ON run.id = child.run_id AND run.provider = child.provider
)
INSERT INTO conversation_ancestry_guard
SELECT (SELECT count(*) FROM rooted) = (SELECT count(*) FROM agent_nodes);

CREATE TABLE conversation_backfill (agent_id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL);
INSERT INTO conversation_backfill
SELECT agent.id, CASE WHEN agent.parent_id IS NULL THEN run.conversation_id
    ELSE lower(hex(randomblob(4)) || '-' || hex(randomblob(2)) || '-4' ||
         substr(hex(randomblob(2)), 2) || '-' || substr('89ab', 1 + (random() & 3), 1) ||
         substr(hex(randomblob(2)), 2) || '-' || hex(randomblob(6))) END
FROM agent_nodes AS agent JOIN provider_runs AS run ON run.id = agent.run_id;

-- Plain INSERT intentionally aborts on an ID collision; no historical row is
-- replaced. Parent foreign keys are deferred for arbitrary historical row order.
INSERT INTO conversations (id, parent_id, observed_agent_id, title, status, created_at, updated_at)
SELECT binding.conversation_id, parent.conversation_id, agent.id, agent.label,
       'active', agent.created_at, agent.updated_at
FROM agent_nodes AS agent
JOIN conversation_backfill AS binding ON binding.agent_id = agent.id
JOIN conversation_backfill AS parent ON parent.agent_id = agent.parent_id;
DROP TABLE conversation_backfill;
DROP TABLE conversation_ancestry_guard;

CREATE TRIGGER immutable_conversation_binding
BEFORE UPDATE OF id, parent_id, observed_agent_id ON conversations
WHEN NEW.id IS NOT OLD.id OR NEW.parent_id IS NOT OLD.parent_id
  OR NEW.observed_agent_id IS NOT OLD.observed_agent_id
BEGIN SELECT RAISE(ABORT, 'conversation binding is immutable'); END;

CREATE TRIGGER validate_conversation_binding
BEFORE INSERT ON conversations WHEN NEW.parent_id IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'conversation execution parent mismatch') WHERE NOT EXISTS (
        SELECT 1 FROM agent_nodes AS agent
        JOIN agent_nodes AS parent ON parent.id = agent.parent_id AND parent.run_id = agent.run_id
        JOIN provider_runs AS run ON run.id = agent.run_id
        JOIN conversations AS parent_conversation ON parent_conversation.id = NEW.parent_id
        WHERE agent.id = NEW.observed_agent_id AND agent.provider = run.provider
          AND ((parent.parent_id IS NULL AND parent_conversation.id = run.conversation_id
                AND parent_conversation.parent_id IS NULL)
               OR parent_conversation.observed_agent_id = parent.id)
    );
END;

CREATE TRIGGER immutable_agent_ancestry
BEFORE UPDATE OF id, run_id, parent_id, provider ON agent_nodes
WHEN NEW.id IS NOT OLD.id OR NEW.run_id IS NOT OLD.run_id OR NEW.parent_id IS NOT OLD.parent_id
  OR NEW.provider IS NOT OLD.provider
BEGIN SELECT RAISE(ABORT, 'execution ancestry is immutable'); END;

CREATE TRIGGER immutable_run_owner
BEFORE UPDATE OF id, conversation_id, provider ON provider_runs
WHEN NEW.id IS NOT OLD.id OR NEW.conversation_id IS NOT OLD.conversation_id
  OR NEW.provider IS NOT OLD.provider
BEGIN SELECT RAISE(ABORT, 'execution owner is immutable'); END;

-- One persistence helper for every root/child insertion, including fallback and
-- direct fixture inserts. It runs inside the caller's transaction, so any later
-- observation conflict rolls back both the agent and its conversation.
CREATE TRIGGER bind_agent_conversation AFTER INSERT ON agent_nodes
BEGIN
    SELECT RAISE(ABORT, 'execution conversation owner mismatch') WHERE NOT EXISTS (
        SELECT 1 FROM provider_runs AS run JOIN conversations AS owner ON owner.id = run.conversation_id
        WHERE run.id = NEW.run_id AND run.provider = NEW.provider AND owner.parent_id IS NULL
    );
    SELECT RAISE(ABORT, 'execution parent is missing or unbound')
    WHERE NEW.parent_id IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM agent_nodes AS parent
        WHERE parent.id = NEW.parent_id AND parent.id <> NEW.id AND parent.run_id = NEW.run_id
          AND (parent.parent_id IS NULL OR EXISTS (
              SELECT 1 FROM conversations WHERE observed_agent_id = parent.id
          ))
    );
    INSERT INTO conversations (id, parent_id, observed_agent_id, title, status, created_at, updated_at)
    SELECT lower(hex(randomblob(4)) || '-' || hex(randomblob(2)) || '-4' ||
           substr(hex(randomblob(2)), 2) || '-' || substr('89ab', 1 + (random() & 3), 1) ||
           substr(hex(randomblob(2)), 2) || '-' || hex(randomblob(6))),
           CASE WHEN parent.parent_id IS NULL THEN run.conversation_id
                ELSE (SELECT id FROM conversations WHERE observed_agent_id = parent.id) END,
           NEW.id, NEW.label, 'active', NEW.created_at, NEW.updated_at
    FROM agent_nodes AS parent JOIN provider_runs AS run ON run.id = NEW.run_id
    WHERE parent.id = NEW.parent_id;
END;
