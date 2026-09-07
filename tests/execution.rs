#![cfg(feature = "records")]
use agent_bridge::execution::*;
use agent_bridge::records::*;
use agent_bridge::*;
use std::sync::Arc;

fn spec(id: &str) -> RunSpec {
    RunSpec {
        id: RunId::new(id).unwrap(),
        session_id: SessionId::new("session").unwrap(),
        slot_id: SlotId::new(id).unwrap(),
        context: Default::default(),
        config: Default::default(),
        continuation: None,
    }
}
fn authority(run: &RunSpec) -> ToolGrant {
    ToolGrant {
        issuer: ActorId::new("host").unwrap(),
        subject: ActorId::new(run.id.as_str()).unwrap(),
        scope: ToolScope {
            session: run.session_id.clone(),
            slot: run.slot_id.clone(),
        },
        tools: vec![ToolRef {
            name: "lookup".into(),
            revision: "v1".into(),
        }],
    }
}
fn delegation(parent: &RunSpec) -> DelegationGrant {
    let grant = authority(parent);
    DelegationGrant {
        issuer: grant.issuer,
        subject: grant.subject,
        parent: parent.id.clone(),
        context: parent.context.clone(),
        tools: grant.tools,
    }
}
fn request(child: &RunSpec) -> ChildRequest {
    ChildRequest {
        id: child.id.clone(),
        slot: child.slot_id.clone(),
        actor: ActorId::new(child.id.as_str()).unwrap(),
        context: child.context.clone(),
        tools: vec![],
    }
}
fn graph_contract(store: Arc<dyn ExecutionStore>) {
    let parent = spec("parent");
    let child = spec("child");
    store.create_session(parent.session_id.clone()).unwrap();
    store.register_run(parent.clone()).unwrap();
    let mut child_request = request(&child);
    child_request.tools = authority(&parent).tools;
    let plan = ChildPlan::new(
        store.as_ref(),
        authority(&parent),
        delegation(&parent),
        child_request,
    )
    .unwrap();
    assert_eq!(plan.authority().tools, authority(&parent).tools);
    let relation = plan.validate_run(store.as_ref(), &child).unwrap();
    assert!(matches!(
        store.link_execution(relation.clone()),
        Err(StoreError::MissingRun)
    ));
    store.register_run(child.clone()).unwrap();
    let saved = store.link_execution(relation.clone()).unwrap();
    assert_eq!(*store.link_execution(relation.clone()).unwrap(), *saved);
    assert_eq!(
        *store.execution_parent(&child.id).unwrap().unwrap(),
        relation
    );
    assert_eq!(store.execution_children(&parent.id).unwrap().len(), 1);
    let mut conflict = relation;
    conflict.child_authority.subject = ActorId::new("someone-else").unwrap();
    assert!(matches!(
        store.link_execution(conflict),
        Err(StoreError::ExecutionRelationConflict)
    ));
    let mut reverse_request = request(&parent);
    reverse_request.tools = authority(&parent).tools;
    let reverse = ChildPlan::new(
        store.as_ref(),
        authority(&child),
        delegation(&child),
        reverse_request,
    )
    .unwrap();
    assert!(matches!(
        store.link_execution(reverse.validate_run(store.as_ref(), &parent).unwrap()),
        Err(StoreError::ExecutionCycle)
    ));
}
#[test]
fn memory_execution_relations_are_immutable_acyclic_and_require_registered_runs() {
    graph_contract(Arc::new(MemoryStore::default()));
}
#[cfg(feature = "sqlite")]
#[test]
fn sqlite_execution_relations_are_immutable_acyclic_and_require_registered_runs() {
    graph_contract(Arc::new(SqliteStore::open_in_memory().unwrap()));
}

#[test]
fn child_selection_cannot_exceed_delegation_or_parent_tool_authority() {
    let store = MemoryStore::default();
    let parent = spec("parent");
    let child = spec("child");
    store.create_session(parent.session_id.clone()).unwrap();
    store.register_run(parent.clone()).unwrap();
    for field in [
        "context", "tools", "issuer", "subject", "parent", "scope", "self",
    ] {
        let mut parent_authority = authority(&parent);
        let mut permission = delegation(&parent);
        let mut child_request = request(&child);
        match field {
            "context" => child_request
                .context
                .records
                .push(RecordId::new("not-granted").unwrap()),
            "tools" => {
                let tool = ToolRef {
                    name: "lookup".into(),
                    revision: "v2".into(),
                };
                permission.tools.push(tool.clone());
                child_request.tools.push(tool);
            }
            "issuer" => permission.issuer = ActorId::new("wrong").unwrap(),
            "subject" => permission.subject = ActorId::new("wrong").unwrap(),
            "parent" => permission.parent = RunId::new("missing-parent").unwrap(),
            "scope" => parent_authority.scope.slot = SlotId::new("wrong").unwrap(),
            _ => child_request.id = parent.id.clone(),
        }
        assert!(
            ChildPlan::new(&store, parent_authority, permission, child_request).is_err(),
            "{field}"
        );
    }
    let mut narrower = delegation(&parent);
    narrower.tools.clear();
    let mut child_request = request(&child);
    child_request.tools = authority(&parent).tools;
    assert!(ChildPlan::new(&store, authority(&parent), narrower, child_request).is_err());
}

#[test]
fn a_child_cannot_replace_its_inherited_tool_grant_for_further_delegation() {
    let store = MemoryStore::default();
    let parent = spec("parent");
    let child = spec("child");
    let grandchild = spec("grandchild");
    store.create_session(parent.session_id.clone()).unwrap();
    store.register_run(parent.clone()).unwrap();
    store.register_run(child.clone()).unwrap();
    let plan = ChildPlan::new(
        &store,
        authority(&parent),
        delegation(&parent),
        request(&child),
    )
    .unwrap();
    assert!(plan.authority().tools.is_empty());
    store
        .link_execution(plan.validate_run(&store, &child).unwrap())
        .unwrap();
    assert!(
        ChildPlan::new(
            &store,
            authority(&child),
            delegation(&child),
            request(&grandchild)
        )
        .is_err()
    );
    let mut requested = request(&grandchild);
    requested.tools = authority(&child).tools;
    assert!(
        ChildPlan::new(
            &store,
            plan.authority().clone(),
            delegation(&child),
            requested
        )
        .is_err()
    );
}

#[cfg(feature = "sqlite")]
#[test]
fn execution_relations_survive_reopen_and_detect_identity_corruption() {
    let path = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("execution-{}.sqlite3", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let store = SqliteStore::open(&path).unwrap();
    let parent = spec("parent");
    let child = spec("child");
    store.create_session(parent.session_id.clone()).unwrap();
    store.register_run(parent.clone()).unwrap();
    store.register_run(child.clone()).unwrap();
    let plan = ChildPlan::new(
        &store,
        authority(&parent),
        delegation(&parent),
        request(&child),
    )
    .unwrap();
    store
        .link_execution(plan.validate_run(&store, &child).unwrap())
        .unwrap();
    drop(store);
    let reopened = SqliteStore::open(&path).unwrap();
    assert_eq!(
        reopened
            .execution_parent(&child.id)
            .unwrap()
            .unwrap()
            .parent,
        parent.id
    );
    let sql = rusqlite::Connection::open(&path).unwrap();
    sql.execute("UPDATE agent_bridge_execution_relations SET relation_json=json_set(relation_json,'$.data.child','wrong')", []).unwrap();
    assert!(matches!(
        reopened.execution_parent(&child.id),
        Err(StoreError::CorruptData(_))
    ));
    drop(sql);
    drop(reopened);
    std::fs::remove_file(path).unwrap();
}
