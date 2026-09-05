use crate::context::{ContextLimits, PreparedContext, Resource, ResourceStore};
use crate::records::{MessageKind, Payload};
use crate::{Content, ContextManifest, InstructionRole, ResourceRef};
use base64::Engine;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Explicitly chooses user-level text context in addition to retained native context.
/// This neither replaces native history nor establishes system-instruction authority.
#[derive(Debug, Clone, Copy)]
pub enum ContextMode {
    AppendToNative,
    /// Append supported image blocks as well as the text envelope.
    AppendImagesToNative,
}
pub type TextContextMode = ContextMode;

pub struct ContextTask<'a, R: ResourceStore> {
    pub policy: crate::context::ContextPolicy,
    pub prompt: &'a str,
    pub manifest: &'a ContextManifest,
    pub resources: &'a R,
    pub limits: ContextLimits,
    pub max_prompt_bytes: usize,
    pub mode: ContextMode,
}

fn reference(reference: &ResourceRef) -> Value {
    json!({"id": reference.id.as_str(), "revision": reference.revision})
}

fn instruction(instruction: &crate::InstructionRef) -> Value {
    json!({"resource":reference(&instruction.resource), "role":match instruction.role { InstructionRole::Base => "base", InstructionRole::Supplemental => "supplemental" }})
}

pub(super) fn policy_evidence(context: &PreparedContext) -> Value {
    use crate::context::ContextItem;
    let omissions: Vec<_> = context.policy.omissions.iter().map(|omission| json!({"item":match &omission.item {
        ContextItem::Record(id) => json!({"type":"record","id":id.as_str()}),
        ContextItem::Instruction(value) => json!({"type":"instruction","instruction":instruction(value)}),
        ContextItem::Resource(value) => json!({"type":"resource","resource":reference(value)}),
    },"reason":omission.reason})).collect();
    let authority = context.policy.instruction_authorization.as_ref().map(|authorization| json!({
        "issuer":authorization.grant.issuer.as_str(), "requester":authorization.requester.as_str(),
        "subject":authorization.grant.subject.as_str(), "granted_instructions":authorization.grant.instructions.iter().map(instruction).collect::<Vec<_>>()
    }));
    json!({"omissions":omissions,"instruction_authority":authority,
        "requested_context":{"records":context.requested_manifest.records.iter().map(|id|id.as_str()).collect::<Vec<_>>(),
            "instructions":context.requested_manifest.instructions.iter().map(instruction).collect::<Vec<_>>(),
            "resources":context.requested_manifest.resources.iter().map(reference).collect::<Vec<_>>()}})
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
    images_allowed: bool,
    image_capability: bool,
) -> Result<(String, Vec<super::ContentBlock>, Value), super::RecordingError> {
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
    let mut resources = vec![];
    let mut images = vec![];
    let mut image_refs = vec![];
    let mut image_bytes = 0usize;
    for resource in &context.resources {
        if resource.media_type.starts_with("image/") {
            if !images_allowed || !image_capability {
                return Err(super::RecordingError::UnsupportedContext(
                    "image delivery requires explicit mode and advertised image support",
                ));
            }
            let bytes = resource.bytes.as_ref();
            let valid = match resource.media_type.as_str() {
                "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
                "image/jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
                "image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
                "image/webp" => bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"),
                _ => false,
            };
            if !valid {
                return Err(super::RecordingError::UnsupportedContext(
                    "unsupported image type or invalid signature",
                ));
            }
            image_bytes = bytes
                .len()
                .checked_add(2)
                .and_then(|n| (n / 3).checked_mul(4))
                .and_then(|n| image_bytes.checked_add(n))
                .filter(|n| *n <= max_bytes)
                .ok_or(super::RecordingError::UnsupportedContext(
                    "encoded prompt exceeds byte limit",
                ))?;
            let descriptor = json!({"reference":reference(&resource.reference),"media_type":resource.media_type,
                "sha256":format!("{:x}",Sha256::digest(bytes)),"bytes":bytes.len(),"prompt_block":images.len()+1});
            resources.push(descriptor.clone());
            image_refs.push(descriptor);
            images.push(super::ContentBlock::Image(
                agent_client_protocol::schema::v1::ImageContent::new(
                    base64::engine::general_purpose::STANDARD.encode(bytes),
                    &resource.media_type,
                ),
            ));
        } else {
            resources.push(json!({"reference":reference(&resource.reference),"media_type":resource.media_type,"text":text(resource)?}));
        }
    }
    let encoding = if images.is_empty() {
        "agent_bridge.text_context.v1"
    } else {
        "agent_bridge.media_context.v1"
    };
    let envelope = json!({"encoding":encoding,"context_mode":"append_to_native",
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
    let mut receipt = json!({"version":1,"state":"prepared","encoding":encoding,
        "context_mode":"append_to_native","omissions":[],"wire_text":wire,"wire_bytes":wire.len()});
    let mut blocks = vec![wire.clone().into()];
    blocks.extend(images);
    if !image_refs.is_empty() {
        // Count serialized blocks without retaining a second copy of the base64 data.
        struct Counter {
            used: usize,
            limit: usize,
        }
        impl std::io::Write for Counter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.used = self
                    .used
                    .checked_add(bytes.len())
                    .filter(|n| *n <= self.limit)
                    .ok_or(std::io::Error::other("prompt byte limit"))?;
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut counter = Counter {
            used: 0,
            limit: max_bytes,
        };
        serde_json::to_writer(&mut counter, &blocks).map_err(|_| {
            super::RecordingError::UnsupportedContext("encoded prompt exceeds byte limit")
        })?;
        receipt["version"] = json!(2);
        receipt["wire_bytes"] = json!(counter.used);
        receipt["images"] = json!(image_refs);
        receipt["resource_retention"] = json!("supplied_resource_store");
    }
    if !context.policy.omissions.is_empty() || context.policy.instruction_authorization.is_some() {
        let evidence = policy_evidence(context);
        receipt["version"] = json!(3);
        for key in ["omissions", "instruction_authority", "requested_context"] {
            receipt[key] = evidence[key].clone();
        }
    }
    Ok((wire, blocks, receipt))
}
