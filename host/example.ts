import { BridgeHost } from "./client";
import { resolve } from "node:path";

const [database, workspace, executable, ...args] = process.argv.slice(2);
if (!database || !workspace || !executable) throw new Error("bun host/example.ts <database> <workspace> <ACP executable> [args...]");
const binary = process.env.AGENT_BRIDGE_HOST ?? resolve(import.meta.dir, "../target/debug/agent-bridge-host");
const host = new BridgeHost(binary);
let sessionId = "";
try {
  const session = await host.createSession({ database: resolve(database), workspace: resolve(workspace), executable, args }); sessionId = session.id;
  let turn = 0;
  for (const prompt of ["Remember the phrase violet lighthouse. Reply only remembered. Do not use tools.", "What phrase did I ask you to remember? Reply only the phrase. Do not use tools."]) {
    const run = session.run(prompt);
    let answer = "";
    for await (const event of run.events) {
      if (event.event === "text_delta") { answer += event.text; process.stdout.write(event.text); }
      if (event.event === "permission") await run.respond(event.permission_id, null);
    }
    console.log("\n", (await run.completed).status);
    if (++turn === 2 && !answer.toLowerCase().includes("violet lighthouse")) throw new Error("second turn did not preserve the phrase");
  }
  const cancelled = session.run("List the integers from 1 to 1000, one per line. Do not use tools.");
  let requested = false;
  for await (const event of cancelled.events) {
    if (event.event === "text_delta" && !requested) { requested = true; await cancelled.cancel().catch(() => {}); }
    if (event.event === "permission") await cancelled.respond(event.permission_id, null);
  }
  const ended = await cancelled.completed;
  if (ended.recording_error || !["completed", "cancelled"].includes(ended.status)) throw new Error("third turn did not settle normally");
  console.log("Cancellation race outcome:", ended.status);
} finally { await host.close(); }
const reopened = new BridgeHost(binary);
try { console.log("Saved records:", (await reopened.history(resolve(database), sessionId)).records.length); }
finally { await reopened.close(); }
