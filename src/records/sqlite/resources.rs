use super::*;
use crate::ResourceRef;
use crate::context::{Resource, ResourceArchive, ResourceError, ResourceStore};
use sha2::{Digest, Sha256};

fn error(error: rusqlite::Error) -> ResourceError {
    ResourceError::Store(database_error(error))
}

fn read(connection: &Connection, reference: &ResourceRef) -> Result<Arc<Resource>, ResourceError> {
    let (media_type, hash, bytes): (String, String, Option<Vec<u8>>) = connection.query_row(
        "SELECT v.media_type, v.sha256, b.bytes FROM agent_bridge_resource_versions v LEFT JOIN agent_bridge_resource_blobs b ON v.sha256 = b.sha256 WHERE v.resource_id = ?1 AND v.revision = ?2",
        params![reference.id.as_str(), reference.revision], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))
    ).optional().map_err(error)?.ok_or(ResourceError::Missing)?;
    let bytes = bytes.ok_or_else(|| {
        ResourceError::Store(StoreError::CorruptData("resource blob missing".into()))
    })?;
    if format!("{:x}", Sha256::digest(&bytes)) != hash {
        return Err(ResourceError::Store(StoreError::CorruptData(
            "resource digest mismatch".into(),
        )));
    }
    Ok(Arc::new(Resource {
        reference: reference.clone(),
        media_type,
        bytes: Arc::from(bytes),
    }))
}

impl ResourceStore for SqliteStore {
    fn get(&self, reference: &ResourceRef) -> Result<Arc<Resource>, ResourceError> {
        if reference.revision.trim().is_empty() {
            return Err(ResourceError::Invalid);
        }
        let connection = self
            .connection
            .lock()
            .map_err(|_| ResourceError::Poisoned)?;
        read(&connection, reference)
    }
}

impl ResourceArchive for SqliteStore {
    fn put(&self, resource: Resource) -> Result<Arc<Resource>, ResourceError> {
        if resource.reference.revision.trim().is_empty() || resource.media_type.trim().is_empty() {
            return Err(ResourceError::Invalid);
        }
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| ResourceError::Poisoned)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(error)?;
        match read(&transaction, &resource.reference) {
            Ok(existing) => {
                return if *existing == resource {
                    Ok(existing)
                } else {
                    Err(ResourceError::RevisionConflict)
                };
            }
            Err(ResourceError::Missing) => {}
            Err(error) => return Err(error),
        }
        let hash = format!("{:x}", Sha256::digest(&resource.bytes));
        transaction
            .execute(
                "INSERT OR IGNORE INTO agent_bridge_resource_blobs(sha256, bytes) VALUES (?1, ?2)",
                params![hash, resource.bytes.as_ref()],
            )
            .map_err(error)?;
        let bytes: Vec<u8> = transaction
            .query_row(
                "SELECT bytes FROM agent_bridge_resource_blobs WHERE sha256 = ?1",
                [&hash],
                |row| row.get(0),
            )
            .map_err(error)?;
        if bytes.as_slice() != resource.bytes.as_ref() {
            return Err(ResourceError::Store(StoreError::CorruptData(
                "resource blob conflicts with digest".into(),
            )));
        }
        transaction.execute("INSERT INTO agent_bridge_resource_versions(resource_id, revision, media_type, sha256) VALUES (?1, ?2, ?3, ?4)", params![resource.reference.id.as_str(), resource.reference.revision, resource.media_type, hash]).map_err(error)?;
        transaction.commit().map_err(error)?;
        Ok(Arc::new(resource))
    }
}
