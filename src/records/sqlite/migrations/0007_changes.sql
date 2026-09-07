CREATE TABLE agent_bridge_change_clock (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    epoch TEXT NOT NULL,
    position INTEGER NOT NULL CHECK (typeof(position) = 'integer' AND position >= 0)
);
INSERT INTO agent_bridge_change_clock VALUES (1, lower(hex(randomblob(16))), 0);
CREATE TABLE agent_bridge_record_changes (
    record_id TEXT PRIMARY KEY REFERENCES agent_bridge_records(id),
    session_id TEXT NOT NULL REFERENCES agent_bridge_sessions(id),
    position INTEGER NOT NULL UNIQUE
);
INSERT INTO agent_bridge_record_changes
SELECT id, session_id, row_number() OVER (ORDER BY session_id, sequence) FROM agent_bridge_records;
CREATE INDEX agent_bridge_changes_session ON agent_bridge_record_changes(session_id, position);
UPDATE agent_bridge_change_clock SET position = (SELECT count(*) FROM agent_bridge_records);

CREATE TRIGGER agent_bridge_record_insert_change AFTER INSERT ON agent_bridge_records BEGIN
    UPDATE agent_bridge_change_clock SET position = position + 1 WHERE id = 1;
    INSERT INTO agent_bridge_record_changes VALUES (NEW.id, NEW.session_id, (SELECT position FROM agent_bridge_change_clock WHERE id = 1));
END;
CREATE TRIGGER agent_bridge_record_update_change AFTER UPDATE OF revision ON agent_bridge_records
WHEN NEW.revision != OLD.revision BEGIN
    UPDATE agent_bridge_change_clock SET position = position + 1 WHERE id = 1;
    UPDATE agent_bridge_record_changes SET position = (SELECT position FROM agent_bridge_change_clock WHERE id = 1) WHERE record_id = NEW.id;
END;
