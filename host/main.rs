//! Experimental owned ACP host. Stdout is exclusively protocol v1 JSON-lines.
mod output;
mod questions;
mod receipts;
mod storage;
mod tool_hub;
use agent_bridge::acp::{
    AcpConnection, AcpEvent, AcpLaunch, ContentBlock, RecordActors, SessionUpdate,
};
use agent_bridge::records::{ChangeCursor, ChangeStore, RecordStore, Snapshot, SqliteStore};
use agent_bridge::{ActorId, RunId, RunStatus, SessionId, SlotId};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::BufRead,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio_util::sync::CancellationToken;

const MAX_LINE: usize = 1024 * 1024;
const MAX_SESSIONS: usize = 8;
const STORAGE_LOCK_WAIT: Duration = Duration::from_millis(100);
static NEXT: AtomicU64 = AtomicU64::new(0);
fn identity(prefix: &str) -> String {
    format!(
        "{prefix}-{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u32,
    id: String,
    method: String,
    #[serde(default)]
    params: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Open {
    database: String,
    workspace: String,
    executable: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
    #[serde(default)]
    delete_session_on_close: bool,
    #[serde(default)]
    allow_tools: Vec<agent_bridge::ToolRef>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigureTools {
    callback_protocol: u32,
    tools: Vec<tool_hub::Definition>,
    timeout_ms: u64,
    #[serde(default)]
    question_protocol: Option<u32>,
}
#[derive(Clone)]
struct Output {
    sender: mpsc::SyncSender<Value>,
    stop: CancellationToken,
    failed: Arc<AtomicBool>,
}
impl Output {
    fn emit(&self, mut value: Value) {
        value["version"] = json!(1);
        if self.sender.try_send(value).is_err() {
            self.failed.store(true, Ordering::SeqCst);
            self.stop.cancel();
        }
    }
    fn ok(&self, id: &str, result: Value) {
        self.emit(json!({"id":id,"ok":true,"result":result}));
    }
    fn error(&self, id: &str, code: &str, message: impl std::fmt::Display) {
        self.emit(json!({"id":id,"ok":false,"error":{"code":code,"message":message.to_string()}}));
    }
}
fn string<'a>(params: &'a Value, key: &str) -> Result<&'a str, &'static str> {
    params[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("missing or empty string parameter")
}
fn state(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Queued => "queued",
        RunStatus::Starting => "starting",
        RunStatus::Running => "running",
        RunStatus::Cancelling => "cancelling",
        RunStatus::Completed => "completed",
        RunStatus::Failed => "failed",
        RunStatus::Cancelled => "cancelled",
        RunStatus::Unknown => "unknown",
    }
}
fn snapshot(value: &Snapshot) -> Value {
    let record = &value.record;
    json!({"id":record.id.as_str(),"session_id":record.session_id.as_str(),"run_id":record.run_id.as_ref().map(|id|id.as_str()),
        "actor":record.actor.as_str(),"sequence":record.sequence.to_string(),"revision":value.revision.to_string(),"state":value.state,"payload":record.payload,"receipt":receipts::view(&record.payload),
        "reply_to_id":record.reply_to_id.as_ref().map(|id|id.as_str()),"source":value.source})
}
fn history(request: &Request) -> Result<Value, Box<dyn std::error::Error>> {
    let store =
        SqliteStore::open_read_only(string(&request.params, "database")?, STORAGE_LOCK_WAIT)?;
    let session = SessionId::new(string(&request.params, "session_id")?)?;
    if request.method == "snapshot" || request.method == "changes" {
        let cursor = request
            .params
            .get("cursor")
            .filter(|v| !v.is_null())
            .map(|v| -> Result<ChangeCursor, Box<dyn std::error::Error>> {
                Ok(ChangeCursor {
                    epoch: string(v, "epoch")?.into(),
                    session_id: SessionId::new(string(v, "session_id")?)?,
                    position: string(v, "position")?.parse()?,
                })
            })
            .transpose()?;
        if cursor.as_ref().is_some_and(|c| c.session_id != session) {
            return Err("cursor belongs to another session".into());
        }
        let limit = request
            .params
            .get("limit")
            .map(|v| v.as_u64().ok_or("invalid limit"))
            .transpose()?
            .unwrap_or(100);
        if !(1..=1000).contains(&limit) {
            return Err("limit must be 1..=1000".into());
        }
        let page = if request.method == "changes" {
            store.changes(cursor.as_ref().ok_or("cursor required")?, limit as usize)?
        } else {
            let after = request
                .params
                .get("after")
                .filter(|v| !v.is_null())
                .map(|v| {
                    v.as_str()
                        .ok_or("invalid after")?
                        .parse::<u64>()
                        .map_err(|_| "invalid after")
                })
                .transpose()?;
            store.snapshot_page(&session, cursor.as_ref(), after, limit as usize)?
        };
        return Ok(
            json!({"records":page.records.iter().map(|r|snapshot(r)).collect::<Vec<_>>(),"cursor":{"epoch":page.cursor.epoch,"session_id":page.cursor.session_id.as_str(),"position":page.cursor.position.to_string()},"next_after":page.next_after.map(|n|n.to_string()),"page_full":page.page_full}),
        );
    }
    let after = request
        .params
        .get("after")
        .filter(|v| !v.is_null())
        .map(|value| {
            value
                .as_str()
                .and_then(|text| text.parse::<u64>().ok())
                .ok_or("after must be an unsigned decimal string")
        })
        .transpose()?;
    let limit = request
        .params
        .get("limit")
        .map(|value| value.as_u64().ok_or("limit must be an unsigned integer"))
        .transpose()?
        .unwrap_or(1000);
    if limit == 0 || limit > 1000 {
        return Err("limit must be 1..=1000".into());
    }
    let records = store.list(&session, after, limit as usize)?;
    Ok(
        json!({"records":records.iter().map(|r|snapshot(r)).collect::<Vec<_>>(),"next_after":records.last().map(|r|r.record.sequence.to_string()),"page_full":records.len()==limit as usize}),
    )
}

async fn session_worker(
    open: Open,
    create_id: String,
    session_id: String,
    mut commands: tokio::sync::mpsc::Receiver<Request>,
    output: Output,
    storage_budget: storage::Budget,
    tools: Arc<tool_hub::Hub>,
) {
    let setup = async {
        if !std::path::Path::new(&open.database).is_absolute()
            || !std::path::Path::new(&open.workspace).is_absolute()
            || open.executable.trim().is_empty()
        {
            return Err(
                "database/workspace must be absolute paths and executable must be nonempty".into(),
            );
        }
        let store = Arc::new(
            storage::Store::open(open.database.clone().into(), &storage_budget)
                .map_err(|e| e.to_string())?,
        );
        let mut launch = AcpLaunch::new(open.executable);
        for argument in open.args {
            launch = launch.arg(argument);
        }
        for (name, value) in open.env {
            launch = launch.env(name, value);
        }
        let connection = AcpConnection::connect(launch)
            .await
            .map_err(|e| e.to_string())?;
        Ok::<_, String>((store, connection))
    };
    let (store, connection) = tokio::select! {
        _ = output.stop.cancelled() => return,
        result = setup => match result { Ok(value)=>value, Err(error)=>{ output.error(&create_id,"setup_failed",error); return; } },
    };
    let result = async {
        store.create_session(SessionId::new(&session_id).unwrap()).map_err(|e| e.to_string())?;
        let slot_id = identity("slot");
        let binding = tools.bind(agent_bridge::ToolScope {session:SessionId::new(&session_id).unwrap(),slot:SlotId::new(&slot_id).unwrap()},open.allow_tools,store.clone()).await?;
        let mcp = binding.as_ref().map(|binding| vec![binding.mcp.clone()]).unwrap_or_default();
        let mut session = tokio::select! {
            _ = output.stop.cancelled() => return Ok::<_, String>(()),
            result = connection.new_session(SessionId::new(&session_id).unwrap(), SlotId::new(&slot_id).unwrap(), open.workspace, mcp) => result.map_err(|e|e.to_string())?,
        };
        output.ok(&create_id,json!({"session_id":session_id,"slot_id":slot_id,"configuration":session.configuration().values,"tool_binding_id":binding.as_ref().map(|b|&b.id)}));
        loop {
            let request = tokio::select! { _ = output.stop.cancelled()=>break, request=commands.recv()=>match request { Some(request)=>request,None=>break } };
            if request.method != "run" { output.error(&request.id,"not_running","session has no active run"); continue; }
            let prompt = match string(&request.params,"prompt") { Ok(prompt)=>prompt,Err(error)=>{output.error(&request.id,"invalid_params",error);continue;} };
            let run_id = identity("run");
            if let Some(binding) = &binding && let Err(error) = binding.begin() { output.error(&request.id,"tool_binding_retired",error); continue; }
            let mut run = match session.start_recorded_run(RunId::new(&run_id).unwrap(),prompt,store.as_ref(),RecordActors {
                user:ActorId::new("user").unwrap(),agent:ActorId::new("assistant").unwrap(),host:ActorId::new("host").unwrap(),
            }) { Ok(run)=>run,Err(error)=>{if let Some(binding) = &binding { binding.end(); } output.error(&request.id,"start_failed",error);continue;} };
            output.ok(&request.id,json!({"run_id":run_id,"session_id":session_id}));
            let mut permissions: HashMap<String, (agent_bridge::acp::PermissionId, Value)> = HashMap::new();
            let mut reason = None; let mut failure = None;
            loop {
                tokio::select! {
                    biased;
                    _ = output.stop.cancelled() => { let _=run.cancel(); break; },
                    command = commands.recv() => {
                        let Some(command)=command else { let _=run.cancel(); break; };
                        if command.method == "run" { output.error(&command.id,"session_busy","session already has an active run"); continue; }
                        if command.params["run_id"].as_str() != Some(run_id.as_str()) { output.error(&command.id,"stale_run","run ID is not active"); continue; }
                        match command.method.as_str() {
                            "pending_permissions" => {
                                permissions.retain(|_, (id, _)| run.permission_pending(id));
                                output.ok(&command.id, json!(permissions.values().map(|(_, event)| event).collect::<Vec<_>>()));
                            }
                            "cancel" => { if let Some(binding) = &binding { binding.end(); } match run.cancel() { Ok(())=>output.ok(&command.id,json!({"cancellation_requested":true})),Err(error)=>output.error(&command.id,"cancel_failed",error) } },
                            "respond" => {
                                let result = (|| {
                                    let token=string(&command.params,"permission_id")?;
                                    let (permission, _)=permissions.get(token).ok_or("permission is not pending")?;
                                    let option=match command.params.get("option_id") { None|Some(Value::Null)=>None,Some(Value::String(value))=>Some(value.as_str()),_=>return Err("option_id must be a string or null") };
                                    run.respond(permission.clone(),option).map_err(|_|"permission response was rejected")?;
                                    permissions.remove(token); Ok::<_,&str>(())
                                })();
                                match result { Ok(())=>output.ok(&command.id,json!({"response_queued":true})),Err(error)=>output.error(&command.id,"invalid_response",error) }
                            }
                            _ => output.error(&command.id,"unknown_method","unsupported session command"),
                        }
                    }
                    event = run.next() => match event {
                        Ok(Some(AcpEvent::Update(SessionUpdate::AgentMessageChunk(chunk)))) => if let ContentBlock::Text(text)=chunk.content {
                            output.emit(json!({"event":"text_delta","stream":request.id,"session_id":session_id,"run_id":run_id,"text":text.text}));
                        },
                        Ok(Some(AcpEvent::Permission {id,request:permission})) if run.permission_pending(&id) => {
                            let token=identity("permission");
                            let event=json!({"event":"permission","stream":request.id,"session_id":session_id,"run_id":run_id,"permission_id":token,
                                "title":permission.tool_call.fields.title,"options":permission.options.iter().map(|option|json!({"id":option.option_id.to_string(),"label":option.name,"effect":option.kind})).collect::<Vec<_>>()});
                            permissions.retain(|_, (id, _)| run.permission_pending(id));
                            permissions.insert(token,(id,event.clone()));
                            output.emit(event);
                        }
                        Ok(Some(AcpEvent::Finished(value))) => reason=Some(value),
                        Ok(Some(_))=>{}, Ok(None)=>break,
                        Err(error)=>{failure=Some(error.to_string());break;}
                    }
                }
            }
            let status=if failure.is_some() { "unknown" } else { state(run.run().status()) };
            drop(run);
            if let Some(binding) = &binding { binding.end(); }
            if output.stop.is_cancelled() { break; }
            output.emit(json!({"event":"run_finished","stream":request.id,"session_id":session_id,"run_id":run_id,"status":status,"reason":reason,"recording_error":failure}));
        }
        if open.delete_session_on_close {
            if let Err(error) = session.delete().await {
                output.failed.store(true, Ordering::SeqCst);
                output.emit(json!({"event":"session_error","session_id":session_id,"message":format!("provider cleanup failed: {error}")}));
            } else { eprintln!("provider session cleanup completed for {session_id}"); }
        }
        Ok(())
    }.await;
    if let Err(error) = result {
        output.error(&create_id, "session_setup_failed", error);
    }
    if let Err(error) = connection.shutdown().await {
        output.emit(
            json!({"event":"session_error","session_id":session_id,"message":error.to_string()}),
        );
    }
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--mcp-tools") {
        if let Err(error) = tool_hub::helper() {
            eprintln!("MCP helper failed: {error}");
            std::process::exit(1);
        }
        return;
    }
    let storage_budget = storage::Budget::new(12);
    let stop = CancellationToken::new();
    let failed = Arc::new(AtomicBool::new(false));
    let (sender, receiver) = mpsc::sync_channel::<Value>(128);
    let output = Output {
        sender,
        stop: stop.clone(),
        failed: failed.clone(),
    };
    let mut tools = Arc::new(tool_hub::Hub::new(vec![], 30000, output.clone()).unwrap());
    let question_service = Arc::new(questions::Service::new(output.clone()));
    let question_runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let mut question_tasks: Vec<tokio::task::JoinHandle<()>> = vec![];
    let mut question_protocol = false;
    let mut tools_locked = false;
    let writer_stop = stop.clone();
    let writer_failed = failed.clone();
    let writer = std::thread::spawn(move || {
        if let Err(error) = output::write_frames(receiver, &writer_stop) {
            eprintln!("host output failed: {error}");
            writer_failed.store(true, Ordering::SeqCst);
            writer_stop.cancel();
        }
    });
    let (input_tx, input_rx) = mpsc::sync_channel::<Vec<u8>>(32);
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut stdin = stdin.lock();
        loop {
            let mut line = Vec::new();
            let mut bounded = std::io::Read::take(&mut stdin, (MAX_LINE + 1) as u64);
            match bounded.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let oversized = line.len() > MAX_LINE;
                    if input_tx.send(line).is_err() || oversized {
                        break;
                    }
                }
            }
        }
    });
    output.emit(json!({"event":"ready","protocol":1}));
    let mut sessions = HashMap::new();
    let mut workers: Vec<std::thread::JoinHandle<()>> = Vec::new();
    let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    loop {
        question_tasks.retain(|task| !task.is_finished());
        sessions.retain(|_, sender: &mut tokio::sync::mpsc::Sender<Request>| !sender.is_closed());
        let mut index = 0;
        while index < workers.len() {
            if workers[index].is_finished() {
                if workers.swap_remove(index).join().is_err() {
                    failed.store(true, Ordering::SeqCst);
                }
            } else {
                index += 1;
            }
        }
        if stop.is_cancelled() {
            break;
        }
        let line = match input_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => line,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        };
        if line.len() > MAX_LINE {
            output.emit(json!({"event":"protocol_error","code":"frame_too_large"}));
            break;
        }
        let request: Request = match serde_json::from_slice(&line) {
            Ok(request) => request,
            Err(_) => {
                output.emit(json!({"event":"protocol_error","code":"invalid_request"}));
                continue;
            }
        };
        if request.version != 1 || request.id.is_empty() {
            output.error(
                &request.id,
                "unsupported_protocol",
                "version 1 and a nonempty request ID are required",
            );
            continue;
        }
        match request.method.as_str() {
            "configure_tools" => {
                if tools_locked {
                    output.error(
                        &request.id,
                        "tools_locked",
                        "tool declarations are immutable after configuration or session creation",
                    );
                    continue;
                }
                let config: ConfigureTools = match serde_json::from_value(request.params) {
                    Ok(config) => config,
                    Err(error) => {
                        output.error(&request.id, "invalid_tools", error);
                        continue;
                    }
                };
                if config.callback_protocol != 1 {
                    output.error(
                        &request.id,
                        "unsupported_tool_protocol",
                        "tool callback protocol 1 required",
                    );
                    continue;
                }
                if config.question_protocol.is_some_and(|version| version != 1) {
                    output.error(
                        &request.id,
                        "unsupported_question_protocol",
                        "question protocol 1 required",
                    );
                    continue;
                }
                match tool_hub::Hub::new(config.tools, config.timeout_ms, output.clone()) {
                    Ok(hub) => {
                        tools = Arc::new(hub);
                        tools_locked = true;
                        question_protocol = config.question_protocol == Some(1);
                        output.ok(&request.id, json!({"callback_protocol":1,"question_protocol":config.question_protocol}));
                    }
                    Err(error) => output.error(&request.id, "invalid_tools", error),
                }
            }
            "tool_result" => match tools.reply(&request.params) {
                Ok(()) => output.ok(&request.id, json!({"response_queued":true})),
                Err(error) => output.error(&request.id, "invalid_tool_result", error),
            },
            "question_ask" | "question_answer" | "question_pending" => {
                if !question_protocol {
                    output.error(
                        &request.id,
                        "questions_disabled",
                        "question callbacks were not negotiated",
                    );
                    continue;
                }
                let owner = if request.method == "question_ask" {
                    match tools.question_owner(&request.params) {
                        Ok(owner) => Some(owner),
                        Err(error) => {
                            output.error(&request.id, "invalid_question_scope", error);
                            continue;
                        }
                    }
                } else {
                    None
                };
                if let Some(task) =
                    question_service.dispatch(question_runtime.handle(), request, owner)
                {
                    question_tasks.push(task);
                }
            }
            "ping" => output.ok(&request.id, json!({"alive":true})),
            "shutdown" => {
                output.ok(&request.id, json!({"shutdown_requested":true}));
                break;
            }
            "create_session" => {
                if sessions.len() >= MAX_SESSIONS {
                    output.error(&request.id, "session_limit", "host session limit reached");
                    continue;
                }
                let open: Open = match serde_json::from_value(request.params) {
                    Ok(open) => open,
                    Err(_) => {
                        output.error(&request.id, "invalid_params", "invalid session options");
                        continue;
                    }
                };
                let id = identity("session");
                if let Err(error) = tools.validate(&open.allow_tools) {
                    output.error(&request.id, "invalid_tool_grant", error);
                    continue;
                }
                tools_locked = true;
                let worker_id = id.clone();
                let worker_output = output.clone();
                let budget = storage_budget.clone();
                let tools = tools.clone();
                let (tx, rx) = tokio::sync::mpsc::channel(16);
                sessions.insert(id, tx);
                workers.push(std::thread::spawn(move || {
                    let runtime = tokio::runtime::Builder::new_multi_thread()
                        .worker_threads(2)
                        .enable_all()
                        .build()
                        .unwrap();
                    runtime.block_on(session_worker(
                        open,
                        request.id,
                        worker_id,
                        rx,
                        worker_output,
                        budget,
                        tools,
                    ));
                }));
            }
            "history" | "snapshot" | "changes" => {
                if reads.load(Ordering::SeqCst) >= 4 {
                    output.error(
                        &request.id,
                        "history_limit",
                        "too many concurrent history requests",
                    );
                    continue;
                }
                reads.fetch_add(1, Ordering::SeqCst);
                let reads = reads.clone();
                let output = output.clone();
                let budget = storage_budget.clone();
                workers.push(std::thread::spawn(move || {
                    struct Permit(Arc<std::sync::atomic::AtomicUsize>);
                    impl Drop for Permit {
                        fn drop(&mut self) {
                            self.0.fetch_sub(1, Ordering::SeqCst);
                        }
                    }
                    let permit = Permit(reads);
                    let id = request.id.clone();
                    let result = budget
                        .execute(move || {
                            let _permit = permit;
                            history(&request).map_err(|error| error.to_string())
                        })
                        .map_err(|error| error.to_string())
                        .and_then(|value| value);
                    match result {
                        Ok(value) => output.ok(&id, value),
                        Err(error) => output.error(&id, "history_failed", error),
                    }
                }));
            }
            "run" | "cancel" | "respond" | "pending_permissions" => {
                let id = match string(&request.params, "session_id") {
                    Ok(id) => id,
                    Err(error) => {
                        output.error(&request.id, "invalid_params", error);
                        continue;
                    }
                };
                match sessions.get(id) {
                    Some(sender) => {
                        let request_id = request.id.clone();
                        if sender.try_send(request).is_err() {
                            output.error(
                                &request_id,
                                "session_unavailable",
                                "session queue is full or closed",
                            );
                        }
                    }
                    None => output.error(
                        &request.id,
                        "unknown_session",
                        "session is not owned by this host",
                    ),
                }
            }
            _ => output.error(&request.id, "unknown_method", "unknown host method"),
        }
    }
    stop.cancel();
    drop(sessions);
    for worker in workers {
        if worker.join().is_err() {
            failed.store(true, Ordering::SeqCst);
        }
    }
    drop(output);
    drop(tools);
    let drained = question_runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(3), async {
            for task in &mut question_tasks {
                let _ = task.await;
            }
        })
        .await
    });
    if drained.is_err() {
        for task in question_tasks {
            task.abort();
        }
        failed.store(true, Ordering::SeqCst);
    }
    question_runtime.shutdown_timeout(Duration::from_secs(3));
    drop(question_service);
    let _ = writer.join();
    if failed.load(Ordering::SeqCst) {
        std::process::exit(1);
    }
}
