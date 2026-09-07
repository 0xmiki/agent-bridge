//! Experimental owned ACP host. Stdout is exclusively protocol v1 JSON-lines.
mod output;
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
        "actor":record.actor.as_str(),"sequence":record.sequence.to_string(),"revision":value.revision.to_string(),"state":value.state,"payload":record.payload})
}
fn history(request: &Request) -> Result<Value, Box<dyn std::error::Error>> {
    let store = SqliteStore::open_with_busy_timeout(
        string(&request.params, "database")?,
        STORAGE_LOCK_WAIT,
    )?;
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
        let store = SqliteStore::open_with_busy_timeout(&open.database, STORAGE_LOCK_WAIT)
            .map_err(|e| e.to_string())?;
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
        let mut session = tokio::select! {
            _ = output.stop.cancelled() => return Ok::<_, String>(()),
            result = connection.new_session(SessionId::new(&session_id).unwrap(), SlotId::new(&slot_id).unwrap(), open.workspace, vec![]) => result.map_err(|e|e.to_string())?,
        };
        output.ok(&create_id,json!({"session_id":session_id,"slot_id":slot_id,"configuration":session.configuration().values}));
        loop {
            let request = tokio::select! { _ = output.stop.cancelled()=>break, request=commands.recv()=>match request { Some(request)=>request,None=>break } };
            if request.method != "run" { output.error(&request.id,"not_running","session has no active run"); continue; }
            let prompt = match string(&request.params,"prompt") { Ok(prompt)=>prompt,Err(error)=>{output.error(&request.id,"invalid_params",error);continue;} };
            let run_id = identity("run");
            let mut run = match session.start_recorded_run(RunId::new(&run_id).unwrap(),prompt,&store,RecordActors {
                user:ActorId::new("user").unwrap(),agent:ActorId::new("assistant").unwrap(),host:ActorId::new("host").unwrap(),
            }) { Ok(run)=>run,Err(error)=>{output.error(&request.id,"start_failed",error);continue;} };
            output.ok(&request.id,json!({"run_id":run_id,"session_id":session_id}));
            let mut permissions: HashMap<String, agent_bridge::acp::PermissionId> = HashMap::new();
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
                            "cancel" => match run.cancel() { Ok(())=>output.ok(&command.id,json!({"cancellation_requested":true})),Err(error)=>output.error(&command.id,"cancel_failed",error) },
                            "respond" => {
                                let result = (|| {
                                    let token=string(&command.params,"permission_id")?;
                                    let permission=permissions.get(token).ok_or("permission is not pending")?;
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
                            permissions.insert(token.clone(),id);
                            output.emit(json!({"event":"permission","stream":request.id,"session_id":session_id,"run_id":run_id,"permission_id":token,
                                "title":permission.tool_call.fields.title,"options":permission.options.iter().map(|option|json!({"id":option.option_id.to_string(),"label":option.name,"effect":option.kind})).collect::<Vec<_>>()}));
                        }
                        Ok(Some(AcpEvent::Finished(value))) => reason=Some(value),
                        Ok(Some(_))=>{}, Ok(None)=>break,
                        Err(error)=>{failure=Some(error.to_string());break;}
                    }
                }
            }
            let status=if failure.is_some() { "unknown" } else { state(run.run().status()) };
            drop(run);
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
    let stop = CancellationToken::new();
    let failed = Arc::new(AtomicBool::new(false));
    let (sender, receiver) = mpsc::sync_channel::<Value>(128);
    let output = Output {
        sender,
        stop: stop.clone(),
        failed: failed.clone(),
    };
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
                let worker_id = id.clone();
                let worker_output = output.clone();
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
                workers.push(std::thread::spawn(move || {
                    struct Permit(Arc<std::sync::atomic::AtomicUsize>);
                    impl Drop for Permit {
                        fn drop(&mut self) {
                            self.0.fetch_sub(1, Ordering::SeqCst);
                        }
                    }
                    let _permit = Permit(reads);
                    match history(&request) {
                        Ok(value) => output.ok(&request.id, value),
                        Err(error) => output.error(&request.id, "history_failed", error),
                    }
                }));
            }
            "run" | "cancel" | "respond" => {
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
    let _ = writer.join();
    if failed.load(Ordering::SeqCst) {
        std::process::exit(1);
    }
}
