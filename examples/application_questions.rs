//! A granted application tool pauses for a scripted host answer, then resumes.
//! Replace the host task with a UI in an application; this example needs no provider.
use agent_bridge::questions::{PendingQuestion, QuestionResponder};
use agent_bridge::records::*;
use agent_bridge::tools::*;
use agent_bridge::{ActorId, RecordId, SessionId, SlotId};
use schemars::JsonSchema;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    error::Error,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ChooseInput {}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let database = std::env::args()
        .nth(1)
        .ok_or("usage: application_questions <database>")?;
    let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let store = Arc::new(SqliteStore::open(&database)?);
    let session = SessionId::new(format!("question-example-{unique}"))?;
    store.create_session(session.clone())?;
    let scope = ToolScope {
        session: session.clone(),
        slot: SlotId::new("assistant-slot")?,
    };
    let host = ActorId::new("application")?;
    let actor = ActorId::new("assistant")?;
    let reference = ToolRef {
        name: "choose_destination".into(),
        revision: "v1".into(),
    };
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<QuestionResponder<SqliteStore>>(1);
    let mut registry = ToolRegistry::default();
    let records = store.clone();
    registry.register::<ChooseInput, _, _, _>(
        reference.clone(),
        "Ask the host where to put a draft",
        move |context, _| {
            let records = records.clone();
            let sender = sender.clone();
            async move {
                let pending = PendingQuestion::open(
                    records,
                    Draft {
                        id: RecordId::new(format!("destination-{unique}")).unwrap(),
                        session_id: context.scope.session,
                        run_id: None,
                        actor: context.actor,
                        reply_to_id: None,
                        source: None,
                        state: RecordState::Open,
                        payload: Payload::Question(Question {
                            title: "Where should the draft go?".into(),
                            fields: vec![QuestionField {
                                id: "destination".into(),
                                label: "Destination".into(),
                                required: true,
                                kind: QuestionFieldKind::Select {
                                    options: vec![
                                        QuestionOption {
                                            id: "drafts".into(),
                                            label: "Drafts".into(),
                                        },
                                        QuestionOption {
                                            id: "review".into(),
                                            label: "Review".into(),
                                        },
                                    ],
                                },
                            }],
                        }),
                    },
                    context.host,
                )
                .map_err(|error| ToolError::Handler(error.to_string()))?;
                sender
                    .send(pending.responder())
                    .await
                    .map_err(|_| ToolError::Handler("host question channel closed".into()))?;
                let answer = pending
                    .wait(context.cancellation)
                    .await
                    .map_err(|error| ToolError::Handler(error.to_string()))?;
                match &answer.record.payload {
                    Payload::Answer {
                        outcome: AnswerOutcome::Submitted(values),
                        ..
                    } => match &values["destination"] {
                        AnswerValue::Selected(destination) => {
                            Ok(serde_json::json!({"destination":destination}))
                        }
                        _ => unreachable!(),
                    },
                    _ => Err(ToolError::Handler(
                        "question was declined or cancelled".into(),
                    )),
                }
            }
        },
    )?;
    let grant = ToolGrant {
        issuer: host.clone(),
        subject: actor.clone(),
        scope: scope.clone(),
        tools: vec![reference],
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        let invocation = registry.invoke(
            "choose_destination",
            serde_json::json!({}),
            &grant,
            ToolInvocation {
                host,
                actor,
                scope,
                cancellation: CancellationToken::new(),
            },
        );
        let host = async {
            let responder = receiver
                .recv()
                .await
                .ok_or("no question reached the host")?;
            // Deterministic test input, not a real user's approval.
            responder.answer(
                ActorId::new("scripted-host-answer")?,
                AnswerOutcome::Submitted(BTreeMap::from([(
                    "destination".into(),
                    AnswerValue::Selected("review".into()),
                )])),
            )?;
            Ok::<_, Box<dyn Error>>(())
        };
        let (result, answered) = tokio::join!(invocation, host);
        answered?;
        if result? != serde_json::json!({"destination":"review"}) {
            return Err("tool did not use the host answer".into());
        }
        Ok::<_, Box<dyn Error>>(())
    })
    .await??;
    drop(registry);
    drop(receiver);
    drop(store);
    let reopened = SqliteStore::open(database)?;
    let history = reopened.list(&session, None, 100)?;
    if !history.iter().any(|record| {
        matches!(record.record.payload, Payload::Question(_))
            && record.state == RecordState::Complete
    }) || !history.iter().any(|record| {
        matches!(
            record.record.payload,
            Payload::Answer {
                delivery: AnswerDelivery::Stored,
                ..
            }
        )
    }) {
        return Err("question and answer did not survive reopen".into());
    }
    println!(
        "Application tool resumed from a validated host answer; question and answer survived SQLite reopen."
    );
    Ok(())
}
