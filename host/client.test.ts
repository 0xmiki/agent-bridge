import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdtempSync, rmSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { Database } from "bun:sqlite";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { BridgeHost, type HostRun, type RunEvent, type StoredRecord } from "./client";
import { SessionState } from "./state";
import { readReceipt } from "./receipts";

const root = resolve(import.meta.dir, "..");
const binary = process.env.AGENT_BRIDGE_HOST ?? join(root, "target/debug/agent-bridge-host");
const directory = mkdtempSync(join(tmpdir(), "bridge-host-test-"));
const fixture = join(directory, "fixture");
beforeAll(() => {
  if (!existsSync(binary)) throw new Error("Build with cargo build --features host --bin agent-bridge-host first");
  const compiled = Bun.spawnSync(["rustc", "--edition=2024", join(root, "tests/support/acp_fixture.rs"), "-o", fixture]);
  if (compiled.exitCode) throw new Error(compiled.stderr.toString());
});
afterAll(() => rmSync(directory, { recursive: true, force: true }));
function options(name: string, mode = "chat", env: Record<string, string> = {}) { return { database: join(directory, `${name}.sqlite3`), workspace: directory, executable: fixture, args: [mode], env }; }
async function text(run: HostRun) { let output = ""; for await (const event of run.events) { if (event.event === "text_delta") output += event.text; } return output; }

async function within<T>(promise: Promise<T>, milliseconds = 2000): Promise<T> {
  let timer: ReturnType<typeof setTimeout>;
  try { return await Promise.race([promise, new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error(`deadline exceeded: ${milliseconds}ms`)), milliseconds); })]); }
  finally { clearTimeout(timer!); }
}
async function until(predicate: () => boolean, milliseconds = 2000) {
  const deadline = performance.now() + milliseconds;
  while (!predicate()) {
    if (performance.now() >= deadline) throw new Error(`condition deadline exceeded: ${milliseconds}ms`);
    await Bun.sleep(5);
  }
}
async function collect(run: HostRun, timeline: string[] = []) {
  const { run_id } = await run.started;
  const events: RunEvent[] = [];
  for await (const event of run.events) {
    expect(event.session_id).toBe(run.sessionId);
    expect(event.run_id).toBe(run_id);
    events.push(event);
    if (event.event === "text_delta") timeline.push(event.text);
  }
  const terminal = events.filter(event => event.event === "run_finished");
  expect(terminal).toHaveLength(1);
  expect(await run.completed).toEqual(terminal[0]!);
  return events;
}
function messageText(record: StoredRecord) {
  const data = record.payload.data as { message: { content: { type: string; data: string }[] } };
  return data.message.content.map(part => { expect(part.type).toBe("text"); return part.data; }).join("");
}

test("distinct interleaved sessions retain their own context and exact history after restart", async () => {
  const host = new BridgeHost(binary);
  const saved: { database: string; id: string; records: StoredRecord[] }[] = [];
  const gates = [join(directory, "gate-a"), join(directory, "gate-b")];
  const nonces = ["alpha-719", "beta-283"];
  try {
    const sessions = await Promise.all(gates.map((gate, i) => host.createSession(options(`history-${i}`, "host-state", { BRIDGE_TEST_GATE: gate }))));
    expect(sessions[0]!.id).not.toBe(sessions[1]!.id);
    const runIds: string[][] = [[], []];
    for (let turn = 0; turn < 2; turn++) {
      const timeline: string[] = [];
      const runs = sessions.map((session, i) => session.run(turn === 0 ? nonces[i]! : "recall"));
      const collecting = runs.map(run => collect(run, timeline));
      for (let chunk = 0; chunk < 2; chunk++) {
        await until(() => gates.every(gate => existsSync(`${gate}.ready-${turn}-${chunk}`)));
        for (let i = 0; i < 2; i++) {
          writeFileSync(`${gates[i]}.go-${turn}-${chunk}`, "go");
          await until(() => timeline.length === chunk * 2 + i + 1);
        }
      }
      expect(timeline).toEqual([...nonces, ...Array(2).fill(turn === 0 ? ":first" : ":recall")]);
      await Promise.all(collecting);
      for (let i = 0; i < 2; i++) {
        const finish = await runs[i]!.completed;
        expect(finish.status).toBe("completed"); expect(finish.recording_error).toBeNull();
        runIds[i]!.push(finish.run_id);
      }
    }
    for (let i = 0; i < 2; i++) {
      const session = sessions[i]!;
      const { records } = await session.history();
      expect(records.every(record => record.session_id === session.id && record.state === "complete")).toBe(true);
      const messages = records.filter(record => record.payload.type === "message");
      expect(messages.map(messageText)).toEqual([nonces[i]!, `${nonces[i]}:first`, "recall", `${nonces[i]}:recall`]);
      expect(messages.map(record => record.actor)).toEqual(["user", "assistant", "user", "assistant"]);
      expect(messages.map(record => record.run_id)).toEqual([runIds[i]![0], runIds[i]![0], runIds[i]![1], runIds[i]![1]]);
      expect(records.filter(record => record.payload.type === "run_finished").map(record => record.payload.data)).toEqual([{ reason: { type: "completed" } }, { reason: { type: "completed" } }]);
      saved.push({ database: session.database, id: session.id, records });
    }
    expect(await host.request("ping")).toEqual({ alive: true });
    await expect(host.request("unsupported")).rejects.toHaveProperty("code", "unknown_method");
  } finally { await host.close(); }
  const reopened = new BridgeHost(binary);
  try {
    for (const snapshot of saved) {
      expect((await reopened.history(snapshot.database, snapshot.id)).records).toEqual(snapshot.records);
      const paged: StoredRecord[] = []; let after: string | undefined;
      for (;;) {
        const page = await reopened.history(snapshot.database, snapshot.id, after, 2);
        paged.push(...page.records);
        if (!page.page_full) break;
        after = page.next_after!;
      }
      expect(paged).toEqual(snapshot.records);
    }
  }
  finally { await reopened.close(); }
}, 30000);

test("SQLite lock failure is explicit while another session runs and cancels within deadlines", async () => {
  const host = new BridgeHost(binary);
  const gate = join(directory, "locked-gate");
  const requests = join(directory, "locked-requests");
  const config = options("locked", "host-state", { BRIDGE_TEST_GATE: gate, BRIDGE_TEST_MESSAGES: requests });
  let lock: Database | undefined;
  try {
    const [blocked, other] = await Promise.all([host.createSession(config), host.createSession(options("lock-other", "cancel"))]);
    const run = blocked.run("locked-719");
    const captured = collect(run);
    await until(() => existsSync(`${gate}.ready-0-0`));
    lock = new Database(config.database);
    lock.exec("BEGIN IMMEDIATE");
    writeFileSync(`${gate}.go-0-0`, "go");
    writeFileSync(`${gate}.go-0-1`, "go");
    expect(await within(host.request("ping"))).toEqual({ alive: true });
    const healthy = other.run("wait");
    const events = healthy.events[Symbol.asyncIterator]();
    expect((await within(events.next())).value?.event).toBe("text_delta");
    await within(healthy.cancel());
    await within((async () => { while (!(await events.next()).done) {} })());
    expect((await healthy.completed).status).toBe("cancelled");
    await within(captured);
    const finish = await run.completed;
    expect(finish.status).toBe("unknown");
    expect(finish.recording_error).toBeString();
    expect(finish.recording_error?.toLowerCase()).toContain("busy");
    lock.exec("ROLLBACK"); lock.close(); lock = undefined;
    expect((await blocked.history()).records.some(record => record.payload.type === "run_finished")).toBe(false);
    await expect(blocked.run("retry").started).rejects.toHaveProperty("code", "start_failed");
    const prompts = readFileSync(requests, "utf8").trim().split("\n").map(line => JSON.parse(line)).filter(frame => frame.method === "session/prompt");
    expect(prompts).toHaveLength(1);
  } finally {
    if (lock) { lock.exec("ROLLBACK"); lock.close(); }
    await host.close();
  }
}, 15000);

test("cancellation and permission responses are routed without ACP objects", async () => {
  const host = new BridgeHost(binary);
  try {
    const session = await host.createSession(options("cancel", "cancel"));
    const run = session.run("wait");
    for await (const event of run.events) { if (event.event === "text_delta") await run.cancel(); }
    expect((await run.completed).status).toBe("cancelled");
    const permissions = await host.createSession(options("permission", "permission"));
    const requested = permissions.run("ask"); let answered = false;
    for await (const event of requested.events) {
      if (event.event === "permission") { answered = true; await requested.respond(event.permission_id, event.options.find(option => option.effect === "allow_once")!.id); }
    }
    expect(answered).toBe(true); expect((await requested.completed).status).toBe("completed");
  } finally { await host.close(); }
}, 30000);

test("host close owns an active provider process", async () => {
  const pidFile = join(directory, "owned.pid");
  const host = new BridgeHost(binary);
  const session = await host.createSession(options("owned", "cancel", { BRIDGE_TEST_PID: pidFile }));
  const run = session.run("wait"); await run.started;
  const pid = Number(readFileSync(pidFile, "utf8"));
  await host.close();
  expect(() => process.kill(pid, 0)).toThrow();
}, 30000);

test("foreign, stale, invalid, and duplicate permission responses cannot consume valid requests", async () => {
  const host = new BridgeHost(binary);
  try {
    const sessions = await Promise.all([host.createSession(options("permission-a", "permissions")), host.createSession(options("permission-b", "permissions"))]);
    const runs = sessions.map(session => session.run("ask"));
    const iterators = runs.map(run => run.events[Symbol.asyncIterator]());
    async function permission(index: number) {
      for (;;) {
        const item = await within(iterators[index]!.next());
        if (item.done) throw new Error("permission missing");
        if (item.value.event === "permission") return item.value;
      }
    }
    const [a, b] = await Promise.all([permission(0), permission(1)]);
    // A foreign token must not resolve B's own pending permission.
    await expect(runs[1]!.respond(a!.permission_id, "allow")).rejects.toHaveProperty("code", "invalid_response");
    await expect(host.request("respond", { session_id: sessions[1]!.id, run_id: a!.run_id, permission_id: b!.permission_id, option_id: "allow" })).rejects.toHaveProperty("code", "stale_run");
    await expect(runs[0]!.respond(a!.permission_id, "invented-option")).rejects.toHaveProperty("code", "invalid_response");
    await runs[0]!.respond(a!.permission_id, "allow");
    await expect(runs[0]!.respond(a!.permission_id, "allow")).rejects.toHaveProperty("code", "invalid_response");
    await runs[1]!.respond(b!.permission_id, "allow");
    for (let index = 0; index < 2; index++) {
      const second = await permission(index);
      await runs[index]!.respond(second.permission_id, "allow");
      let terminals = 0;
      for (;;) {
        const item = await within(iterators[index]!.next());
        if (item.done) break;
        if (item.value.event === "run_finished") terminals++;
      }
      expect(terminals).toBe(1);
      expect((await runs[index]!.completed).status).toBe("completed");
      expect((await runs[index]!.completed).recording_error).toBeNull();
      const records = (await sessions[index]!.history()).records;
      expect(records.filter(record => record.payload.type === "decision")).toHaveLength(2);
    }
  } finally { await host.close(); }
}, 15000);

test("failed provider startup leaves the host usable", async () => {
  const host = new BridgeHost(binary);
  try {
    await expect(host.createSession({ ...options("missing"), executable: join(directory, "missing-executable") })).rejects.toThrow();
    expect(await host.request("ping")).toEqual({ alive: true });
    const session = await host.createSession(options("after-failure"));
    expect(await text(session.run("hello"))).toBe("Hello world");
  } finally { await host.close(); }
}, 30000);

function stopped(pid: number) {
  try { return readFileSync(`/proc/${pid}/stat`, "utf8").split(") ").at(-1)!.startsWith("Z"); }
  catch (error) { if ((error as NodeJS.ErrnoException).code === "ENOENT") return true; throw error; }
}

for (const fault of ["stdin-eof", "stdout-disconnect", "stalled-output-eof", "stalled-output"] as const) {
  test.skipIf(process.platform !== "linux")(fault + " stops the host, provider, and descendant within five seconds", async () => {
    const files = Object.fromEntries(["parent", "child", "host", "ready", "streaming"].map(name => [name, join(directory, fault + "." + name)]));
    const consumer = Bun.spawn([fixture, "host-consumer", binary, join(directory, fault + ".sqlite3"), directory], {
      stdin: "pipe", stdout: "ignore", stderr: "inherit",
      env: { ...process.env, BRIDGE_TEST_PID: files.parent!, BRIDGE_TEST_DESCENDANT: files.child!,
        BRIDGE_TEST_HOST_PID: files.host!, BRIDGE_TEST_CONSUMER_READY: files.ready!, BRIDGE_TEST_STREAMING: files.streaming!,
        BRIDGE_TEST_PROVIDER_MODE: fault === "stdin-eof" ? "host-tree-cancel" : fault === "stalled-output" ? "host-tree-burst" : "host-tree-flood" },
    });
    try {
      await until(() => existsSync(files.ready!) && existsSync(files.child!));
      if (fault !== "stdin-eof") await until(() => existsSync(files.streaming!));
      const command = fault === "stdout-disconnect" ? "close" : fault === "stalled-output" ? "wait" : "eof";
      consumer.stdin.write(command + "\n"); await consumer.stdin.flush();
      expect(await within(consumer.exited, 5000)).toBe(fault === "stdin-eof" ? 0 : 1);
      await until(() => [files.host!, files.parent!, files.child!].every(path => stopped(Number(readFileSync(path, "utf8")))));
    } finally {
      consumer.stdin.end();
      // Failed fault probes must not leak intentionally blocked processes.
      for (const path of [files.host!, files.parent!, files.child!]) {
        if (existsSync(path)) {
          const pid = Number(readFileSync(path, "utf8"));
          if (!stopped(pid)) { try { process.kill(pid, "SIGKILL"); } catch {} }
        }
      }
      if (consumer.exitCode === null) consumer.kill();
      await consumer.exited;
    }
  }, 15000);
}

test("wire versions and malformed requests fail clearly without launching a provider", async () => {
  const host = Bun.spawn([binary], { stdin: "pipe", stdout: "pipe", stderr: "inherit" });
  host.stdin.write('{broken\n' + JSON.stringify({ version: 99, id: "bad-version", method: "ping" }) + '\n' + JSON.stringify({ version: 1, id: "ping", method: "ping" }) + '\n' + JSON.stringify({ version: 1, id: "stop", method: "shutdown" }) + '\n');
  host.stdin.end();
  const frames = (await new Response(host.stdout).text()).trim().split('\n').map(line => JSON.parse(line));
  expect(frames.find(frame => frame.event === "protocol_error")?.code).toBe("invalid_request");
  expect(frames.find(frame => frame.id === "bad-version")?.error.code).toBe("unsupported_protocol");
  expect(frames.find(frame => frame.id === "ping")?.result.alive).toBe(true);
  expect(await host.exited).toBe(0);
}, 10000);

test("state projection catches old-record updates and resumes from a saved checkpoint", async () => {
  const host = new BridgeHost(binary);
  const gate = join(directory, "state-gate");
  const config = options("state", "host-state", { BRIDGE_TEST_GATE: gate });
  let saved: ReturnType<SessionState["checkpoint"]>;
  let expected: StoredRecord[] = []; let id = "";
  try {
    const session = await host.createSession(config); id = session.id;
    const timeline: string[] = [];
    const run = session.run("state-319"); const collected = collect(run, timeline);
    await until(() => existsSync(`${gate}.ready-0-0`));
    writeFileSync(`${gate}.go-0-0`, "go");
    await until(() => timeline.length === 1);
    const state = new SessionState(config.database, id);
    await state.sync(host, 1);
    const initial = state.checkpoint()!;
    expect(state.items.some(item => item.kind === "message" && item.role === "agent")).toBe(true);
    writeFileSync(`${gate}.go-0-1`, "go"); await collected;
    const update = await session.changes(initial.cursor);
    expect(update.records.some(row => initial.records.some(old => old.id === row.id && BigInt(row.revision) > BigInt(old.revision)))).toBe(true);
    // A failed fetch must not advance the durable projection cursor.
    const changes = host.changes.bind(host);
    host.changes = async () => { throw new Error("simulated read failure"); };
    await expect(state.sync(host)).rejects.toThrow("simulated read failure");
    expect(state.checkpoint()).toEqual(initial);
    host.changes = changes;
    await state.sync(host, 1);
    expect(state.items.filter(item => item.kind === "message" && item.role === "agent").map(item => item.kind === "message" && item.text)).toEqual(["state-319:first"]);
    expected = (await session.history()).records;
    expect(state.items.map(item => item.record)).toEqual(expected);
    saved = state.checkpoint();
    await state.sync(host, 1); expect(state.checkpoint()).toEqual(saved);
    const foreign = await host.createSession(options("state-foreign"));
    await expect(foreign.changes(saved!.cursor)).rejects.toThrow("another session");
  } finally { await host.close(); }
  const reopened = new BridgeHost(binary);
  try {
    const restored = new SessionState(config.database, id, saved!);
    await restored.sync(reopened, 1);
    expect(restored.items.map(item => item.record)).toEqual(expected);
  } finally { await reopened.close(); }
}, 15000);

test("disposable sessions request provider cleanup and retain local records", async () => {
  const marker = join(directory, "deleted");
  const host = new BridgeHost(binary);
  const config = options("cleanup", "chat", { BRIDGE_TEST_DELETED: marker });
  let id = "";
  try {
    const session = await host.createSession({ ...config, delete_session_on_close: true }); id = session.id;
    expect(await text(session.run("hello"))).toBe("Hello world");
  } finally { await host.close(); }
  expect(readFileSync(marker, "utf8")).toBe('"native-1"');
  const reopened = new BridgeHost(binary);
  try { expect((await reopened.history(config.database, id)).records.some(row => row.payload.type === "run_finished")).toBe(true); }
  finally { await reopened.close(); }
}, 15000);

test("provider cleanup failure makes close fail instead of claiming success", async () => {
  const host = new BridgeHost(binary);
  await host.createSession({ ...options("cleanup-error", "host-delete-error"), delete_session_on_close: true });
  await expect(host.close()).rejects.toThrow("host exited (1)");
}, 15000);

test("history and state readers do not contend for an external writer's reserved lock", async () => {
  const host = new BridgeHost(binary);
  const config = options("reader-lock");
  let lock: Database | undefined;
  try {
    const session = await host.createSession(config);
    await text(session.run("saved"));
    const expected = (await session.history()).records;
    const state = new SessionState(config.database, session.id);
    await state.sync(host);
    lock = new Database(config.database); lock.exec("BEGIN IMMEDIATE");
    // Reserved write locks permit reads of committed state in SQLite's default journal mode.
    expect((await within(session.history())).records).toEqual(expected);
    expect((await within(session.snapshot())).records).toEqual(expected);
    await within(state.sync(host));
    expect(state.items.map(item => item.record)).toEqual(expected);
    expect(await within(host.request("ping"))).toEqual({ alive: true });
    lock.exec("ROLLBACK");
    const checkpoint = state.checkpoint();
    lock.exec("BEGIN EXCLUSIVE");
    await expect(within(state.sync(host))).rejects.toHaveProperty("code", "history_failed");
    expect(state.checkpoint()).toEqual(checkpoint);
    expect(await within(host.request("ping"))).toEqual({ alive: true });
    lock.exec("ROLLBACK"); lock.close(); lock = undefined;
    await state.sync(host);
    expect(state.checkpoint()).toEqual(checkpoint);
  } finally {
    if (lock) { lock.exec("ROLLBACK"); lock.close(); }
    await host.close();
  }
}, 15000);

for (const journal of ["DELETE", "WAL"] as const) {
  test(`three sessions share ${journal} storage while independent projections refresh`, async () => {
    const host = new BridgeHost(binary);
    const database = join(directory, `shared-${journal}.sqlite3`);
    const sql = new Database(database, {create:true});
    expect((sql.query(`PRAGMA journal_mode=${journal}`).get() as {journal_mode:string}).journal_mode).toBe(journal.toLowerCase());
    sql.close();
    const gates = [0, 1, 2].map(i => join(directory, `shared-${journal}-${i}`));
    const checkpoints: {id:string; saved:NonNullable<ReturnType<SessionState["checkpoint"]>>; expected:StoredRecord[]}[] = [];
    try {
      const sessions = await Promise.all(gates.map((gate, i) => host.createSession({...options(`shared-${journal}-${i}`, "host-state", {BRIDGE_TEST_GATE:gate}), database})));
      const states = sessions.map(session => new SessionState(database, session.id));
      for (let turn = 0; turn < 2; turn++) {
        const runs = sessions.map((session, i) => session.run(turn === 0 ? `shared-${i}` : "recall"));
        const results = runs.map(run => collect(run));
        for (let chunk = 0; chunk < 2; chunk++) {
          await until(() => gates.every(gate => existsSync(`${gate}.ready-${turn}-${chunk}`)));
          gates.forEach(gate => writeFileSync(`${gate}.go-${turn}-${chunk}`, "go"));
          await within(Promise.all(states.map(state => state.sync(host, 2))));
        }
        const events = await Promise.all(results);
        for (let i = 0; i < sessions.length; i++) {
          expect(events[i]!.filter(event => event.event === "text_delta").map(event => event.event === "text_delta" && event.text).join("")).toBe(`shared-${i}:${turn === 0 ? "first" : "recall"}`);
          expect((await runs[i]!.completed).status).toBe("completed");
          expect((await runs[i]!.completed).recording_error).toBeNull();
        }
      }
      await Promise.all(states.map(state => state.sync(host, 2)));
      for (let i = 0; i < sessions.length; i++) {
        const expected = (await sessions[i]!.history()).records;
        expect(states[i]!.items.map(item => item.record)).toEqual(expected);
        expect(new Set(expected.map(record => record.id)).size).toBe(expected.length);
        checkpoints.push({id:sessions[i]!.id, saved:states[i]!.checkpoint()!, expected});
      }
    } finally { await host.close(); }
    const reopened = new BridgeHost(binary);
    try {
      for (const {id, saved, expected} of checkpoints) {
        const state = new SessionState(database, id, saved);
        await state.sync(reopened, 2);
        expect(state.items.map(item => item.record)).toEqual(expected);
      }
    } finally { await reopened.close(); }
    const check = new Database(database, {readonly:true});
    expect((check.query("PRAGMA journal_mode").get() as {journal_mode:string}).journal_mode).toBe(journal.toLowerCase());
    check.close();
  }, 30000);
}

test("an unread run stream cannot disconnect another run or lose durable history", async () => {
  const host = new BridgeHost(binary);
  try {
    const [noisy, healthy] = await Promise.all([host.createSession(options("unread", "host-subscriber")), host.createSession(options("unread-healthy", "cancel"))]);
    const unread = noisy.run("stream");
    const normal = healthy.run("hello");
    const normalEvents = collect(normal);
    expect((await within(unread.completed, 5000)).status).toBe("completed");
    expect(await host.request("ping")).toEqual({alive:true});
    await within(normal.cancel());
    expect((await within(normalEvents)).filter(event => event.event === "text_delta").map(event => event.event === "text_delta" && event.text)).toEqual(["Hello "]);
    expect((await normal.completed).status).toBe("cancelled");
    await expect(unread.events[Symbol.asyncIterator]().next()).rejects.toHaveProperty("code", "subscriber_lagged");
    const state = new SessionState(noisy.database, noisy.id);
    await state.sync(host);
    expect(state.items.filter(item => item.kind === "message" && item.role === "agent").map(item => item.kind === "message" && item.text)).toEqual([Array.from({length:160}, (_, i) => `${i}|`).join("")]);
    expect((await unread.completed).recording_error).toBeNull();
  } finally { await host.close(); }
}, 15000);

test("slow subscribers hit their own event and byte limits while a fast subscriber receives every event", async () => {
  const host = new BridgeHost(binary);
  const gate = join(directory, "subscriber-gate");
  try {
    const session = await host.createSession(options("subscribers", "host-subscriber", {BRIDGE_TEST_SUBSCRIBER_GATE:gate}));
    const run = session.run("stream");
    const slow = run.subscribe();
    const tiny = run.subscribe({maxBytes:512});
    const timeline: string[] = [];
    const fast = collect(run, timeline);
    await until(() => timeline.length === 8 && existsSync(`${gate}.ready`));
    // Only eight events exist: this overflow must be the byte budget, not 128 events.
    await expect(tiny[Symbol.asyncIterator]().next()).rejects.toHaveProperty("code", "subscriber_lagged");
    expect(await host.request("ping")).toEqual({alive:true});
    writeFileSync(`${gate}.go`, "go");
    const events = await within(fast, 5000);
    expect(timeline).toEqual(Array.from({length:160}, (_, i) => `${i}|`));
    expect(events.filter(event => event.event === "run_finished")).toHaveLength(1);
    expect((await run.completed).status).toBe("completed");
    await expect(slow[Symbol.asyncIterator]().next()).rejects.toHaveProperty("code", "subscriber_lagged");
    const late = run.subscribe();
    const observed = []; for await (const event of late) observed.push(event);
    expect(observed).toEqual([await run.completed]);
  } finally { writeFileSync(`${gate}.go`, "go"); await host.close(); }
}, 15000);

test("leaving one observer does not cancel a run or let it mutate another observer's permissions", async () => {
  const host = new BridgeHost(binary);
  try {
    const session = await host.createSession(options("observer-permission", "permission"));
    const run = session.run("ask");
    const observer = run.subscribe();
    // Async-iterator return on break must release this subscription only.
    for await (const event of run.events) {
      expect(event.event).toBe("text_delta");
      expect(() => Object.assign(event, {text:"corrupted"})).toThrow();
      break;
    }
    let decisions = 0; let terminals = 0;
    for await (const event of observer) {
      if (event.event === "permission") {
        expect(() => Object.assign(event.options[0]!, {id:"spoofed"})).toThrow();
        expect(() => (event.options as unknown[]).push({id:"spoofed"})).toThrow();
        await run.respond(event.permission_id, "allow"); decisions++;
      }
      if (event.event === "run_finished") terminals++;
    }
    expect(decisions).toBe(1); expect(terminals).toBe(1);
    expect((await run.completed).status).toBe("completed");
    expect((await run.completed).recording_error).toBeNull();
    expect(await host.request("ping")).toEqual({alive:true});
  } finally { await host.close(); }
}, 15000);

test("subscriber admission and unsubscribe are bounded independently of run ownership", async () => {
  const host = new BridgeHost(binary);
  try {
    const session = await host.createSession(options("observer-limit", "cancel"));
    const run = session.run("wait");
    await run.started;
    expect(() => run.subscribe({maxEvents:129})).toThrow("invalid subscription limits");
    expect(() => run.subscribe({maxBytes:0})).toThrow("invalid subscription limits");
    const watchers = Array.from({length:7}, () => run.subscribe());
    expect(() => run.subscribe()).toThrow("subscriber limit");
    watchers[0]!.close(); watchers[0]!.close();
    const replacement = run.subscribe();
    const iterator = replacement[Symbol.asyncIterator]();
    const pending = iterator.next();
    replacement.close();
    // It may have received the first delta before close; the next read must be done.
    await pending;
    expect((await iterator.next()).done).toBe(true);
    for (const watcher of watchers) watcher.close();
    run.events.close();
    expect((await watchers[0]![Symbol.asyncIterator]().next()).done).toBe(true);
    await within(run.cancel());
    expect((await within(run.completed)).status).toBe("cancelled");
    expect(await host.request("ping")).toEqual({alive:true});
  } finally { await host.close(); }
}, 15000);

test("reattached observers recover a pending permission after lag without replaying the run", async () => {
  const host = new BridgeHost(binary);
  const marker = join(directory, "lagged-permission");
  const requests = join(directory, "lagged-permission-requests");
  try {
    const session = await host.createSession(options("lagged-permission", "host-subscriber-permission", {BRIDGE_TEST_PERMISSION_READY:marker, BRIDGE_TEST_MESSAGES:requests}));
    const run = session.run("ask");
    await until(() => existsSync(marker));
    const reattached = run.subscribe();
    const pending = await within((async () => {
      for (;;) { const pending = await run.pendingPermissions(); if (pending.length) return pending; await Bun.sleep(5); }
    })());
    await expect(run.events[Symbol.asyncIterator]().next()).rejects.toHaveProperty("code", "subscriber_lagged");
    expect(pending).toHaveLength(1);
    expect(pending[0]!.session_id).toBe(session.id);
    expect(pending[0]!.options.map(option => option.id)).toEqual(["allow", "reject"]);
    await run.respond(pending[0]!.permission_id, "allow");
    const seen = []; for await (const event of reattached) seen.push(event);
    expect(seen.filter(event => event.event === "run_finished")).toEqual([await run.completed]);
    expect((await run.completed).status).toBe("completed");
    expect(await run.pendingPermissions()).toEqual([]);
    const wire = readFileSync(requests, "utf8").trim().split("\n").map(line => JSON.parse(line));
    expect(wire.filter(frame => frame.method === "session/prompt")).toHaveLength(1);
    expect(wire.filter(frame => frame.method === "session/cancel")).toHaveLength(0);
  } finally { await host.close(); }
}, 15000);

test("typed receipt views survive host transport, preserve precision, and refresh legacy checkpoints", async () => {
  const host = new BridgeHost(binary);
  const config = options("receipt-views");
  let checkpoint: ReturnType<SessionState["checkpoint"]>; let sessionId = "";
  try {
    const session = await host.createSession(config); sessionId = session.id;
    const vectors = readFileSync(join(root, "verification/receipts.json"), "utf8");
    const expected = JSON.parse(vectors) as {expected:string}[];
    const sql = new Database(config.database);
    // JSON stays inside SQLite here: JS must not round the large revision before
    // Rust reads the fixture and emits its decimal-string projection.
    sql.exec("BEGIN IMMEDIATE");
    try {
      sql.query(`INSERT INTO agent_bridge_records (id,session_id,sequence,actor_id,payload_json,state,revision)
        SELECT 'receipt-' || key, ?, key, 'host', json_object('version',2,'data',json_object(
          'type','extension','data',json_object('namespace',coalesce(json_extract(value,'$.namespace'),'agent_bridge'),
          'name',json_extract(value,'$.name'),'data',json(json_extract(value,'$.data'))))), 'complete',0
        FROM json_each(?)`).run(session.id, vectors);
      sql.query("UPDATE agent_bridge_sessions SET next_sequence = ? WHERE id = ?").run(expected.length, session.id);
      sql.exec("COMMIT");
    } catch(error) { sql.exec("ROLLBACK"); throw error; }
    finally { sql.close(); }
    const state = new SessionState(config.database, session.id);
    await state.sync(host, 3);
    const records = state.items.map(item => item.record);
    const kinds: string[] = records.map(record => readReceipt(record)?.kind ?? "none");
    expect(kinds).toEqual(expected.map(vector => vector.expected));
    const input = readReceipt(records[0]!);
    expect(input?.kind === "input" && input.data.state === "prepared" && input.data.wire_text).toBe("wire-marker");
    expect(input?.kind === "input" && input.data.state === "prepared" && input.data.wire_bytes).toBe("11");
    expect("wire_text" in (records[0]!.receipt!.data)).toBe(false);
    const contract = readReceipt(records[5]!);
    expect(contract?.kind === "result_contract" && contract.data.wire_text).toBe("contract-wire-marker");
    const validation = readReceipt(records[6]!);
    expect(validation?.kind === "result_validation" && validation.data.sources[0]!.revision).toBe("9007199254740993");
    expect(validation?.kind === "result_validation" && validation.data.native_enforcement).toBe(false);
    expect(state.items[11]!.kind).toBe("receipt");
    expect(readReceipt(records[11]!)?.kind).toBe("invalid");
    expect((records[10]!.payload.data as {data:{extra:{keep:boolean}}}).data.extra.keep).toBe(true);
    checkpoint = state.checkpoint();
    const legacy = {...checkpoint!, projection_version:undefined, records:records.map(({receipt, ...record}) => record)};
    const migrated = new SessionState(config.database, session.id, legacy);
    expect(migrated.cursor).toBeUndefined();
    await migrated.sync(host, 3);
    expect(migrated.checkpoint()).toEqual(checkpoint);
    expect(() => new SessionState(config.database, session.id, {...checkpoint!, projection_version:99})).toThrow("unsupported state projection version");
  } finally { await host.close(); }
  const reopened = new BridgeHost(binary);
  try {
    const state = new SessionState(config.database, sessionId, checkpoint!);
    await state.sync(reopened, 3);
    expect(state.checkpoint()).toEqual(checkpoint);
  } finally { await reopened.close(); }
}, 15000);
