CREATE TABLE agent_bridge_execution_relations (
    child_id TEXT PRIMARY KEY NOT NULL REFERENCES agent_bridge_runs(id),
    parent_id TEXT NOT NULL REFERENCES agent_bridge_runs(id),
    relation_json TEXT NOT NULL,
    CHECK (child_id <> parent_id)
);
CREATE INDEX agent_bridge_execution_parent ON agent_bridge_execution_relations(parent_id, child_id);
