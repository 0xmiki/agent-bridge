use super::*;
use crate::execution::{ExecutionRelation, ExecutionStore};

fn read(
    connection: &Connection,
    child: &RunId,
) -> Result<Option<Arc<ExecutionRelation>>, StoreError> {
    let row: Option<(String, String)> = connection.query_row("SELECT parent_id, relation_json FROM agent_bridge_execution_relations WHERE child_id=?1", [child.as_str()], |row| Ok((row.get(0)?, row.get(1)?))).optional().map_err(database_error)?;
    row.map(|(parent, document)| {
        let relation: ExecutionRelation = codec::decode(&document)?;
        if relation.child != *child || relation.parent.as_str() != parent {
            return Err(StoreError::CorruptData(
                "execution relation identity mismatch".into(),
            ));
        }
        let parent = read_run(connection, &relation.parent)?.ok_or(StoreError::MissingRun)?;
        let child = read_run(connection, child)?.ok_or(StoreError::MissingRun)?;
        relation
            .validate(&parent, &child)
            .map_err(|_| StoreError::CorruptData("invalid execution relation".into()))?;
        Ok(Arc::new(relation))
    })
    .transpose()
}
impl ExecutionStore for SqliteStore {
    fn link_execution(
        &self,
        relation: ExecutionRelation,
    ) -> Result<Arc<ExecutionRelation>, StoreError> {
        self.write(|connection| {
            if let Some(existing) = read(connection, &relation.child)? {
                return if *existing == relation { Ok(existing) } else { Err(StoreError::ExecutionRelationConflict) };
            }
            let parent = read_run(connection, &relation.parent)?.ok_or(StoreError::MissingRun)?;
            let child = read_run(connection, &relation.child)?.ok_or(StoreError::MissingRun)?;
            relation.validate(&parent, &child)?;
            if read(connection, &relation.parent)?.is_some_and(|edge| edge.child_authority != relation.parent_authority) { return Err(StoreError::InvalidExecutionRelation); }
            let mut children = connection.prepare("SELECT child_id FROM agent_bridge_execution_relations WHERE parent_id=?1").map_err(database_error)?;
            let descendants = children.query_map([relation.child.as_str()], |row| row.get::<_, String>(0)).map_err(database_error)?.collect::<Result<Vec<_>, _>>().map_err(database_error)?;
            for descendant in descendants {
                let id = RunId::new(descendant).map_err(|e| StoreError::CorruptData(e.to_string()))?;
                if read(connection, &id)?.is_some_and(|edge| edge.parent_authority != relation.child_authority) { return Err(StoreError::InvalidExecutionRelation); }
            }
            for id in &child.context.records {
                if entry(connection, id)?.ok_or(StoreError::MissingRecord)?.current.record.session_id != child.session_id { return Err(StoreError::WrongSession); }
            }
            let mut ancestor = relation.parent.clone(); let mut visited = std::collections::HashSet::new();
            loop {
                if ancestor == relation.child || !visited.insert(ancestor.clone()) { return Err(StoreError::ExecutionCycle); }
                match read(connection, &ancestor)? { Some(edge) => ancestor = edge.parent.clone(), None => break }
            }
            connection.execute("INSERT INTO agent_bridge_execution_relations(child_id,parent_id,relation_json) VALUES (?1,?2,?3)", params![relation.child.as_str(), relation.parent.as_str(), codec::encode(&relation)?]).map_err(database_error)?;
            Ok(Arc::new(relation))
        })
    }
    fn execution_parent(
        &self,
        child: &RunId,
    ) -> Result<Option<Arc<ExecutionRelation>>, StoreError> {
        let connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        read(&connection, child)
    }
    fn execution_children(
        &self,
        parent: &RunId,
    ) -> Result<Vec<Arc<ExecutionRelation>>, StoreError> {
        let connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        let mut statement = connection.prepare("SELECT child_id FROM agent_bridge_execution_relations WHERE parent_id=?1 ORDER BY child_id").map_err(database_error)?;
        let ids = statement
            .query_map([parent.as_str()], |row| row.get::<_, String>(0))
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?;
        ids.into_iter()
            .map(|child| {
                read(
                    &connection,
                    &RunId::new(child).map_err(|e| StoreError::CorruptData(e.to_string()))?,
                )?
                .ok_or(StoreError::MissingRun)
            })
            .collect()
    }
}
