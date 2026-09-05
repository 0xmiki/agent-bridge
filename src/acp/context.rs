use crate::context::{ContextLimits, PreparedContext, Resource, ResourceStore};
use crate::records::{MessageKind, Payload};
use crate::{Content, ContextManifest, InstructionRole, ResourceRef};
use serde_json::{Value, json};

/// Explicitly chooses user-level text context in addition to retained native context.
/// This neither replaces native history nor establishes system-instruction authority.
#[derive(Debug, Clone, Copy)]
pub enum TextContextMode {
    AppendToNative,
}

pub struct ContextTask<'a, R: ResourceStore> {
    pub prompt: &'a str,
    pub manifest: &'a ContextManifest,
    pub resources: &'a R,
    pub limits: ContextLimits,
    pub max_prompt_bytes: usize,
    pub mode: TextContextMode,
}

fn reference(reference: &ResourceRef) -> Value {
    json!({"id": reference.id.as_str(), "revision": reference.revision})
}

fn text(resource: &Resource) -> Result<&str, super::RecordingError> {
    if !matches!(
        resource
            .media_type
            .split(';')
            .next()
            .unwrap_or_default()
            .trim(),
        "text/plain" | "text/markdown"
    ) {
        return Err(super::RecordingError::UnsupportedContext(
            "only plain-text and Markdown resources are supported",
        ));
    }
    std::str::from_utf8(&resource.bytes)
        .map_err(|_| super::RecordingError::UnsupportedContext("resource is not UTF-8"))
}

/// Text is mandatory in ACP. No optional image, embedding, or base-instruction
/// capability is inferred from unrelated provider declarations.
pub(super) fn encode(
    context: &PreparedContext,
    prompt: &str,
    max_bytes: usize,
) -> Result<(String, Value), super::RecordingError> {
    let mut instructions = vec![];
    for instruction in &context.instructions {
        if instruction.reference.role == InstructionRole::Base {
            return Err(super::RecordingError::UnsupportedContext(
                "ACP text delivery cannot replace base instructions",
            ));
        }
        instructions.push(json!({"resource": reference(&instruction.reference.resource), "role": "supplemental_user_text"}));
    }
    let mut records = vec![];
    for snapshot in &context.records {
        let Payload::Message { kind, message } = &snapshot.record.payload else {
            return Err(super::RecordingError::UnsupportedContext(
                "selected record is not a conversation message",
            ));
        };
        let kind = match kind {
            MessageKind::User => "user",
            MessageKind::Agent => "agent",
            MessageKind::Reasoning => {
                return Err(super::RecordingError::UnsupportedContext(
                    "reasoning records require an explicit future delivery policy",
                ));
            }
        };
        let content: Vec<_> = message
            .content
            .iter()
            .map(|content| match content {
                Content::Text(text) => json!({"text": text}),
                Content::Resource(resource) => json!({"resource": reference(resource)}),
            })
            .collect();
        let state = match snapshot.state {
            crate::records::RecordState::Open => "open",
            crate::records::RecordState::Complete => "complete",
            crate::records::RecordState::Interrupted => "interrupted",
        };
        records.push(
            json!({"id":snapshot.record.id.as_str(),"revision":snapshot.revision,
            "actor":snapshot.record.actor.as_str(),"kind":kind,"state":state,"content":content}),
        );
    }
    let resources = context.resources.iter().map(|resource| Ok(json!({
        "reference":reference(&resource.reference),"media_type":resource.media_type,"text":text(resource)?
    }))).collect::<Result<Vec<_>, super::RecordingError>>()?;
    let envelope = json!({"encoding":"agent_bridge.text_context.v1","context_mode":"append_to_native",
        "history":records,"supplemental_instructions":instructions,"resources":resources,"task":prompt});
    struct LimitedWriter {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl std::io::Write for LimitedWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
                return Err(std::io::Error::other("prompt byte limit"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = LimitedWriter {
        bytes: vec![],
        limit: max_bytes,
    };
    serde_json::to_writer(&mut writer, &envelope).map_err(|_| {
        super::RecordingError::UnsupportedContext("encoded prompt exceeds byte limit")
    })?;
    let wire = String::from_utf8(writer.bytes).expect("JSON serializer writes UTF-8");
    // Persist the exact wire text once, independent of resource-store retention.
    let receipt = json!({"version":1,"state":"prepared","encoding":"agent_bridge.text_context.v1",
        "context_mode":"append_to_native","omissions":[],"wire_text":wire,"wire_bytes":wire.len()});
    Ok((wire, receipt))
}
