//! Bounded waits around synchronous storage. A timed-out write may still commit.
use agent_bridge::records::{
    Draft, Payload, RecordState, RecordStore, Snapshot, SqliteStore, StoreError,
};
use agent_bridge::{RecordId, RunId, RunSpec, SessionId};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard, Weak,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

pub const DEADLINE: Duration = Duration::from_millis(500);

#[derive(Clone)]
pub struct Budget {
    active: Arc<AtomicUsize>,
    limit: usize,
    writers: Arc<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>>,
}
struct Permit(Arc<AtomicUsize>);
impl Drop for Permit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
impl Budget {
    pub fn new(limit: usize) -> Self {
        Self {
            active: Arc::new(AtomicUsize::new(0)),
            limit,
            writers: Arc::new(Mutex::new(HashMap::new())),
        }
    }
    fn reserve(&self) -> Result<Permit, StoreError> {
        self.active
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n < self.limit).then_some(n + 1)
            })
            .map_err(|_| StoreError::StorageOverloaded)?;
        Ok(Permit(self.active.clone()))
    }
    fn writer_gate(&self, path: &PathBuf) -> Result<Arc<Mutex<()>>, StoreError> {
        let mut gates = self
            .writers
            .lock()
            .map_err(|_| StoreError::StorageUnavailable)?;
        gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = gates.get(path).and_then(Weak::upgrade) {
            return Ok(gate);
        }
        let gate = Arc::new(Mutex::new(()));
        gates.insert(path.clone(), Arc::downgrade(&gate));
        Ok(gate)
    }
    /// The permit stays with actual work, even after the caller stops waiting.
    pub fn execute<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, StoreError> {
        let permit = self.reserve()?;
        let (tx, rx) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("bridge-storage-read".into())
            .spawn(move || {
                let _permit = permit;
                let _ = tx.send(work());
            })
            .map_err(|_| StoreError::StorageUnavailable)?;
        receive(rx, DEADLINE)
    }
}

fn receive<T>(rx: mpsc::Receiver<T>, deadline: Duration) -> Result<T, StoreError> {
    rx.recv_timeout(deadline).map_err(|error| match error {
        mpsc::RecvTimeoutError::Timeout => StoreError::StorageTimedOut,
        mpsc::RecvTimeoutError::Disconnected => StoreError::StorageUnavailable,
    })
}
type Job<S> = Box<dyn FnOnce(&S) + Send>;
fn enter(gate: &Option<Arc<Mutex<()>>>) -> Result<Option<MutexGuard<'_, ()>>, StoreError> {
    gate.as_ref()
        .map(|gate| gate.lock().map_err(|_| StoreError::StorageUnavailable))
        .transpose()
}

struct Worker<S> {
    jobs: mpsc::SyncSender<Job<S>>,
    failed: Arc<AtomicBool>,
}
impl<S: Send + 'static> Worker<S> {
    #[cfg(test)]
    fn start(
        budget: &Budget,
        factory: impl FnOnce() -> Result<S, StoreError> + Send + 'static,
    ) -> Result<Self, StoreError> {
        Self::start_serialized(budget, None, factory)
    }
    fn start_serialized(
        budget: &Budget,
        gate: Option<Arc<Mutex<()>>>,
        factory: impl FnOnce() -> Result<S, StoreError> + Send + 'static,
    ) -> Result<Self, StoreError> {
        let permit = budget.reserve()?;
        let (jobs, rx) = mpsc::sync_channel::<Job<S>>(1);
        let (ready, initialized) = mpsc::sync_channel(1);
        let failed = Arc::new(AtomicBool::new(false));
        let stop = failed.clone();
        std::thread::Builder::new()
            .name("bridge-storage-write".into())
            .spawn(move || {
                let _permit = permit;
                let result = (|| {
                    let _guard = enter(&gate)?;
                    if stop.load(Ordering::SeqCst) {
                        return Err(StoreError::StorageUnavailable);
                    }
                    factory()
                })();
                let store = match result {
                    Ok(store) => store,
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                };
                if ready.send(Ok(())).is_err() {
                    return;
                }
                while let Ok(job) = rx.recv() {
                    let Ok(_guard) = enter(&gate) else {
                        break;
                    };
                    // A job may have timed out while waiting behind another writer.
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    job(&store);
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                }
            })
            .map_err(|_| StoreError::StorageUnavailable)?;
        if let Err(error) = receive(initialized, DEADLINE).and_then(|result| result) {
            failed.store(true, Ordering::SeqCst);
            return Err(error);
        }
        Ok(Self { jobs, failed })
    }
    fn call<T: Send + 'static>(
        &self,
        work: impl FnOnce(&S) -> Result<T, StoreError> + Send + 'static,
    ) -> Result<T, StoreError> {
        if self.failed.load(Ordering::SeqCst) {
            return Err(StoreError::StorageUnavailable);
        }
        let (tx, rx) = mpsc::sync_channel(1);
        self.jobs
            .try_send(Box::new(move |store| {
                let _ = tx.send(work(store));
            }))
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => StoreError::StorageOverloaded,
                mpsc::TrySendError::Disconnected(_) => StoreError::StorageUnavailable,
            })?;
        match receive(rx, DEADLINE) {
            Ok(result) => result,
            Err(error) => {
                self.failed.store(true, Ordering::SeqCst);
                Err(error)
            }
        }
    }
}
impl<S> Drop for Worker<S> {
    fn drop(&mut self) {
        self.failed.store(true, Ordering::SeqCst);
    }
}

pub struct Store<S = SqliteStore> {
    worker: Worker<S>,
}
impl Store {
    pub fn open(path: PathBuf, budget: &Budget) -> Result<Self, StoreError> {
        let gate = budget.writer_gate(&path)?;
        Ok(Self {
            worker: Worker::start_serialized(budget, Some(gate), move || {
                SqliteStore::open_with_busy_timeout(path, super::STORAGE_LOCK_WAIT)
            })?,
        })
    }
}
impl<S: RecordStore + 'static> RecordStore for Store<S> {
    fn create_session(&self, id: SessionId) -> Result<(), StoreError> {
        self.worker.call(move |s| s.create_session(id))
    }
    fn register_run(&self, spec: RunSpec) -> Result<bool, StoreError> {
        self.worker.call(move |s| s.register_run(spec))
    }
    fn insert(&self, draft: Draft) -> Result<Arc<Snapshot>, StoreError> {
        self.worker.call(move |s| s.insert(draft))
    }
    fn checkpoint(
        &self,
        id: &RecordId,
        revision: u64,
        payload: Payload,
        state: RecordState,
    ) -> Result<Arc<Snapshot>, StoreError> {
        let id = id.clone();
        self.worker
            .call(move |s| s.checkpoint(&id, revision, payload, state))
    }
    fn resolve(
        &self,
        request: &RecordId,
        revision: u64,
        decision: Draft,
    ) -> Result<Arc<Snapshot>, StoreError> {
        let request = request.clone();
        self.worker
            .call(move |s| s.resolve(&request, revision, decision))
    }
    fn get(&self, id: &RecordId) -> Result<Arc<Snapshot>, StoreError> {
        let id = id.clone();
        self.worker.call(move |s| s.get(&id))
    }
    fn get_run(&self, id: &RunId) -> Result<RunSpec, StoreError> {
        let id = id.clone();
        self.worker.call(move |s| s.get_run(&id))
    }
    fn list(
        &self,
        session: &SessionId,
        after: Option<u64>,
        limit: usize,
    ) -> Result<Vec<Arc<Snapshot>>, StoreError> {
        let session = session.clone();
        self.worker.call(move |s| s.list(&session, after, limit))
    }
}

#[cfg(test)]
mod tests;
