import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdtempSync, rmSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { Database } from "bun:sqlite";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { BridgeHost, type HostRun, type RunEvent, type StoredRecord } from "./client";

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
