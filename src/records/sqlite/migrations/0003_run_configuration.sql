ALTER TABLE agent_bridge_runs ADD COLUMN config_json TEXT;
ALTER TABLE agent_bridge_runs ADD COLUMN continuation_id TEXT REFERENCES agent_bridge_continuations(id);
CREATE INDEX agent_bridge_runs_continuation ON agent_bridge_runs(continuation_id);
