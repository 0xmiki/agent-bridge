import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { Database } from "bun:sqlite";
import { BridgeHost } from "agent-bridge";

const [binary, upgrade] = process.argv.slice(2).map(path => resolve(path));
assert.ok(binary && upgrade);
const database = resolve("legacy.sqlite3");
let sql = new Database(database);
sql.exec(readFileSync("legacy-v1.sql", "utf8"));
const before = sql.query("SELECT * FROM agent_bridge_records").all();
sql.close();
const host = new BridgeHost(binary);
try {
  await assert.rejects(host.history(database, "legacy-session"), /migration/i);
  const migrated = Bun.spawnSync([upgrade, database]);
  assert.equal(migrated.exitCode, 0, migrated.stderr.toString());
  const history = await host.history(database, "legacy-session");
  assert.equal(history.records.length, 1);
  assert.equal(history.records[0]!.id, "legacy-record");
  sql = new Database(database);
  assert.deepEqual(sql.query("SELECT * FROM agent_bridge_records").all(), before);
  assert.deepEqual(sql.query("SELECT value FROM application_data").get(), { value: "keep" });
  assert.deepEqual(sql.query("PRAGMA user_version").get(), { user_version: 42 });
  assert.deepEqual(sql.query("SELECT version FROM agent_bridge_schema").get(), { version: 7 });
  sql.exec("UPDATE agent_bridge_schema SET version = 99");
  sql.close();
  await assert.rejects(host.history(database, "legacy-session"), /99/);
  assert.notEqual(Bun.spawnSync([upgrade, database]).exitCode, 0);
  sql = new Database(database, { readonly: true });
  assert.deepEqual(sql.query("SELECT version FROM agent_bridge_schema").get(), { version: 99 });
  assert.deepEqual(sql.query("SELECT * FROM agent_bridge_records").all(), before);
  sql.close();
} finally { await host.close(); }

const wire = Bun.spawn([binary], { stdin: "pipe", stdout: "pipe", stderr: "inherit" });
wire.stdin.write('{broken\n' + JSON.stringify({ version: 99, id: "future", method: "ping" }) + '\n' + JSON.stringify({ version: 1, id: "ping", method: "ping" }) + '\n');
wire.stdin.end();
const frames = (await new Response(wire.stdout).text()).trim().split("\n").map(line => JSON.parse(line));
assert.ok(frames.every(frame => frame.version === 1));
assert.ok(frames.some(frame => frame.code === "invalid_request"));
assert.equal(frames.find(frame => frame.id === "future")?.error.code, "unsupported_protocol");
assert.match(frames.find(frame => frame.id === "future")?.error.message, /version 1/);
assert.equal(frames.find(frame => frame.id === "ping")?.result.alive, true);
assert.equal(await wire.exited, 0);
console.log("Packaged database upgrade and wire compatibility passed");
