CREATE TABLE conversations (
    id TEXT PRIMARY KEY NOT NULL,
    title TEXT NOT NULL,
    workspace_id TEXT UNIQUE,
    status TEXT NOT NULL CHECK (status IN ('active', 'archived')),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY (workspace_id, id) REFERENCES workspaces(id, conversation_id) DEFERRABLE INITIALLY DEFERRED
);

CREATE TABLE workspaces (
    id TEXT PRIMARY KEY NOT NULL,
    conversation_id TEXT NOT NULL UNIQUE REFERENCES conversations(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
    project_root TEXT,
    execution_path TEXT NOT NULL,
    owned_worktree INTEGER NOT NULL CHECK (owned_worktree IN (0, 1)),
    worktree_base_commit TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK (
        (owned_worktree = 1 AND project_root IS NOT NULL AND worktree_base_commit IS NOT NULL)
        OR (owned_worktree = 0 AND worktree_base_commit IS NULL)
    ),
    UNIQUE (id, conversation_id)
);

CREATE TABLE provider_sessions (
    id TEXT PRIMARY KEY NOT NULL,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    provider TEXT NOT NULL CHECK (provider IN ('codex', 'claude')),
    native_session_id TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (conversation_id, provider),
    UNIQUE (id, conversation_id, provider)
);

CREATE TABLE provider_runs (
    id TEXT PRIMARY KEY NOT NULL,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    provider_session_id TEXT,
    provider TEXT NOT NULL CHECK (provider IN ('codex', 'claude')),
    native_session_id TEXT,
    status TEXT NOT NULL CHECK (status IN ('queued', 'running', 'waiting', 'completed', 'interrupted', 'failed')),
    mutation_state TEXT NOT NULL CHECK (mutation_state IN ('none_observed', 'observed', 'unknown')),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (id, conversation_id),
    FOREIGN KEY (provider_session_id, conversation_id, provider) REFERENCES provider_sessions(id, conversation_id, provider)
);

CREATE TABLE agent_nodes (
    id TEXT PRIMARY KEY NOT NULL,
    run_id TEXT NOT NULL REFERENCES provider_runs(id) ON DELETE CASCADE,
    parent_id TEXT,
    provider TEXT NOT NULL CHECK (provider IN ('codex', 'claude')),
    label TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('queued', 'running', 'waiting', 'completed', 'interrupted', 'failed')),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (id, run_id),
    FOREIGN KEY (parent_id, run_id) REFERENCES agent_nodes(id, run_id) ON DELETE CASCADE
);

CREATE TABLE messages (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    run_id TEXT,
    role TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
    content TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    FOREIGN KEY (run_id, conversation_id) REFERENCES provider_runs(id, conversation_id)
);

CREATE TABLE events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    conversation_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('message', 'tool', 'progress', 'diagnostic', 'lifecycle')),
    content TEXT NOT NULL,
    payload_json TEXT CHECK (payload_json IS NULL OR json_valid(payload_json)),
    created_at INTEGER NOT NULL,
    FOREIGN KEY (run_id, conversation_id) REFERENCES provider_runs(id, conversation_id) ON DELETE CASCADE,
    FOREIGN KEY (agent_id, run_id) REFERENCES agent_nodes(id, run_id) ON DELETE CASCADE
);

CREATE TABLE approvals (
    id TEXT PRIMARY KEY NOT NULL,
    run_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    provider TEXT NOT NULL CHECK (provider IN ('codex', 'claude')),
    operation TEXT NOT NULL,
    scope TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'approved', 'denied')),
    decision TEXT CHECK (decision IS NULL OR decision IN ('approved', 'denied')),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY (agent_id, run_id) REFERENCES agent_nodes(id, run_id) ON DELETE CASCADE
);

CREATE TABLE routing_decisions (
    id TEXT PRIMARY KEY NOT NULL,
    run_id TEXT NOT NULL UNIQUE REFERENCES provider_runs(id) ON DELETE CASCADE,
    chosen_provider TEXT NOT NULL CHECK (chosen_provider IN ('codex', 'claude')),
    details_json TEXT NOT NULL CHECK (json_valid(details_json)),
    created_at INTEGER NOT NULL
);

-- SQLite applies SET NULL to every column of a composite foreign key. These triggers preserve
-- required aggregate keys while clearing only the optional reference before its parent is deleted.
CREATE TRIGGER clear_provider_session_from_runs
BEFORE DELETE ON provider_sessions
FOR EACH ROW
BEGIN
    UPDATE provider_runs SET provider_session_id = NULL WHERE provider_session_id = OLD.id;
END;

CREATE TRIGGER clear_run_from_messages
BEFORE DELETE ON provider_runs
FOR EACH ROW
BEGIN
    UPDATE messages SET run_id = NULL WHERE run_id = OLD.id;
END;

CREATE INDEX idx_conversations_status_updated ON conversations(status, updated_at DESC, id DESC);
CREATE INDEX idx_conversations_workspace_updated ON conversations(workspace_id, updated_at DESC, id DESC);
CREATE INDEX idx_runs_conversation_created ON provider_runs(conversation_id, created_at, id);
CREATE INDEX idx_agents_run_parent ON agent_nodes(run_id, parent_id);
CREATE INDEX idx_events_conversation_sequence ON events(conversation_id, sequence);
CREATE INDEX idx_approvals_pending ON approvals(status, created_at) WHERE status = 'pending';
CREATE INDEX idx_conversations_updated ON conversations(updated_at DESC, id DESC);
CREATE INDEX idx_runs_status_created ON provider_runs(status, created_at, id);
CREATE INDEX idx_runs_session_ownership ON provider_runs(provider_session_id, conversation_id, provider);
CREATE INDEX idx_agents_run_created ON agent_nodes(run_id, created_at, id);
CREATE INDEX idx_events_run_sequence ON events(run_id, sequence DESC);
CREATE INDEX idx_messages_run_ownership ON messages(run_id, conversation_id);
