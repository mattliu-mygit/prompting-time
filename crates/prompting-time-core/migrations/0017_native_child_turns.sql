-- Turn history is scoped to the canonical agent; ended IDs are never reusable.
CREATE TABLE native_child_turns (
    agent_id TEXT NOT NULL REFERENCES agent_nodes(id) ON DELETE CASCADE,
    native_turn_id TEXT NOT NULL CHECK(length(native_turn_id) BETWEEN 1 AND 256),
    started_event_id TEXT NOT NULL REFERENCES events(id) ON DELETE CASCADE,
    ended INTEGER NOT NULL DEFAULT 0 CHECK(ended IN (0, 1)),
    PRIMARY KEY (agent_id, native_turn_id)
);

CREATE UNIQUE INDEX idx_native_child_turn_active
ON native_child_turns(agent_id) WHERE ended = 0;

-- Root/legacy approvals have no row here. A native child control retains its
-- original turn even after that child begins a later turn.
CREATE UNIQUE INDEX idx_approvals_native_child_owner ON approvals(id, agent_id);

CREATE TABLE native_child_controls (
    approval_id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    native_turn_id TEXT NOT NULL,
    FOREIGN KEY (approval_id, agent_id)
        REFERENCES approvals(id, agent_id) ON DELETE CASCADE,
    FOREIGN KEY (agent_id, native_turn_id)
        REFERENCES native_child_turns(agent_id, native_turn_id) ON DELETE CASCADE
);
