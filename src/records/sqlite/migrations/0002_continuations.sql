CREATE TABLE agent_bridge_continuations (
    id TEXT PRIMARY KEY NOT NULL,
    session_id TEXT NOT NULL REFERENCES agent_bridge_sessions(id),
    adapter TEXT NOT NULL,
    scope TEXT NOT NULL,
    native_key TEXT NOT NULL,
    predecessor_id TEXT UNIQUE REFERENCES agent_bridge_continuations(id),
    descriptor_json TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('available', 'claimed')),
    latest INTEGER NOT NULL CHECK (latest IN (0, 1))
);

CREATE UNIQUE INDEX agent_bridge_continuation_latest
    ON agent_bridge_continuations(adapter, scope, native_key) WHERE latest = 1;
