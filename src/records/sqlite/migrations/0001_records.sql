CREATE TABLE agent_bridge_schema (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    version INTEGER NOT NULL CHECK (version >= 1)
);

CREATE TABLE agent_bridge_sessions (
    id TEXT PRIMARY KEY NOT NULL,
    next_sequence INTEGER NOT NULL DEFAULT 0 CHECK (typeof(next_sequence) = 'integer' AND next_sequence >= 0)
);

CREATE TABLE agent_bridge_runs (
    id TEXT PRIMARY KEY NOT NULL,
    session_id TEXT NOT NULL REFERENCES agent_bridge_sessions(id),
    slot_id TEXT NOT NULL,
    context_json TEXT NOT NULL,
    UNIQUE (id, session_id)
);

CREATE TABLE agent_bridge_records (
    id TEXT PRIMARY KEY NOT NULL,
    session_id TEXT NOT NULL REFERENCES agent_bridge_sessions(id),
    run_id TEXT,
    sequence INTEGER NOT NULL CHECK (typeof(sequence) = 'integer' AND sequence >= 0),
    actor_id TEXT NOT NULL,
    reply_to_id TEXT REFERENCES agent_bridge_records(id),
    source_json TEXT,
    payload_json TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('open', 'complete', 'interrupted')),
    revision INTEGER NOT NULL DEFAULT 0 CHECK (typeof(revision) = 'integer' AND revision >= 0),
    initial_json TEXT,
    UNIQUE (session_id, sequence),
    FOREIGN KEY (run_id, session_id) REFERENCES agent_bridge_runs(id, session_id)
);

CREATE INDEX agent_bridge_records_run ON agent_bridge_records(run_id, sequence);

CREATE TABLE agent_bridge_decisions (
    request_id TEXT PRIMARY KEY NOT NULL REFERENCES agent_bridge_records(id),
    response_id TEXT NOT NULL UNIQUE REFERENCES agent_bridge_records(id)
);

INSERT INTO agent_bridge_schema (id, version) VALUES (1, 1);
