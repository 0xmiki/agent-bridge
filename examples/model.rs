//! Demonstrates the model only. No provider is launched and no model is called.
use agent_bridge::{
    ActorId, Content, ContextManifest, Message, Record, RecordId, Run, RunEvent, RunId, RunSpec,
    Session, SessionId, Slot, SlotId,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let session = Session {
        id: SessionId::new("design-review")?,
    };
    let first_slot = Slot {
        id: SlotId::new("local-a")?,
        driver: "example".to_owned(),
        config: (),
    };
    let second_slot = Slot {
        id: SlotId::new("local-b")?,
        ..first_slot.clone()
    };
    let question = Record {
        id: RecordId::new("question")?,
        session_id: session.id.clone(),
        run_id: None,
        sequence: 0,
        actor: ActorId::new("user")?,
        reply_to_id: None,
        payload: Message {
            content: vec![Content::Text("Review this layout.".to_owned())],
        },
    };

    // Requested configuration is caller-typed in this first slice.
    let mut first = Run::new(RunSpec {
        id: RunId::new("review-a")?,
        session_id: session.id.clone(),
        slot_id: first_slot.id,
        context: ContextManifest {
            records: vec![question.id.clone()],
            ..Default::default()
        },
        config: "model-a",
    });
    first.apply(RunEvent::DispatchStarted)?;
    first.apply(RunEvent::Started)?;
    first.apply(RunEvent::Completed)?;

    let answer = Record {
        id: RecordId::new("answer-a")?,
        session_id: session.id.clone(),
        run_id: Some(first.spec().id.clone()),
        sequence: 1,
        actor: ActorId::new("reviewer")?,
        reply_to_id: Some(question.id.clone()),
        payload: Message {
            content: vec![Content::Text("Consider increasing the spacing.".to_owned())],
        },
    };
    let second = Run::new(RunSpec {
        id: RunId::new("review-b")?,
        session_id: session.id,
        slot_id: second_slot.id,
        context: ContextManifest {
            records: vec![question.id, answer.id],
            ..Default::default()
        },
        config: "model-b",
    });

    assert_eq!(first.spec().session_id, second.spec().session_id);
    assert_ne!(first.spec().slot_id, second.spec().slot_id);
    println!(
        "First run: {:?}; next run: {:?}",
        first.status(),
        second.status()
    );
    println!("The session keeps its identity; the next run selects its slot and context.");
    Ok(())
}
