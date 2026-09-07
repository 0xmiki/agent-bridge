/** Experimental v1 client. No ACP SDK objects cross this boundary. */
import { spawn } from "bun";

export interface SessionOptions { database: string; workspace: string; executable: string; args?: string[]; env?: Record<string, string> }
export interface StoredRecord { id: string; session_id: string; run_id: string | null; actor: string; sequence: string; revision: string; state: "open" | "complete" | "interrupted"; payload: { type: string; data: unknown } }
export interface HistoryPage { records: StoredRecord[]; next_after: string | null; page_full: boolean }
function startHost(binary: string) { return spawn([binary], { stdin: "pipe", stdout: "pipe", stderr: "inherit" }); }
export type RunEvent =
  | { event: "text_delta"; session_id: string; run_id: string; text: string }
  | { event: "permission"; session_id: string; run_id: string; permission_id: string; title: string | null; options: { id: string; label: string; effect: string }[] }
  | { event: "run_finished"; session_id: string; run_id: string; status: string; reason: string | null; recording_error: string | null };
type Finish = Extract<RunEvent, { event: "run_finished" }>;
function deferred<T>() { let resolve!: (value: T) => void; let reject!: (error: Error) => void; const promise = new Promise<T>((a, b) => { resolve = a; reject = b; }); promise.catch(() => {}); return { promise, resolve, reject }; }

class Events<T> implements AsyncIterable<T> {
  private items: T[] = [];
  private waiter?: ReturnType<typeof deferred<IteratorResult<T>>>;
  private done = false;
  private error?: Error;
  private claimed = false;
  push(value: T) {
    if (this.done) return;
    if (this.waiter) { const waiter = this.waiter; this.waiter = undefined; waiter.resolve({ value, done: false }); }
    else { if (this.items.length >= 128) throw new Error("client event buffer exhausted"); this.items.push(value); }
  }
  end(error?: Error) { this.done = true; this.error = error; if (this.waiter) { if (error) this.waiter.reject(error); else this.waiter.resolve({ value: undefined as T, done: true }); this.waiter = undefined; } }
  [Symbol.asyncIterator](): AsyncIterator<T> { if (this.claimed) throw new Error("events have one consumer"); this.claimed = true; return { next: () => {
    if (this.error) return Promise.reject(this.error);
    if (this.items.length) return Promise.resolve({ value: this.items.shift()!, done: false });
    if (this.done) return Promise.resolve({ value: undefined as T, done: true });
    if (this.waiter) return Promise.reject(new Error("only one event consumer is supported"));
    this.waiter = deferred<IteratorResult<T>>(); return this.waiter.promise;
  } }; }
}

export class HostRun {
  private runId?: string;
  private queue = new Events<RunEvent>();
  readonly events: AsyncIterable<RunEvent> = this.queue;
  private finished = deferred<Finish>();
  readonly completed = this.finished.promise;
  constructor(private host: BridgeHost, readonly sessionId: string, readonly started: Promise<{ run_id: string }>) {}
  async cancel() { const { run_id } = await this.started; await this.host.request("cancel", { session_id: this.sessionId, run_id }); }
  async respond(permissionId: string, optionId: string | null) { const { run_id } = await this.started; await this.host.request("respond", { session_id: this.sessionId, run_id, permission_id: permissionId, option_id: optionId }); }
  /** @internal */ bind(info: {run_id: string; session_id: string}) { if (typeof info.run_id !== "string" || info.session_id !== this.sessionId) throw new Error("invalid run acknowledgement"); this.runId = info.run_id; }
  /** @internal */ accept(event: RunEvent) { if (!this.runId || event.run_id !== this.runId || event.session_id !== this.sessionId || !["text_delta", "permission", "run_finished"].includes(event.event)) throw new Error("invalid run event"); this.queue.push(event); if (event.event === "run_finished") { this.finished.resolve(event); this.queue.end(); } }
  /** @internal */ fail(error: Error) { this.queue.end(error); this.finished.reject(error); }
}

export class HostSession {
  constructor(private host: BridgeHost, readonly id: string, readonly slotId: string, readonly database: string) {}
  run(prompt: string) { return this.host.startRun(this.id, prompt); }
  history(after?: string, limit = 1000) { return this.host.history(this.database, this.id, after, limit); }
}

export class BridgeHost {
  private process: ReturnType<typeof startHost>;
  private nextId = 0;
  private pending = new Map<string, ReturnType<typeof deferred<any>>>();
  private runs = new Map<string, HostRun>();
  private readyState = deferred<void>();
  readonly ready = this.readyState.promise;
  private error?: Error;
  private closing = false;
  constructor(binary: string) {
    this.process = startHost(binary);
    const reading = this.read();
    reading.catch(error => { this.fail(error); this.process.stdin.end(); });
    this.process.exited.then(async code => { await reading.catch(() => {}); this.fail(new Error(`host exited (${code})`)); });
  }
  private fail(error: Error) { if (this.error) return; this.error = error; this.readyState.reject(error); for (const value of this.pending.values()) value.reject(error); this.pending.clear(); for (const run of this.runs.values()) run.fail(error); this.runs.clear(); }
  private async read() {
    const reader = this.process.stdout.getReader(); const decoder = new TextDecoder("utf-8", { fatal: true }); let buffer = "";
    try {
    for (;;) {
      const { value, done } = await reader.read(); if (done) break;
      buffer += decoder.decode(value, { stream: true });
      if (buffer.length > 2 * 1024 * 1024) throw new Error("oversized host output");
      for (;;) {
        const end = buffer.indexOf("\n"); if (end < 0) break;
        const frame = JSON.parse(buffer.slice(0, end)); buffer = buffer.slice(end + 1);
        if (frame.version !== 1) throw new Error("unsupported host protocol");
        if (typeof frame.id === "string") { const pending = this.pending.get(frame.id); if (!pending) throw new Error("uncorrelated host response"); this.pending.delete(frame.id); if (frame.ok) { this.runs.get(frame.id)?.bind(frame.result); pending.resolve(frame.result); } else pending.reject(Object.assign(new Error(frame.error.message), { code: frame.error.code })); }
        else if (frame.event === "ready") this.readyState.resolve();
        else if (frame.stream) { const run = this.runs.get(frame.stream); if (!run) throw new Error("uncorrelated run event"); run.accept(frame); if (frame.event === "run_finished") this.runs.delete(frame.stream); }
        else if (frame.event === "protocol_error") throw new Error(frame.code);
        else if (frame.event !== "session_error") throw new Error("unknown host event");
      }
    }
    if (buffer.trim()) throw new Error("truncated host frame");
    } catch (error) { await reader.cancel().catch(() => {}); throw error; }
    finally { reader.releaseLock(); }
  }
  private send(method: string, params: unknown, id: string) {
    if (this.error) return Promise.reject(this.error);
    if (this.closing && method !== "shutdown") return Promise.reject(new Error("host is closing"));
    if (this.pending.size >= 64) return Promise.reject(new Error("too many pending requests"));
    const pending = deferred<any>(); this.pending.set(id, pending);
    const timer = setTimeout(() => { if (this.pending.has(id)) { this.fail(new Error("host request timed out")); this.process.stdin.end(); } }, 45000);
    pending.promise.finally(() => clearTimeout(timer)).catch(() => {});
    try { this.process.stdin.write(JSON.stringify({ version: 1, id, method, params }) + "\n"); Promise.resolve(this.process.stdin.flush()).catch(error => this.fail(error)); }
    catch (error) { this.fail(error as Error); }
    return pending.promise;
  }
  /** Low-level versioned command access for contract tests. */
  request(method: string, params: unknown = {}) { return this.send(method, params, `request-${++this.nextId}`); }
  async createSession(options: SessionOptions) { await this.ready; const result = await this.request("create_session", options); return new HostSession(this, result.session_id, result.slot_id, options.database); }
  startRun(sessionId: string, prompt: string) {
    const id = `request-${++this.nextId}`;
    const start = deferred<{ run_id: string }>();
    const run = new HostRun(this, sessionId, start.promise); this.runs.set(id, run);
    this.send("run", { session_id: sessionId, prompt }, id).then(start.resolve, error => { start.reject(error); run.fail(error); this.runs.delete(id); });
    return run;
  }
  history(database: string, sessionId: string, after?: string, limit = 1000): Promise<HistoryPage> { return this.request("history", { database, session_id: sessionId, after, limit }); }
  async close() { if (!this.closing && !this.error) { this.closing = true; await this.request("shutdown"); } this.process.stdin.end(); const code = await this.process.exited; if (code !== 0) throw new Error(`host exited (${code})`); }
}
