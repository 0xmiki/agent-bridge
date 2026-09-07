import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdtempSync, rmSync, existsSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { BridgeHost, type HostRun } from "./client";

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

test("two sessions correlate streams; a session continues; history reopens without a provider", async () => {
  const host = new BridgeHost(binary);
  let sessionId = ""; const config = options("history");
  try {
    const [a, b] = await Promise.all([host.createSession(config), host.createSession(options("other"))]);
    expect(a.id).not.toBe(b.id); sessionId = a.id;
    expect(await Promise.all([text(a.run("first")), text(b.run("other"))])).toEqual(["Hello world", "Hello world"]);
    expect(await text(a.run("second"))).toBe("Hello world");
    const page = await a.history();
    expect(page.records.filter(record => record.payload.type === "message").length).toBe(4);
    expect(await host.request("ping")).toEqual({ alive: true });
    await expect(host.request("unsupported")).rejects.toHaveProperty("code", "unknown_method");
  } finally { await host.close(); }
  const reopened = new BridgeHost(binary);
  try { expect((await reopened.history(config.database, sessionId)).records.length).toBeGreaterThanOrEqual(6); }
  finally { await reopened.close(); }
}, 30000);

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

test("failed provider startup leaves the host usable", async () => {
  const host = new BridgeHost(binary);
  try {
    await expect(host.createSession({ ...options("missing"), executable: join(directory, "missing-executable") })).rejects.toThrow();
    expect(await host.request("ping")).toEqual({ alive: true });
    const session = await host.createSession(options("after-failure"));
    expect(await text(session.run("hello"))).toBe("Hello world");
  } finally { await host.close(); }
}, 30000);

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
