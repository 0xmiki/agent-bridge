use agent_bridge::{acp::*, context::*, structured::*, *};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub context: Option<Selection>,
    pub result: Option<ResultDefinition>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    mode: String,
    #[serde(default)]
    records: Vec<RecordId>,
    #[serde(default)]
    resources: Vec<TextResource>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TextResource {
    id: ResourceId,
    revision: String,
    media_type: String,
    text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultDefinition {
    name: String,
    revision: String,
    schema: Value,
    max_validation_bytes: usize,
    mode: String,
}

impl ResultDefinition {
    pub fn contract(self) -> Result<JsonContract<Value>, String> {
        if self.mode != "validate_returned_text" {
            return Err("native result enforcement is unsupported".into());
        }
        if self.max_validation_bytes > 65536 {
            return Err("result validation limit exceeds 65536 bytes".into());
        }
        let validator =
            agent_bridge::tools::compile_schema(&self.schema).map_err(|e| e.to_string())?;
        Ok(JsonContract::new(
            self.name,
            self.revision,
            format!("Return JSON matching this schema: {}", self.schema),
            self.max_validation_bytes,
        )
        .map_err(|e| e.to_string())?
        .with_validation(move |value| {
            if validator.is_valid(value) {
                Ok(())
            } else {
                Err("result does not match the registered schema".into())
            }
        }))
    }
}

impl Selection {
    pub fn prepare(self) -> Result<(ContextManifest, MemoryResourceStore), String> {
        if self.mode != "append_to_native" {
            return Err("only append_to_native text context is supported".into());
        }
        if self.records.len() + self.resources.len() > 128 {
            return Err("context exceeds 128 items".into());
        }
        let mut manifest = ContextManifest {
            records: self.records,
            ..Default::default()
        };
        let resources = MemoryResourceStore::default();
        for resource in self.resources {
            if !matches!(resource.media_type.as_str(), "text/plain" | "text/markdown") {
                return Err("context resources must be plain text or Markdown".into());
            }
            let reference = ResourceRef {
                id: resource.id,
                revision: resource.revision,
            };
            resources
                .put(Resource {
                    reference: reference.clone(),
                    media_type: resource.media_type,
                    bytes: resource.text.into_bytes().into(),
                })
                .map_err(|e| e.to_string())?;
            manifest.resources.push(reference);
        }
        Ok((manifest, resources))
    }
}

pub fn context_task<'a>(
    prompt: &'a str,
    manifest: &'a ContextManifest,
    resources: &'a MemoryResourceStore,
) -> ContextTask<'a, MemoryResourceStore> {
    ContextTask {
        prompt,
        manifest,
        resources,
        policy: ContextPolicy::default(),
        mode: ContextMode::AppendToNative,
        limits: ContextLimits {
            max_items: 128,
            max_resource_bytes: 262144,
        },
        max_prompt_bytes: 524288,
    }
}

pub enum HostedRun<'s, 'c, 'd, 'v> {
    Plain(RecordedRun<'s, 'c, 'd, super::storage::Store>),
    Json(RecordedJsonRun<'s, 'c, 'd, 'v, super::storage::Store, Value>),
}

impl HostedRun<'_, '_, '_, '_> {
    pub fn run(&self) -> &Run {
        match self {
            Self::Plain(run) => run.run(),
            Self::Json(run) => run.run(),
        }
    }
    pub fn permission_pending(&self, id: &PermissionId) -> bool {
        match self {
            Self::Plain(run) => run.permission_pending(id),
            Self::Json(run) => run.permission_pending(id),
        }
    }
    pub fn respond(
        &mut self,
        id: PermissionId,
        option: Option<&str>,
    ) -> Result<(), RecordingError> {
        match self {
            Self::Plain(run) => run.respond(id, option),
            Self::Json(run) => run.respond(id, option),
        }
    }
    pub fn cancel(&mut self) -> Result<(), RecordingError> {
        match self {
            Self::Plain(run) => run.cancel(),
            Self::Json(run) => run.cancel(),
        }
    }
    pub async fn next(&mut self) -> Result<Option<AcpEvent>, RecordingError> {
        match self {
            Self::Plain(run) => run.next().await,
            Self::Json(run) => run.next().await,
        }
    }
    pub fn result(&self) -> Value {
        match self {
            Self::Plain(_) => Value::Null,
            Self::Json(run) => match run.result() {
                Some(Ok(value)) => json!({"status":"valid", "value":value}),
                Some(Err(rejection)) => json!({"status":"rejected", "rejection":rejection}),
                None => json!({"status":"unavailable"}),
            },
        }
    }
}
