use super::*;
use agent_bridge::records::{MemoryStore, MessageKind};
use std::sync::{Condvar, Mutex};
use std::time::Instant;

#[derive(Clone, Default)]
struct Gate {
    state: Arc<(Mutex<bool>, Condvar)>,
    entered: Arc<AtomicBool>,
}
impl Gate {
    fn wait(&self) {
        self.entered.store(true, Ordering::SeqCst);
        let (lock, changed) = &*self.state;
        // A broken deadline must fail the test rather than hang the whole suite.
        let (released, _) = changed
            .wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(10), |open| !*open)
            .unwrap();
        assert!(*released, "test did not release stalled storage");
    }
    fn open(&self) {
        let (lock, changed) = &*self.state;
        *lock.lock().unwrap() = true;
        changed.notify_all();
    }
}
struct Release(Gate);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.open();
    }
}
fn wait_until(predicate: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !predicate() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn timed_out_reads_keep_capacity_until_work_actually_stops() {
    let budget = Budget::new(1);
    let gate = Gate::default();
    let _release = Release(gate.clone());
    let blocked = gate.clone();
    let start = Instant::now();
    assert_eq!(
        budget.execute(move || {
            blocked.wait();
            7
        }),
        Err(StoreError::StorageTimedOut)
    );
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(gate.entered.load(Ordering::SeqCst));
    assert_eq!(budget.active.load(Ordering::SeqCst), 1);
    assert_eq!(
        budget.execute(|| panic!("overloaded work must not start")),
        Err(StoreError::StorageOverloaded)
    );
    gate.open();
    wait_until(|| budget.active.load(Ordering::SeqCst) == 0);
    assert_eq!(budget.execute(|| 9), Ok(9));
}

#[test]
fn stalled_initialization_is_bounded_and_does_not_leak_replacement_workers() {
    let budget = Budget::new(1);
    let gate = Gate::default();
    let _release = Release(gate.clone());
    let blocked = gate.clone();
    assert!(matches!(
        Worker::<()>::start(&budget, move || {
            blocked.wait();
            Ok(())
        }),
        Err(StoreError::StorageTimedOut)
    ));
    assert!(matches!(
        Worker::<()>::start(&budget, || Ok(())),
        Err(StoreError::StorageOverloaded)
    ));
    gate.open();
    wait_until(|| budget.active.load(Ordering::SeqCst) == 0);
    let worker = Worker::start(&budget, || Ok(42)).unwrap();
    assert_eq!(worker.call(|value| Ok(*value)), Ok(42));
    drop(worker);
    wait_until(|| budget.active.load(Ordering::SeqCst) == 0);
}

#[test]
fn panicked_storage_retires_its_handle_and_releases_capacity() {
    let budget = Budget::new(1);
    let worker = Worker::start(&budget, || Ok(())).unwrap();
    assert_eq!(
        worker.call::<()>(|_| panic!("injected storage panic")),
        Err(StoreError::StorageUnavailable)
    );
    assert_eq!(worker.call(|_| Ok(())), Err(StoreError::StorageUnavailable));
    wait_until(|| budget.active.load(Ordering::SeqCst) == 0);
}

#[test]
fn timed_out_work_waiting_for_its_database_does_not_start_later() {
    let budget = Budget::new(1);
    let path = PathBuf::from("/test/database");
    let gate = budget.writer_gate(&path).unwrap();
    assert!(Arc::ptr_eq(&gate, &budget.writer_gate(&path).unwrap()));
    assert!(!Arc::ptr_eq(
        &gate,
        &budget.writer_gate(&PathBuf::from("/test/other")).unwrap()
    ));
    let mutations = Arc::new(AtomicUsize::new(0));
    let observed = mutations.clone();
    let worker =
        Worker::start_serialized(&budget, Some(gate.clone()), move || Ok(observed)).unwrap();
    let held = gate.lock().unwrap();
    assert_eq!(
        worker.call(|count| {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }),
        Err(StoreError::StorageTimedOut)
    );
    drop(held);
    wait_until(|| budget.active.load(Ordering::SeqCst) == 0);
    assert_eq!(mutations.load(Ordering::SeqCst), 0);
}

struct PausedStore {
    memory: MemoryStore,
    gate: Gate,
}
impl RecordStore for PausedStore {
    fn create_session(&self, id: SessionId) -> Result<(), StoreError> {
        self.memory.create_session(id)
    }
    fn register_run(&self, spec: RunSpec) -> Result<bool, StoreError> {
        self.memory.register_run(spec)
    }
    fn insert(&self, draft: Draft) -> Result<Arc<Snapshot>, StoreError> {
        if matches!(
            &draft.payload,
            Payload::Message {
                kind: MessageKind::Agent,
                ..
            }
        ) {
            self.gate.wait();
        }
        self.memory.insert(draft)
    }
    fn checkpoint(
        &self,
        id: &RecordId,
        revision: u64,
        payload: Payload,
        state: RecordState,
    ) -> Result<Arc<Snapshot>, StoreError> {
        self.memory.checkpoint(id, revision, payload, state)
    }
    fn resolve(
        &self,
        request: &RecordId,
        revision: u64,
        decision: Draft,
    ) -> Result<Arc<Snapshot>, StoreError> {
        self.memory.resolve(request, revision, decision)
    }
    fn get(&self, id: &RecordId) -> Result<Arc<Snapshot>, StoreError> {
        self.memory.get(id)
    }
    fn get_run(&self, id: &RunId) -> Result<RunSpec, StoreError> {
        self.memory.get_run(id)
    }
    fn list(
        &self,
        session: &SessionId,
        after: Option<u64>,
        limit: usize,
    ) -> Result<Vec<Arc<Snapshot>>, StoreError> {
        self.memory.list(session, after, limit)
    }
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stalled_recording_retires_the_run_and_cleans_provider_tree_before_storage_returns() {
    use agent_bridge::acp::{AcpConnection, AcpLaunch, RecordActors, RecordingError};
    use agent_bridge::{ActorId, SlotId};
    let directory =
        std::env::temp_dir().join(format!("bridge-stalled-storage-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let fixture = directory.join("fixture");
    assert!(
        std::process::Command::new("rustc")
            .args([
                "--edition=2024",
                concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/acp_fixture.rs"),
                "-o"
            ])
            .arg(&fixture)
            .status()
            .unwrap()
            .success()
    );
    let parent = directory.join("parent.pid");
    let child = directory.join("child.pid");
    let requests = directory.join("requests.jsonl");
    let connection = AcpConnection::connect(
        AcpLaunch::new(&fixture)
            .arg("host-tree-cancel")
            .env("BRIDGE_TEST_PID", parent.to_string_lossy())
            .env("BRIDGE_TEST_DESCENDANT", child.to_string_lossy())
            .env("BRIDGE_TEST_MESSAGES", requests.to_string_lossy()),
    )
    .await
    .unwrap();
    let session_id = SessionId::new("stalled").unwrap();
    let mut session = connection
        .new_session(
            session_id.clone(),
            SlotId::new("slot").unwrap(),
            directory.clone(),
            vec![],
        )
        .await
        .unwrap();
    let budget = Budget::new(2);
    let gate = Gate::default();
    let _release = Release(gate.clone());
    let memory = MemoryStore::default();
    let inner = PausedStore {
        memory: memory.clone(),
        gate: gate.clone(),
    };
    let store = Store {
        worker: Worker::start(&budget, move || Ok(inner)).unwrap(),
    };
    let healthy = Worker::start(&budget, || Ok(MemoryStore::default())).unwrap();
    let mut run = session
        .start_recorded_run(
            RunId::new("run").unwrap(),
            "wait",
            &store,
            RecordActors {
                user: ActorId::new("user").unwrap(),
                agent: ActorId::new("agent").unwrap(),
                host: ActorId::new("host").unwrap(),
            },
        )
        .unwrap();
    let start = Instant::now();
    assert!(matches!(
        run.next().await,
        Err(RecordingError::Store(StoreError::StorageTimedOut))
    ));
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(gate.entered.load(Ordering::SeqCst));
    assert_eq!(
        healthy.call(|s| s.create_session(SessionId::new("healthy").unwrap())),
        Ok(())
    );
    let start = Instant::now();
    drop(run);
    assert!(start.elapsed() < Duration::from_millis(100));
    assert!(
        session
            .start_run(RunId::new("retry").unwrap(), "must not replay")
            .is_err()
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if std::fs::read_to_string(&requests)
                .unwrap()
                .contains("session/cancel")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("cancellation must reach the provider while storage remains blocked");
    drop(session);
    tokio::time::timeout(Duration::from_secs(2), connection.shutdown())
        .await
        .unwrap()
        .unwrap();
    for path in [&parent, &child] {
        let pid = std::fs::read_to_string(path).unwrap();
        wait_until(
            || match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
                Ok(stat) => stat
                    .rsplit_once(')')
                    .unwrap()
                    .1
                    .trim_start()
                    .starts_with('Z'),
                Err(error) => error.kind() == std::io::ErrorKind::NotFound,
            },
        );
    }
    assert_eq!(budget.active.load(Ordering::SeqCst), 2);
    assert!(
        !memory
            .list(&session_id, None, 100)
            .unwrap()
            .iter()
            .any(|r| matches!(
                r.record.payload,
                Payload::Message {
                    kind: MessageKind::Agent,
                    ..
                }
            ))
    );
    // Timeout is uncertainty, not rollback: the in-flight write can commit later.
    gate.open();
    wait_until(|| budget.active.load(Ordering::SeqCst) == 1);
    let records = memory.list(&session_id, None, 100).unwrap();
    assert!(records.iter().any(|r| matches!(
        r.record.payload,
        Payload::Message {
            kind: MessageKind::Agent,
            ..
        }
    )));
    assert!(
        !records
            .iter()
            .any(|r| matches!(r.record.payload, Payload::RunFinished { .. }))
    );
    assert_eq!(
        store.list(&session_id, None, 100).unwrap_err(),
        StoreError::StorageUnavailable
    );
    drop(healthy);
    wait_until(|| budget.active.load(Ordering::SeqCst) == 0);
    std::fs::remove_dir_all(directory).unwrap();
}
