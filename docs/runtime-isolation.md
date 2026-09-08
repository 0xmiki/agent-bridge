# Host storage isolation

This is the experimental host's bounded failure policy. It does not change the
synchronous `RecordStore` trait or make every storage adapter asynchronous.

## Ownership and deadlines

The host executes SQLite operations outside its provider runtimes. Each session has
a storage worker with eight queued commands; read requests run separately. H2 expanded
the queue to accommodate the recorder and up to four admitted tool callbacks. A shared
budget admits at most twelve actual storage workers, including work whose caller
has already timed out. Exhaustion returns `StorageOverloaded`; it does not spawn
replacement workers without a free slot.

Opening a store, each record operation, and each history/state query have a 500 ms
caller wait budget. SQLite's separate busy-handler budget remains 100 ms. A command
that performs several record operations can take longer than 500 ms overall.

Host writes for the same configured absolute database path are serialized inside
storage workers. The path registry does no filesystem resolution on control threads.
Aliases, symlinks, and other processes may still contend through SQLite; use one
consistent configured path. Read-only queries do not take this writer gate.

## Timeout means uncertainty

`StorageTimedOut` means the caller stopped waiting. It does not mean an in-progress
transaction was rolled back. A mutation may commit after its run reports `unknown`.
The host includes the recording error, retires the affected storage handle, and never
replays the prompt. Later calls through that handle return `StorageUnavailable`.
Work still waiting behind another writer is discarded after timeout, before execution.

Recording failure lets cancellation reach the provider. Run drop does not wait again
on a retired store. Provider shutdown remains independent of an outstanding storage
job; that job owns neither the provider nor the output channel. The host joins its
supervisors, not indefinitely blocked storage threads. Worker capacity is released
only when actual work finishes or panics.

Applications must reconcile uncertain outcomes from later evidence. Do not retry an
agent run merely because recording timed out. State reads can be retried using the
last committed projection/cursor pair. Exhausted capacity may require restarting the
host if underlying work never returns.

## Evidence and remaining limits

Fault tests use a controlled blocking store rather than attempting to hang a real
disk. While an agent-message write remains blocked, a recorded run times out,
cancellation reaches an ACP fixture, and the provider plus its descendant exit.
Another storage worker remains usable. Releasing the blocked store afterward proves
that the write can still commit and that no completion record or replay was invented.

Other tests cover blocked initialization, read-worker saturation, panics, and skipping
queued writes after their deadlines. Existing host tests cover shared DELETE/WAL
databases, stream routing, state restoration, and process ownership.

This is not a hard real-time guarantee. Kernel-level uninterruptible I/O, a stalled
allocator, whole-process scheduling failure, and platform-specific process termination
are not simulated. Standard threads cannot safely be force-killed individually.
OS failure testing remains release work; durable recovery remains H3. Independent
[Bun run observers](subscriptions.md) now have separate bounded queues. Shared
transport failures and provider-output scheduling fairness remain separate concerns.
