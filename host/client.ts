/** Experimental v1 client. No ACP SDK objects cross this boundary. */
import { spawn } from "bun";
import type { ReceiptView } from "./receipts";

export interface SessionOptions { database: string; workspace: string; executable: string; args?: string[]; env?: Record<string, string>; delete_session_on_close?: boolean }
export interface StoredRecord { id: string; session_id: string; run_id: string | null; actor: string; sequence: string; revision: string; state: "open" | "complete" | "interrupted"; payload: { type: string; data: unknown }; receipt?: ReceiptView | null }
export interface HistoryPage { records: StoredRecord[]; next_after: string | null; page_full: boolean }
export interface ChangeCursor { epoch: string; session_id: string; position: string }
export interface StatePage extends HistoryPage { cursor: ChangeCursor }
function startHost(binary: string) { return spawn([binary], { stdin: "pipe", stdout: "pipe", stderr: "inherit" }); }
export type RunEvent = Readonly<
  | { event: "text_delta"; session_id: string; run_id: string; text: string }
  | { event: "permission"; session_id: string; run_id: string; permission_id: string; title: string | null; options: readonly Readonly<{ id: string; label: string; effect: string }>[] }
  | { event: "run_finished"; session_id: string; run_id: string; status: string; reason: string | null; recording_error: string | null }>;
export interface SubscriptionOptions { maxEvents?: number; maxBytes?: number }
export interface RunSubscription extends AsyncIterable<RunEvent> { close(): void }
export class SubscriptionLaggedError extends Error {
  readonly code = "subscriber_lagged";
  constructor() { super("run subscriber exceeded its buffer; refresh saved state to recover"); this.name = "SubscriptionLaggedError"; }
}
type Finish = Extract<RunEvent, { event: "run_finished" }>;
export type PendingPermission = Extract<RunEvent, { event: "permission" }>;
function freezeEvent(event: RunEvent) {
  if (event.event === "permission") { event.options.forEach(Object.freeze); Object.freeze(event.options); }
  return Object.freeze(event);
}
function deferred<T>() { let resolve!: (value: T) => void; let reject!: (error: Error) => void; const promise = new Promise<T>((a, b) => { resolve = a; reject = b; }); promise.catch(() => {}); return { promise, resolve, reject }; }

class Events<T> implements AsyncIterable<T> {
  private items: {value:T; bytes:number}[] = [];
  private bytes = 0;
  private waiter?: ReturnType<typeof deferred<IteratorResult<T>>>;
  private done = false;
  private error?: Error;
  private claimed = false;
  private maxEvents: number;
  private maxBytes: number;
  constructor(private onClosed: () => void, options: SubscriptionOptions = {}) {
    this.maxEvents = options.maxEvents ?? 128;
    this.maxBytes = options.maxBytes ?? 1024 * 1024;
    if (!Number.isInteger(this.maxEvents) || this.maxEvents < 1 || this.maxEvents > 128 || !Number.isInteger(this.maxBytes) || this.maxBytes < 1 || this.maxBytes > 1024 * 1024) throw new Error("invalid subscription limits");
  }
  push(value: T, bytes: number) {
    if (this.done) return;
    if (this.waiter) { const waiter = this.waiter; this.waiter = undefined; waiter.resolve({ value, done: false }); }
    else {
      if (this.items.length >= this.maxEvents || this.bytes + bytes > this.maxBytes) { this.end(new SubscriptionLaggedError()); return; }
      this.items.push({value,bytes}); this.bytes += bytes;
    }
  }
  end(error?: Error) {
    if (this.done) return;
    this.done = true; this.error = error;
    if (error) { this.items = []; this.bytes = 0; }
    if (this.waiter) { if (error) this.waiter.reject(error); else this.waiter.resolve({ value: undefined as T, done: true }); this.waiter = undefined; }
    this.onClosed();
  }
  close() { this.items = []; this.bytes = 0; this.end(); }
  [Symbol.asyncIterator](): AsyncIterator<T> { if (this.claimed) throw new Error("events have one consumer"); this.claimed = true; return { next: () => {
    if (this.error) return Promise.reject(this.error);
    if (this.items.length) { const item = this.items.shift()!; this.bytes -= item.bytes; return Promise.resolve({ value: item.value, done: false }); }
    if (this.done) return Promise.resolve({ value: undefined as T, done: true });
    if (this.waiter) return Promise.reject(new Error("only one event consumer is supported"));
    this.waiter = deferred<IteratorResult<T>>(); return this.waiter.promise;
  }, return: async () => { this.close(); return {value:undefined as T, done:true}; } }; }
}

export class HostRun {
  private runId?: string;
  private subscribers = new Set<Events<RunEvent>>();
  readonly events: RunSubscription;
  private terminal?: Finish;
  private failure?: Error;
  private finished = deferred<Finish>();
  readonly completed = this.finished.promise;
  constructor(private host: BridgeHost, readonly sessionId: string, readonly started: Promise<{ run_id: string }>) { this.events = this.subscribe(); }
  /** Observe future events. The default events subscription starts with the run. */
  subscribe(options: SubscriptionOptions = {}): RunSubscription {
    if (this.subscribers.size >= 8) throw Object.assign(new Error("run subscriber limit reached"), {code:"subscriber_limit"});
    const queue = new Events<RunEvent>(() => this.subscribers.delete(queue), options);
    if (this.terminal) { queue.push(this.terminal, Buffer.byteLength(JSON.stringify(this.terminal))); queue.end(); }
    else if (this.failure) queue.end(this.failure);
    else this.subscribers.add(queue);
    return queue;
  }
  async cancel() { const { run_id } = await this.started; await this.host.request("cancel", { session_id: this.sessionId, run_id }); }
  async respond(permissionId: string, optionId: string | null) { const { run_id } = await this.started; await this.host.request("respond", { session_id: this.sessionId, run_id, permission_id: permissionId, option_id: optionId }); }
  /** Current live requests, not persisted-history replay. Attach an observer before querying. */
  async pendingPermissions(): Promise<readonly PendingPermission[]> {
    const { run_id } = await this.started;
    if (this.terminal) return [];
    try {
      const events: PendingPermission[] = await this.host.request("pending_permissions", {session_id:this.sessionId, run_id});
      for (const event of events) {
        if (event.event !== "permission" || event.session_id !== this.sessionId || event.run_id !== run_id) throw new Error("invalid pending permission");
        freezeEvent(event);
      }
      return Object.freeze(events);
    } catch (error) {
      if (["not_running", "stale_run"].includes((error as {code:string}).code)) return [];
      throw error;
    }
  }
  /** @internal */ bind(info: {run_id: string; session_id: string}) { if (typeof info.run_id !== "string" || info.session_id !== this.sessionId) throw new Error("invalid run acknowledgement"); this.runId = info.run_id; }
  /** @internal */ accept(event: RunEvent) {
    if (!this.runId || event.run_id !== this.runId || event.session_id !== this.sessionId || !["text_delta", "permission", "run_finished"].includes(event.event)) throw new Error("invalid run event");
    freezeEvent(event);
    const bytes = Buffer.byteLength(JSON.stringify(event));
    if (event.event === "run_finished") { this.terminal = event; this.finished.resolve(event); }
    for (const queue of this.subscribers) { queue.push(event, bytes); if (this.terminal) queue.end(); }
  }
  /** @internal */ fail(error: Error) { this.failure = error; for (const queue of this.subscribers) queue.end(error); this.finished.reject(error); }
}

export class HostSession {
  constructor(private host: BridgeHost, readonly id: string, readonly slotId: string, readonly database: string) {}
  run(prompt: string) { return this.host.startRun(this.id, prompt); }
  history(after?: string, limit = 1000) { return this.host.history(this.database, this.id, after, limit); }
  snapshot(cursor?: ChangeCursor, after?: string, limit = 100): Promise<StatePage> { return this.host.snapshot(this.database, this.id, cursor, after, limit); }
  changes(cursor: ChangeCursor, limit = 100): Promise<StatePage> { return this.host.changes(this.database, this.id, cursor, limit); }
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
  snapshot(database: string, sessionId: string, cursor?: ChangeCursor, after?: string, limit = 100): Promise<StatePage> { return this.request("snapshot", { database, session_id: sessionId, cursor, after, limit }); }
  changes(database: string, sessionId: string, cursor: ChangeCursor, limit = 100): Promise<StatePage> { return this.request("changes", { database, session_id: sessionId, cursor, limit }); }
  async close() { if (!this.closing && !this.error) { this.closing = true; await this.request("shutdown"); } this.process.stdin.end(); const code = await this.process.exited; if (code !== 0) throw new Error(`host exited (${code})`); }
}
