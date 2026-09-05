CREATE TABLE agent_bridge_resource_blobs (
    sha256 TEXT PRIMARY KEY NOT NULL,
    bytes BLOB NOT NULL
);

CREATE TABLE agent_bridge_resource_versions (
    resource_id TEXT NOT NULL,
    revision TEXT NOT NULL,
    media_type TEXT NOT NULL,
    sha256 TEXT NOT NULL REFERENCES agent_bridge_resource_blobs(sha256),
    PRIMARY KEY (resource_id, revision)
);
