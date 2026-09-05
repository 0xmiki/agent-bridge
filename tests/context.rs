#![cfg(feature = "records")]
use agent_bridge::context::*;
use agent_bridge::records::*;
use agent_bridge::*;
use std::sync::Arc;

fn reference(revision: &str) -> ResourceRef {
    ResourceRef {
        id: ResourceId::new("instructions").unwrap(),
        revision: revision.into(),
    }
}
fn resource(revision: &str, bytes: &[u8]) -> Resource {
    Resource {
        reference: reference(revision),
        media_type: "text/plain".into(),
        bytes: Arc::from(bytes),
    }
}
fn limits() -> ContextLimits {
    ContextLimits {
        max_items: 10,
        max_resource_bytes: 100,
    }
}

#[test]
fn skill_text_requires_exact_authority_and_keeps_its_versioned_identity() {
    let records = MemoryStore::default();
    let resources = MemoryResourceStore::default();
    resources.put(resource("v1", b"skill version one")).unwrap();
    resources.put(resource("v2", b"skill version two")).unwrap();
    let manifest = ContextManifest::default();
    let skill = SkillRequest {
        resource: reference("v1"),
        delivery: SkillDelivery::SupplementalText,
    };
    let mut policy = ContextPolicy {
        skills: vec![skill.clone()],
        ..Default::default()
    };
    assert!(matches!(
        prepare_with_policy(&manifest, &records, &resources, &[], limits(), &policy),
        Err(ContextError::InstructionUnauthorized)
    ));
    let authorized = InstructionRef {
        resource: reference("v1"),
        role: InstructionRole::Supplemental,
    };
    policy.instruction_authorization =
        ContextPolicy::for_host(ActorId::new("host").unwrap(), vec![authorized.clone()])
            .instruction_authorization;
    let prepared =
        prepare_with_policy(&manifest, &records, &resources, &[], limits(), &policy).unwrap();
    assert!(prepared.requested_manifest.instructions.is_empty());
    assert_eq!(prepared.manifest.instructions, vec![authorized]);
    assert_eq!(prepared.policy.skills, vec![skill]);
    assert_eq!(prepared.resources[0].bytes.as_ref(), b"skill version one");
    policy.skills[0].resource.revision = "v2".into();
    assert!(matches!(
        prepare_with_policy(&manifest, &records, &resources, &[], limits(), &policy),
        Err(ContextError::InstructionUnauthorized)
    ));
}

#[test]
fn skill_omission_is_explicit_and_conflicting_selections_are_rejected() {
    let records = MemoryStore::default();
    let resources = MemoryResourceStore::default();
    let instruction = InstructionRef {
        resource: reference("v1"),
        role: InstructionRole::Supplemental,
    };
    let mut policy =
        ContextPolicy::for_host(ActorId::new("host").unwrap(), vec![instruction.clone()]);
    policy.skills.push(SkillRequest {
        resource: reference("v1"),
        delivery: SkillDelivery::Omit {
            reason: "not required".into(),
        },
    });
    let empty = ContextManifest::default();
    assert!(
        prepare_with_policy(&empty, &records, &resources, &[], limits(), &policy)
            .unwrap()
            .resources
            .is_empty()
    );
    let manifest = ContextManifest {
        resources: vec![reference("v1")],
        ..Default::default()
    };
    assert!(matches!(
        prepare_with_policy(&manifest, &records, &resources, &[], limits(), &policy),
        Err(ContextError::ReferencedOmission(_))
    ));
    policy.skills.push(policy.skills[0].clone());
    assert!(matches!(
        prepare_with_policy(&empty, &records, &resources, &[], limits(), &policy),
        Err(ContextError::InvalidSkillRequest)
    ));
    policy.skills.pop();
    policy.skills[0].delivery = SkillDelivery::Omit { reason: " ".into() };
    assert!(matches!(
        prepare_with_policy(&empty, &records, &resources, &[], limits(), &policy),
        Err(ContextError::InvalidSkillRequest)
    ));
    policy.skills[0].delivery = SkillDelivery::SupplementalText;
    policy.omissions.push(ContextOmission {
        item: ContextItem::Instruction(instruction.clone()),
        reason: "conflicting omission".into(),
    });
    let manifest = ContextManifest {
        instructions: vec![instruction],
        ..Default::default()
    };
    assert!(matches!(
        prepare_with_policy(&manifest, &records, &resources, &[], limits(), &policy),
        Err(ContextError::InvalidSkillRequest)
    ));
}

#[test]
fn instruction_authority_pins_requester_revision_and_role_even_for_omissions() {
    let records = MemoryStore::default();
    let resources = MemoryResourceStore::default();
    resources.put(resource("v1", b"guidance")).unwrap();
    resources.put(resource("v2", b"changed guidance")).unwrap();
    let instruction = InstructionRef {
        resource: reference("v1"),
        role: InstructionRole::Supplemental,
    };
    let mut manifest = ContextManifest {
        instructions: vec![instruction.clone()],
        ..Default::default()
    };
    let mut policy =
        ContextPolicy::for_host(ActorId::new("host").unwrap(), vec![instruction.clone()]);
    assert!(prepare_with_policy(&manifest, &records, &resources, &[], limits(), &policy).is_ok());
    Arc::make_mut(policy.instruction_authorization.as_mut().unwrap()).requester =
        ActorId::new("ungranted-agent").unwrap();
    assert!(matches!(
        prepare_with_policy(&manifest, &records, &resources, &[], limits(), &policy),
        Err(ContextError::InstructionUnauthorized)
    ));
    Arc::make_mut(policy.instruction_authorization.as_mut().unwrap()).requester =
        ActorId::new("host").unwrap();
    manifest.instructions[0].resource.revision = "v2".into();
    assert!(matches!(
        prepare_with_policy(&manifest, &records, &resources, &[], limits(), &policy),
        Err(ContextError::InstructionUnauthorized)
    ));
    manifest.instructions[0] = instruction.clone();
    manifest.instructions[0].role = InstructionRole::Base;
    assert!(matches!(
        prepare_with_policy(&manifest, &records, &resources, &[], limits(), &policy),
        Err(ContextError::InstructionUnauthorized)
    ));
    manifest.instructions[0] = instruction.clone();
    policy.instruction_authorization = None;
    policy.omissions.push(ContextOmission {
        item: ContextItem::Instruction(instruction),
        reason: "skip".into(),
    });
    assert!(matches!(
        prepare_with_policy(&manifest, &records, &resources, &[], limits(), &policy),
        Err(ContextError::InstructionUnauthorized)
    ));
}

#[test]
fn exact_omissions_skip_unavailable_inputs_and_keep_an_audit() {
    let records = MemoryStore::default();
    let resources = MemoryResourceStore::default();
    let manifest = ContextManifest {
        resources: vec![reference("missing"), reference("missing")],
        ..Default::default()
    };
    let omission = ContextOmission {
        item: ContextItem::Resource(reference("missing")),
        reason: "not needed for this run".into(),
    };
    let policy = ContextPolicy {
        omissions: vec![omission.clone()],
        ..Default::default()
    };
    let prepared =
        prepare_with_policy(&manifest, &records, &resources, &[], limits(), &policy).unwrap();
    assert_eq!(prepared.requested_manifest, manifest);
    assert!(prepared.manifest.resources.is_empty());
    assert_eq!(prepared.policy.omissions, vec![omission.clone()]);
    for omissions in [
        vec![ContextOmission {
            item: ContextItem::Resource(reference("wrong")),
            reason: "wrong item".into(),
        }],
        vec![ContextOmission {
            reason: " ".into(),
            ..omission.clone()
        }],
        vec![omission.clone(), omission],
    ] {
        assert!(matches!(
            prepare_with_policy(
                &manifest,
                &records,
                &resources,
                &[],
                limits(),
                &ContextPolicy {
                    omissions,
                    ..Default::default()
                }
            ),
            Err(ContextError::InvalidOmission)
        ));
    }
    assert!(matches!(
        prepare_with_policy(
            &manifest,
            &records,
            &resources,
            &[],
            ContextLimits {
                max_items: 1,
                ..limits()
            },
            &policy
        ),
        Err(ContextError::ItemLimit)
    ));
}

#[test]
fn an_omitted_resource_cannot_leak_back_through_retained_messages() {
    let records = MemoryStore::default();
    let resources = MemoryResourceStore::default();
    let session = SessionId::new("history").unwrap();
    records.create_session(session.clone()).unwrap();
    let record = records
        .insert(draft("attached", RecordState::Complete))
        .unwrap();
    let manifest = ContextManifest {
        records: vec![record.record.id.clone()],
        resources: vec![reference("v1")],
        ..Default::default()
    };
    let policy = ContextPolicy {
        omissions: vec![ContextOmission {
            item: ContextItem::Resource(reference("v1")),
            reason: "exclude attachment".into(),
        }],
        ..Default::default()
    };
    assert!(matches!(
        prepare_with_policy(
            &manifest,
            &records,
            &resources,
            &[session],
            limits(),
            &policy
        ),
        Err(ContextError::ReferencedOmission(_))
    ));
}
fn draft(id: &str, state: RecordState) -> Draft {
    Draft {
        id: RecordId::new(id).unwrap(),
        session_id: SessionId::new("history").unwrap(),
        run_id: None,
        actor: ActorId::new("author").unwrap(),
        reply_to_id: None,
        source: None,
        state,
        payload: Payload::Message {
            kind: MessageKind::User,
            message: Message {
                content: vec![Content::Resource(reference("v1"))],
            },
        },
    }
}

fn ordered_resolution(store: &impl RecordStore) {
    let session = SessionId::new("history").unwrap();
    store.create_session(session.clone()).unwrap();
    let first = store.insert(draft("first", RecordState::Complete)).unwrap();
    let second = store
        .insert(draft("second", RecordState::Interrupted))
        .unwrap();
    let resources = MemoryResourceStore::default();
    let original = resources.put(resource("v1", b"original")).unwrap();
    resources.put(resource("v2", b"new instructions")).unwrap();
    let manifest = ContextManifest {
        records: vec![second.record.id.clone(), first.record.id.clone()],
        instructions: vec![InstructionRef {
            resource: reference("v1"),
            role: InstructionRole::Base,
        }],
        resources: vec![reference("v1")],
    };
    let prepared = prepare(&manifest, store, &resources, &[session], limits()).unwrap();
    assert_eq!(prepared.records[0].record.id, second.record.id);
    assert_eq!(prepared.records[0].state, RecordState::Interrupted);
    assert_eq!(prepared.records[1].record.actor, first.record.actor);
    assert_eq!(
        prepared.instructions[0].reference.role,
        InstructionRole::Base
    );
    assert_eq!(
        prepared.instructions[0].resource.bytes.as_ref(),
        b"original"
    );
    assert_eq!(prepared.resources.len(), 1);
    assert_eq!(prepared.resource_bytes, 8);
    assert!(Arc::ptr_eq(&original, &prepared.resources[0]));
}

#[test]
fn memory_context_preserves_order_roles_revisions_and_shares_bytes() {
    ordered_resolution(&MemoryStore::default());
}

#[cfg(feature = "sqlite")]
#[test]
fn sqlite_context_preserves_order_roles_revisions_and_shares_bytes() {
    ordered_resolution(&SqliteStore::open_in_memory().unwrap());
}

#[test]
fn resource_revisions_cannot_be_overwritten_or_replaced_by_latest() {
    let store = MemoryResourceStore::default();
    let first = store.put(resource("v1", b"old")).unwrap();
    assert!(Arc::ptr_eq(
        &first,
        &store.put(resource("v1", b"old")).unwrap()
    ));
    assert_eq!(
        store.put(resource("v1", b"changed")),
        Err(ResourceError::RevisionConflict)
    );
    store.put(resource("v2", b"new")).unwrap();
    assert_eq!(
        store.get(&reference("missing")),
        Err(ResourceError::Missing)
    );
    assert_eq!(
        store.put(resource(" ", b"bad")),
        Err(ResourceError::Invalid)
    );
}

#[test]
fn preparation_rejects_open_missing_and_out_of_scope_history() {
    let store = MemoryStore::default();
    let session = SessionId::new("history").unwrap();
    store.create_session(session.clone()).unwrap();
    let record = store.insert(draft("live", RecordState::Open)).unwrap();
    let resources = MemoryResourceStore::default();
    let mut manifest = ContextManifest {
        records: vec![record.record.id.clone()],
        ..Default::default()
    };
    assert!(matches!(
        prepare(&manifest, &store, &resources, &[], limits()),
        Err(ContextError::RecordOutsideScope(_))
    ));
    assert!(matches!(
        prepare(
            &manifest,
            &store,
            &resources,
            std::slice::from_ref(&session),
            limits()
        ),
        Err(ContextError::OpenRecord(_))
    ));
    manifest.records[0] = RecordId::new("absent").unwrap();
    assert!(matches!(
        prepare(&manifest, &store, &resources, &[session], limits()),
        Err(ContextError::Store(StoreError::MissingRecord))
    ));
}

#[test]
fn budgets_count_nested_selections_but_charge_shared_resource_bytes_once() {
    let records = MemoryStore::default();
    let session = SessionId::new("history").unwrap();
    records.create_session(session.clone()).unwrap();
    let record = records
        .insert(draft("selected", RecordState::Complete))
        .unwrap();
    let resources = MemoryResourceStore::default();
    resources.put(resource("v1", b"abc")).unwrap();
    let manifest = ContextManifest {
        records: vec![record.record.id.clone()],
        resources: vec![reference("v1")],
        ..Default::default()
    };
    let limited = ContextLimits {
        max_items: 3,
        max_resource_bytes: 3,
    };
    assert!(
        prepare(
            &manifest,
            &records,
            &resources,
            std::slice::from_ref(&session),
            limited
        )
        .is_ok()
    );
    assert!(matches!(
        prepare(
            &manifest,
            &records,
            &resources,
            std::slice::from_ref(&session),
            ContextLimits {
                max_items: 2,
                ..limited
            }
        ),
        Err(ContextError::ItemLimit)
    ));
    assert!(matches!(
        prepare(
            &manifest,
            &records,
            &resources,
            &[session],
            ContextLimits {
                max_resource_bytes: 2,
                ..limited
            }
        ),
        Err(ContextError::ResourceByteLimit)
    ));
}

#[test]
fn instructions_require_resolved_text_without_changing_their_role() {
    let records = MemoryStore::default();
    let resources = MemoryResourceStore::default();
    let manifest = ContextManifest {
        instructions: vec![InstructionRef {
            resource: reference("v1"),
            role: InstructionRole::Supplemental,
        }],
        ..Default::default()
    };
    assert!(matches!(
        prepare(&manifest, &records, &resources, &[], limits()),
        Err(ContextError::Resource {
            error: ResourceError::Missing,
            ..
        })
    ));
    resources.put(resource("v1", &[255])).unwrap();
    assert!(matches!(
        prepare(&manifest, &records, &resources, &[], limits()),
        Err(ContextError::InvalidInstruction(_))
    ));
}

#[test]
fn a_store_returning_the_wrong_revision_is_rejected() {
    struct WrongStore;
    impl ResourceStore for WrongStore {
        fn get(&self, _: &ResourceRef) -> Result<Arc<Resource>, ResourceError> {
            Ok(Arc::new(resource("v2", b"wrong")))
        }
    }
    let manifest = ContextManifest {
        resources: vec![reference("v1")],
        ..Default::default()
    };
    assert!(matches!(
        prepare(
            &manifest,
            &MemoryStore::default(),
            &WrongStore,
            &[],
            limits()
        ),
        Err(ContextError::Resource {
            error: ResourceError::RevisionConflict,
            ..
        })
    ));
}
