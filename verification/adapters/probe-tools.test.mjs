import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';

test('MCP fixture proves tool execution and handles cancellation', { timeout: 5000 }, async (t) => {
  const directory = mkdtempSync(join(tmpdir(), 'bridge-mcp-test-'));
  const ledger = join(directory, 'calls.jsonl');
  const child = spawn(process.execPath, [new URL('./probe-tools.mjs', import.meta.url).pathname, ledger], { stdio: ['pipe', 'pipe', 'inherit'] });
  const exited = once(child, 'exit');
  t.after(async () => { child.kill(); await exited; rmSync(directory, { recursive: true }); });
  const replies = new Map();
  createInterface({ input: child.stdout }).on('line', (line) => {
    const message = JSON.parse(line);
    replies.get(message.id)?.(message);
    replies.delete(message.id);
  });
  const send = (message) => child.stdin.write(JSON.stringify({ jsonrpc: '2.0', ...message }) + '\n');
  const request = (id, method, params) => new Promise((resolve) => { replies.set(id, resolve); send({ id, method, params }); });
  assert.equal((await request(1, 'initialize', { protocolVersion: '2025-06-18' })).result.protocolVersion, '2025-06-18');
  send({ method: 'notifications/initialized' });
  assert.equal((await request(2, 'tools/list', {})).result.tools.length, 2);
  const token = (await request(3, 'tools/call', { name: 'bridge_probe_token', arguments: {} })).result.content[0].text;
  const events = () => readFileSync(ledger, 'utf8').trim().split('\n').map(JSON.parse);
  assert.equal(events().find(event => event.event === 'finished').token, token);
  send({ id: 4, method: 'tools/call', params: { name: 'bridge_probe_wait', arguments: {} } });
  // A ping response establishes that the preceding tool request was processed.
  await request(5, 'ping', {});
  assert(events().some(event => event.id === 4 && event.event === 'started'));
  send({ method: 'notifications/cancelled', params: { requestId: 4 } });
  await request(6, 'ping', {});
  assert(events().some(event => event.id === 4 && event.event === 'cancelled'));
  assert(!events().some(event => event.id === 4 && event.event === 'finished'));
  assert.equal((await request(7, 'tools/call', { name: 'unknown' })).error.code, -32601);
  child.stdin.end();
  const [code] = await exited;
  assert.equal(code, 0);
});
