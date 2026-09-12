use super::{
    AcpError, AcpEvent, AcpSession, PermissionId, RecordActors, RecordedRun, RecordingError,
    StopReason,
};
use crate::records::RecordStore;
use crate::structured::{JsonContract, JsonRejection};
use crate::{Run, RunId};
use serde::de::DeserializeOwned;
use serde_json::json;

#[derive(Debug, Clone, Copy)]
pub enum JsonOutputMode {
    ValidateReturnedText,
    RequireNativeEnforcement,
}

pub struct JsonTask<'a, T> {
    pub prompt: &'a str,
    pub contract: &'a JsonContract<T>,
    pub mode: JsonOutputMode,
}

impl<'connection> AcpSession<'connection> {
    pub fn start_recorded_json_run<
        'session,
        'store,
        'contract,
        S: RecordStore,
        T: DeserializeOwned,
    >(
        &'session mut self,
        id: RunId,
        task: JsonTask<'contract, T>,
        store: &'store S,
        actors: RecordActors,
    ) -> Result<RecordedJsonRun<'session, 'connection, 'store, 'contract, S, T>, RecordingError>
    {
        self.start_json_run(id, task, None, store, actors)
    }

    /// Validate returned JSON while delivering explicit selected context.
    /// Both tasks must name the same prompt.
    pub fn start_recorded_context_json_run<
        'session,
        'store,
        'contract,
        S: RecordStore,
        T: DeserializeOwned,
        R: crate::context::ResourceStore,
    >(
        &'session mut self,
        id: RunId,
        task: JsonTask<'contract, T>,
        context: super::ContextTask<'_, R>,
        store: &'store S,
        actors: RecordActors,
    ) -> Result<RecordedJsonRun<'session, 'connection, 'store, 'contract, S, T>, RecordingError>
    {
        if context.prompt != task.prompt {
            return Err(RecordingError::UnsupportedContext(
                "context and result prompts differ",
            ));
        }
        let prepared = crate::context::prepare_with_policy(
            context.manifest,
            store,
            context.resources,
            std::slice::from_ref(&self.session_id),
            context.limits,
            &context.policy,
        )?;
        self.start_json_run(
            id,
            task,
            Some((&prepared, context.mode, context.max_prompt_bytes)),
            store,
            actors,
        )
    }

    fn start_json_run<'session, 'store, 'contract, S: RecordStore, T: DeserializeOwned>(
        &'session mut self,
        id: RunId,
        task: JsonTask<'contract, T>,
        context: Option<(&crate::context::PreparedContext, super::ContextMode, usize)>,
        store: &'store S,
        actors: RecordActors,
    ) -> Result<RecordedJsonRun<'session, 'connection, 'store, 'contract, S, T>, RecordingError>
    {
        if matches!(task.mode, JsonOutputMode::RequireNativeEnforcement) {
            return Err(RecordingError::NativeStructuredOutputUnsupported);
        }
        if task.prompt.trim().is_empty() {
            return Err(AcpError::EmptyPrompt.into());
        }
        if self.retired || !self.quiescent {
            return Err(AcpError::SessionUnavailable.into());
        }
        if self.connection.is_closed() {
            return Err(AcpError::Closed.into());
        }
        let mut spec = self.run_spec(id)?;
        let wire = json!({"task":task.prompt,"output_instructions":task.contract.instructions(),
            "format":"Return exactly one JSON value in one assistant message. No Markdown fences or surrounding prose."}).to_string();
        let (wire, blocks, receipt) = if let Some((context, mode, limit)) = context {
            if context
                .policy
                .instruction_authorization
                .as_ref()
                .is_some_and(|auth| auth.grant.issuer != actors.host)
            {
                return Err(crate::context::ContextError::InstructionUnauthorized.into());
            }
            for selected in &context.records {
                if *store.get(&selected.record.id)? != **selected {
                    return Err(crate::records::StoreError::RevisionConflict.into());
                }
            }
            spec.context = context.manifest.clone();
            let (wire, blocks, receipt) = super::context::encode(
                context,
                &wire,
                limit,
                matches!(mode, super::ContextMode::AppendImagesToNative),
                self.connection
                    .info
                    .agent_capabilities
                    .prompt_capabilities
                    .image,
            )?;
            (wire, blocks, Some(receipt))
        } else {
            (wire.clone(), vec![wire.into()], None)
        };
        let mut recorder =
            super::recording::Recorder::new(store, spec.clone(), task.prompt, actors)?;
        recorder.result_evidence("result_contract", json!({"version":1,"name":task.contract.name(),"revision":task.contract.revision(),
            "mode":"validate_returned_text","native_enforcement":false,"max_validation_bytes":task.contract.max_validation_bytes(),
            "validator":"serde_deserialize","application_validation":task.contract.has_application_validation(),"wire_text":wire}))?;
        if let Some(receipt) = receipt {
            recorder.prepare_input(receipt)?;
            recorder.input_dispatch_attempted()?;
        }
        match self.dispatch_blocks(spec, wire, blocks) {
            Ok(inner) => Ok(RecordedJsonRun {
                inner: RecordedRun::new(inner, recorder),
                contract: task.contract,
                result: None,
                failed: false,
            }),
            Err(error) => {
                recorder.interrupt(error.to_string())?;
                Err(error.into())
            }
        }
    }
}

/// A normal provider completion and a valid structured result are separate facts.
/// The contract's output limit bounds parsing, not the existing transcript buffer.
pub struct RecordedJsonRun<'session, 'connection, 'store, 'contract, S: RecordStore, T> {
    inner: RecordedRun<'session, 'connection, 'store, S>,
    contract: &'contract JsonContract<T>,
    result: Option<Result<T, JsonRejection>>,
    failed: bool,
}

impl<S: RecordStore, T: DeserializeOwned> RecordedJsonRun<'_, '_, '_, '_, S, T> {
    pub fn run(&self) -> &Run {
        self.inner.run()
    }
    pub fn snapshot(&self) -> Vec<crate::records::Snapshot> {
        self.inner.snapshot()
    }
    pub fn checkpoint(&mut self) -> Result<(), RecordingError> {
        self.inner.checkpoint()
    }
    pub fn result(&self) -> Option<&Result<T, JsonRejection>> {
        self.result.as_ref()
    }
    pub fn into_result(self) -> Option<Result<T, JsonRejection>> {
        self.result
    }
    pub fn permission_pending(&self, id: &PermissionId) -> bool {
        self.inner.permission_pending(id)
    }
    pub fn respond(
        &mut self,
        id: PermissionId,
        option: Option<&str>,
    ) -> Result<(), RecordingError> {
        self.inner.respond(id, option)
    }
    pub fn cancel(&mut self) -> Result<(), RecordingError> {
        self.inner.cancel()
    }

    fn settle(&mut self, reason: Option<&StopReason>) -> Result<(), RecordingError> {
        let (sources, candidate) = self.inner.json_candidate();
        let result = if reason == Some(&StopReason::EndTurn) {
            candidate.and_then(|text| self.contract.validate(text))
        } else {
            Err(JsonRejection::Incomplete(match reason {
                Some(reason) => format!("provider stopped: {reason:?}"),
                None => "recording or provider error".into(),
            }))
        };
        let evidence = match &result {
            Ok(_) => json!({"status":"valid"}),
            Err(rejection) => json!({"status":"rejected","rejection":rejection}),
        };
        self.inner.result_evidence(json!({"version":1,"contract":{"name":self.contract.name(),"revision":self.contract.revision()},
            "mode":"validate_returned_text","native_enforcement":false,"sources":sources,"validation":evidence}))?;
        self.result = Some(result);
        Ok(())
    }

    pub async fn next(&mut self) -> Result<Option<AcpEvent>, RecordingError> {
        if self.failed {
            return Err(RecordingError::Closed);
        }
        let event = match self.inner.next().await {
            Ok(event) => event,
            Err(error) => {
                self.failed = true;
                self.settle(None)?;
                return Err(error);
            }
        };
        if let Some(AcpEvent::Finished(reason)) = &event
            && self.result.is_none()
            && let Err(error) = self.settle(Some(reason))
        {
            self.failed = true;
            return Err(error);
        }
        Ok(event)
    }
}
