use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use agent_client_protocol::{
    Responder,
    schema::v1::{
        CancelNotification, McpServer, NewSessionRequest, NewSessionResponse, PermissionOptionId,
        PromptRequest, PromptResponse, RequestPermissionOutcome, RequestPermissionRequest,
        RequestPermissionResponse, SelectedPermissionOutcome, SessionId as NativeSessionId,
        SessionNotification, SessionUpdate, StopReason,
    },
};
use tokio::{
    sync::{mpsc, watch},
    time::timeout,
};

use super::{AcpConnection, AcpError};
use crate::{ContextManifest, Run, RunEvent, RunId, RunSpec, RunStatus, SessionId, SlotId};

const EVENT_CAPACITY: usize = 256;
const PERMISSION_CAPACITY: usize = 32;

/// Permission identity local to one run. Pass it back to that run only.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PermissionId {
    run_id: RunId,
    sequence: u64,
}

/// SDK-typed protocol updates. Portable record conversion is a later layer.
#[derive(Debug)]
pub enum AcpEvent {
    Update(SessionUpdate),
    Permission {
        id: PermissionId,
        request: RequestPermissionRequest,
    },
    /// Execution stopped; inspect the reason before treating the task as successful.
    Finished(StopReason),
}

enum Delivery {
    Event(Box<AcpEvent>),
    Finished(Result<PromptResponse, agent_client_protocol::Error>),
}

struct PendingPermission {
    options: Vec<PermissionOptionId>,
    responder: Responder<RequestPermissionResponse>,
}

struct Route {
    fault: watch::Sender<bool>,
    run_id: RunId,
    sender: mpsc::Sender<Delivery>,
    pending: HashMap<PermissionId, PendingPermission>,
    next_permission: u64,
    accepting: bool,
    cancelling: bool,
    overflow: bool,
}

impl Route {
    fn deliver(&mut self, message: Delivery) -> Result<(), agent_client_protocol::Error> {
        if !self.accepting {
            return Ok(());
        }
        if self.sender.try_send(message).is_err() {
            self.overflow = true;
            self.close();
            let _ = self.fault.send(true);
            // Stop the connection rather than silently losing part of a transcript.
            return Err(agent_client_protocol::Error::internal_error());
        }
        Ok(())
    }

    fn cancel_permissions(&mut self) {
        for (_, pending) in self.pending.drain() {
            let _ = pending.responder.respond(RequestPermissionResponse::new(
                RequestPermissionOutcome::Cancelled,
            ));
        }
    }

    fn close(&mut self) {
        self.accepting = false;
        self.cancel_permissions();
    }
}

pub(super) struct Routes {
    fault: watch::Sender<bool>,
    sessions: HashSet<NativeSessionId>,
    active: HashMap<NativeSessionId, Arc<Mutex<Route>>>,
}

impl Routes {
    pub(super) fn new(fault: watch::Sender<bool>) -> Self {
        Self {
            fault,
            sessions: HashSet::new(),
            active: HashMap::new(),
        }
    }
}

pub(super) fn route_update(
    routes: &Mutex<Routes>,
    notification: SessionNotification,
) -> Result<(), agent_client_protocol::Error> {
    let route = routes
        .lock()
        .unwrap()
        .active
        .get(&notification.session_id)
        .cloned();
    if let Some(route) = route {
        route
            .lock()
            .unwrap()
            .deliver(Delivery::Event(Box::new(AcpEvent::Update(
                notification.update,
            ))))?;
    }
    Ok(())
}

pub(super) fn route_permission(
    routes: &Mutex<Routes>,
    request: RequestPermissionRequest,
    responder: Responder<RequestPermissionResponse>,
) -> Result<(), agent_client_protocol::Error> {
    let route = routes
        .lock()
        .unwrap()
        .active
        .get(&request.session_id)
        .cloned();
    let Some(route) = route else {
        return responder.respond(RequestPermissionResponse::new(
            RequestPermissionOutcome::Cancelled,
        ));
    };
    let mut route = route.lock().unwrap();
    if !route.accepting || route.cancelling {
        return responder.respond(RequestPermissionResponse::new(
            RequestPermissionOutcome::Cancelled,
        ));
    }
    if route.pending.len() >= PERMISSION_CAPACITY {
        let _ = responder.respond(RequestPermissionResponse::new(
            RequestPermissionOutcome::Cancelled,
        ));
        route.overflow = true;
        route.close();
        let _ = route.fault.send(true);
        return Err(agent_client_protocol::Error::internal_error());
    }
    let id = PermissionId {
        run_id: route.run_id.clone(),
        sequence: route.next_permission,
    };
    route.next_permission += 1;
    route.pending.insert(
        id.clone(),
        PendingPermission {
            options: request
                .options
                .iter()
                .map(|option| option.option_id.clone())
                .collect(),
            responder,
        },
    );
    route.deliver(Delivery::Event(Box::new(AcpEvent::Permission {
        id,
        request,
    })))
}

/// A new native ACP session associated with application-owned session and slot IDs.
///
/// Borrows the connection, keeping process ownership explicit. Only one run may
/// borrow this handle at a time. A dropped or interrupted stream retires the handle
/// rather than allowing late provider traffic to mix into another run.
pub struct AcpSession<'connection> {
    connection: &'connection AcpConnection,
    session_id: SessionId,
    slot_id: SlotId,
    info: NewSessionResponse,
    retired: bool,
}

impl AcpConnection {
    /// Create a session with an explicit workspace and optional existing MCP servers.
    /// No local history is restored and no instructions are injected implicitly.
    pub async fn new_session(
        &self,
        session_id: SessionId,
        slot_id: SlotId,
        cwd: impl Into<PathBuf>,
        mcp_servers: Vec<McpServer>,
    ) -> Result<AcpSession<'_>, AcpError> {
        let cwd = cwd.into();
        if !cwd.is_absolute() {
            return Err(AcpError::InvalidWorkingDirectory);
        }
        if self.is_closed() {
            return Err(AcpError::Closed);
        }
        for server in &mcp_servers {
            let supported = match server {
                McpServer::Stdio(server) => {
                    if !server.command.is_absolute() {
                        return Err(AcpError::InvalidMcpCommand);
                    }
                    true
                }
                McpServer::Http(_) => self.info.agent_capabilities.mcp_capabilities.http,
                McpServer::Sse(_) => self.info.agent_capabilities.mcp_capabilities.sse,
                _ => false,
            };
            if !supported {
                return Err(AcpError::UnsupportedMcpTransport);
            }
        }
        let info = timeout(
            Duration::from_secs(30),
            self.connection
                .send_request(NewSessionRequest::new(cwd).mcp_servers(mcp_servers))
                .block_task(),
        )
        .await
        .map_err(|_| AcpError::RequestTimedOut)?
        .map_err(AcpError::Protocol)?;
        if !self
            .routes
            .lock()
            .unwrap()
            .sessions
            .insert(info.session_id.clone())
        {
            return Err(AcpError::SessionUnavailable);
        }
        Ok(AcpSession {
            connection: self,
            session_id,
            slot_id,
            info,
            retired: false,
        })
    }
}

impl<'connection> AcpSession<'connection> {
    /// Native initial session configuration, including any offered models or modes.
    pub fn info(&self) -> &NewSessionResponse {
        &self.info
    }

    /// Start a text-only run. Callers assign unique run IDs.
    ///
    /// ACP retains this native session's prior context. This first API supplies only
    /// the new text; it does not resolve a stored context manifest or change models.
    pub fn start_run(
        &mut self,
        id: RunId,
        text: impl Into<String>,
    ) -> Result<AcpRun<'_, 'connection>, AcpError> {
        let text = text.into();
        if text.trim().is_empty() {
            return Err(AcpError::EmptyPrompt);
        }
        if self.retired {
            return Err(AcpError::SessionUnavailable);
        }
        if self.connection.is_closed() {
            return Err(AcpError::Closed);
        }
        let (sender, receiver) = mpsc::channel(EVENT_CAPACITY);
        let route = Arc::new(Mutex::new(Route {
            fault: self.connection.routes.lock().unwrap().fault.clone(),
            run_id: id.clone(),
            sender,
            pending: HashMap::new(),
            next_permission: 0,
            accepting: true,
            cancelling: false,
            overflow: false,
        }));
        self.connection
            .routes
            .lock()
            .unwrap()
            .active
            .insert(self.info.session_id.clone(), route.clone());
        let mut run = Run::new(RunSpec {
            id,
            session_id: self.session_id.clone(),
            slot_id: self.slot_id.clone(),
            context: ContextManifest::default(),
            config: (),
        });
        run.apply(RunEvent::DispatchStarted).unwrap();
        let completion_route = route.clone();
        let result = self
            .connection
            .connection
            .send_request(PromptRequest::new(
                self.info.session_id.clone(),
                vec![text.clone().into()],
            ))
            .on_receiving_result(move |result| async move {
                let mut route = completion_route.lock().unwrap();
                route.deliver(Delivery::Finished(result))?;
                route.close();
                Ok(())
            });
        if let Err(error) = result {
            route.lock().unwrap().close();
            self.connection
                .routes
                .lock()
                .unwrap()
                .active
                .remove(&self.info.session_id);
            self.retired = true;
            return Err(AcpError::Protocol(error));
        }
        let closed = self.connection.closed.clone();
        Ok(AcpRun {
            session: self,
            run,
            input: text,
            receiver,
            route,
            closed,
            settled: false,
        })
    }
}

/// A streamed execution with application IDs and the shared run state machine.
/// Dropping it requests cancellation and retires its session; it does not claim
/// the provider stopped. Prefer `cancel()` followed by draining `next()`.
pub struct AcpRun<'session, 'connection> {
    session: &'session mut AcpSession<'connection>,
    run: Run,
    input: String,
    receiver: mpsc::Receiver<Delivery>,
    route: Arc<Mutex<Route>>,
    closed: watch::Receiver<bool>,
    settled: bool,
}

impl AcpRun<'_, '_> {
    pub fn run(&self) -> &Run {
        &self.run
    }
    pub fn input(&self) -> &str {
        &self.input
    }

    /// Consume streamed updates in arrival order. Cancellation of this wait alone
    /// is safe; dropping the whole run retires the session.
    pub async fn next(&mut self) -> Result<Option<AcpEvent>, AcpError> {
        if self.settled {
            return Ok(None);
        }
        loop {
            if self.route.lock().unwrap().overflow {
                self.unknown();
                return Err(AcpError::EventBufferFull);
            }
            let delivery = tokio::select! {
                biased;
                event = self.receiver.recv() => event,
                _ = self.closed.changed() => {
                    // Give any terminal response already queued precedence over EOF.
                    self.receiver.try_recv().ok()
                }
            };
            match delivery {
                Some(Delivery::Event(event)) => {
                    let event = *event;
                    if let AcpEvent::Permission { id, .. } = &event
                        && !self.route.lock().unwrap().pending.contains_key(id)
                    {
                        continue;
                    }
                    if self.run.status() == RunStatus::Starting {
                        self.run.apply(RunEvent::Started).unwrap();
                    }
                    return Ok(Some(event));
                }
                Some(Delivery::Finished(result)) => {
                    self.settled = true;
                    match result {
                        Ok(response) => {
                            self.run
                                .apply(if response.stop_reason == StopReason::Cancelled {
                                    RunEvent::CancellationConfirmed
                                } else {
                                    RunEvent::Completed
                                })
                                .unwrap();
                            return Ok(Some(AcpEvent::Finished(response.stop_reason)));
                        }
                        Err(error) => {
                            self.run.apply(RunEvent::Failed).unwrap();
                            return Err(AcpError::Protocol(error));
                        }
                    }
                }
                None => {
                    self.unknown();
                    return Err(AcpError::Closed);
                }
            }
        }
    }

    /// Select one of this request's offered option IDs, or dismiss it with None.
    /// Invalid decisions leave the request pending; resolved requests cannot be reused.
    pub fn respond(&mut self, id: PermissionId, option: Option<&str>) -> Result<(), AcpError> {
        let mut route = self.route.lock().unwrap();
        let pending = route.pending.get(&id).ok_or(AcpError::InvalidPermission)?;
        if let Some(option) = option
            && !pending.options.iter().any(|id| id.0.as_ref() == option)
        {
            return Err(AcpError::InvalidPermission);
        }
        let pending = route.pending.remove(&id).unwrap();
        let outcome = match option {
            Some(option) => RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                option.to_owned(),
            )),
            None => RequestPermissionOutcome::Cancelled,
        };
        pending
            .responder
            .respond(RequestPermissionResponse::new(outcome))
            .map_err(AcpError::Protocol)
    }

    pub fn cancel(&mut self) -> Result<(), AcpError> {
        if self.settled || self.run.cancellation_requested() {
            return Ok(());
        }
        self.run.apply(RunEvent::CancellationRequested).unwrap();
        let mut route = self.route.lock().unwrap();
        route.cancelling = true;
        route.cancel_permissions();
        self.session
            .connection
            .connection
            .send_notification(CancelNotification::new(
                self.session.info.session_id.clone(),
            ))
            .map_err(AcpError::Protocol)
    }

    fn unknown(&mut self) {
        self.run.apply(RunEvent::ConnectionLost).unwrap();
        self.settled = true;
        self.session.retired = true;
        self.route.lock().unwrap().close();
    }
}

impl Drop for AcpRun<'_, '_> {
    fn drop(&mut self) {
        if !self.settled {
            let _ = self.cancel();
            self.session.retired = true;
        }
        self.route.lock().unwrap().close();
        self.session
            .connection
            .routes
            .lock()
            .unwrap()
            .active
            .remove(&self.session.info.session_id);
    }
}
