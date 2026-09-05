#![cfg(feature = "tools")]
use agent_bridge::tools::*;
use agent_bridge::{ActorId, SessionId, SlotId};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Input {
    value: u32,
}
fn binding() -> (ToolGrant, ToolInvocation) {
    let scope = ToolScope {
        session: SessionId::new("session").unwrap(),
        slot: SlotId::new("slot").unwrap(),
    };
    let host = ActorId::new("host").unwrap();
    let actor = ActorId::new("agent").unwrap();
    (
        ToolGrant {
            issuer: host.clone(),
            subject: actor.clone(),
            scope: scope.clone(),
            tools: vec![ToolRef {
                name: "double".into(),
                revision: "v1".into(),
            }],
        },
        ToolInvocation {
            host,
            actor,
            scope,
            cancellation: CancellationToken::new(),
        },
    )
}
fn registry(calls: Arc<AtomicUsize>) -> ToolRegistry {
    let mut registry = ToolRegistry::default();
    registry
        .register::<Input, _, _, _>(
            ToolRef {
                name: "double".into(),
                revision: "v1".into(),
            },
            "Double a number",
            move |_, input| {
                calls.fetch_add(1, Ordering::SeqCst);
                async move { Ok(json!({"value":u64::from(input.value)*2})) }
            },
        )
        .unwrap();
    registry
}

#[tokio::test]
async fn declarations_do_not_grant_execution_and_typed_inputs_are_checked() {
    let calls = Arc::new(AtomicUsize::new(0));
    let registry = registry(calls.clone());
    let (mut grant, invocation) = binding();
    grant.tools.clear();
    assert!(registry.catalog(&grant, &invocation).unwrap().is_empty());
    assert_eq!(
        registry
            .invoke("double", json!({"value":2}), &grant, invocation.clone())
            .await,
        Err(ToolError::NotGranted)
    );
    let (grant, _) = binding();
    let catalog = registry.catalog(&grant, &invocation).unwrap();
    assert_eq!(
        catalog[0].input_schema["properties"]["value"]["type"],
        "integer"
    );
    for input in [
        json!({"value":"2"}),
        json!({"value":2,"scope":"another-session"}),
        json!([]),
    ] {
        assert!(matches!(
            registry
                .invoke("double", input, &grant, invocation.clone())
                .await,
            Err(ToolError::InvalidArguments(_))
        ));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        registry
            .invoke("double", json!({"value":2}), &grant, invocation)
            .await
            .unwrap(),
        json!({"value":4})
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn grants_pin_issuer_subject_session_slot_and_revision() {
    let calls = Arc::new(AtomicUsize::new(0));
    let registry = registry(calls.clone());
    for field in ["issuer", "subject", "session", "slot", "revision"] {
        let (mut grant, invocation) = binding();
        match field {
            "issuer" => grant.issuer = ActorId::new("wrong").unwrap(),
            "subject" => grant.subject = ActorId::new("wrong").unwrap(),
            "session" => grant.scope.session = SessionId::new("wrong").unwrap(),
            "slot" => grant.scope.slot = SlotId::new("wrong").unwrap(),
            _ => grant.tools[0].revision = "v2".into(),
        }
        assert_eq!(
            registry
                .invoke("double", json!({"value":2}), &grant, invocation)
                .await,
            Err(ToolError::NotGranted)
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cancellation_does_not_start_new_work_and_drops_a_pending_handler() {
    let calls = Arc::new(AtomicUsize::new(0));
    let registry = registry(calls.clone());
    let (grant, invocation) = binding();
    invocation.cancellation.cancel();
    assert_eq!(
        registry
            .invoke("double", json!({"value":2}), &grant, invocation)
            .await,
        Err(ToolError::Cancelled)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let started = Arc::new(tokio::sync::Notify::new());
    let ended = Arc::new(AtomicUsize::new(0));
    struct Guard(Arc<AtomicUsize>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let mut registry = ToolRegistry::default();
    let signal = started.clone();
    let cleanup = ended.clone();
    registry
        .register::<Input, serde_json::Value, _, _>(
            ToolRef {
                name: "double".into(),
                revision: "v1".into(),
            },
            "Wait",
            move |_, _| {
                let signal = signal.clone();
                let cleanup = cleanup.clone();
                async move {
                    let _guard = Guard(cleanup);
                    signal.notify_one();
                    std::future::pending().await
                }
            },
        )
        .unwrap();
    let (grant, invocation) = binding();
    let cancel = invocation.cancellation.clone();
    let running = tokio::spawn(async move {
        registry
            .invoke("double", json!({"value":2}), &grant, invocation)
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    cancel.cancel();
    assert_eq!(running.await.unwrap(), Err(ToolError::Cancelled));
    assert_eq!(ended.load(Ordering::SeqCst), 1);
}

#[test]
fn duplicate_or_non_object_definitions_are_rejected() {
    let mut registry = registry(Arc::new(AtomicUsize::new(0)));
    assert_eq!(
        registry.register::<Input, _, _, _>(
            ToolRef {
                name: "double".into(),
                revision: "v2".into()
            },
            "Replacement",
            |_, input| async move { Ok(input.value) }
        ),
        Err(ToolError::DuplicateDefinition)
    );
    assert_eq!(
        registry.register::<String, _, _, _>(
            ToolRef {
                name: "string".into(),
                revision: "v1".into()
            },
            "Non object",
            |_, input| async move { Ok(input) }
        ),
        Err(ToolError::InvalidDefinition)
    );
}
