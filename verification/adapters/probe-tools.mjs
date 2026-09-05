// A deliberately small MCP fixture, not an application tool server.
import { createInterface } from 'node:readline';
import { appendFileSync } from 'node:fs';
import { randomUUID } from 'node:crypto';

const ledger = process.argv[2];
if (!ledger) throw new Error('missing absolute ledger path');
const pending = new Map();
const send = (id, result) => process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id, result }) + '\n');
const record = (event) => appendFileSync(ledger, JSON.stringify(event) + '\n');
const inputSchema = { type: 'object', properties: {}, additionalProperties: false };
const tools = [
  { name: 'bridge_probe_token', description: 'Return a unique verification token. Call once and repeat the returned token exactly.', inputSchema },
  { name: 'bridge_probe_wait', description: 'Wait 30 seconds before returning a verification token. Used to test cancellation.', inputSchema },
];

createInterface({ input: process.stdin }).on('line', (line) => {
  const message = JSON.parse(line);
  const { id, method, params } = message;
  if (method === 'notifications/cancelled') {
    const active = pending.has(params.requestId);
    clearTimeout(pending.get(params.requestId));
    pending.delete(params.requestId);
    record({ event: active ? 'cancelled' : 'cancellation_received', id: params.requestId });
    return;
  }
  if (id === undefined) return;
  if (method === 'initialize') {
    send(id, { protocolVersion: params.protocolVersion, capabilities: { tools: {} }, serverInfo: { name: 'bridge-probe', version: '1.0.0' } });
  } else if (method === 'ping') {
    send(id, {});
  } else if (method === 'tools/list') {
    send(id, { tools });
  } else if (method === 'tools/call' && tools.some(tool => tool.name === params.name)) {
    const token = randomUUID();
    record({ event: 'started', tool: params.name, id, token });
    const finish = () => {
      pending.delete(id);
      record({ event: 'finished', tool: params.name, id, token });
      send(id, { content: [{ type: 'text', text: token }] });
    };
    if (params.name === 'bridge_probe_wait') pending.set(id, setTimeout(finish, 30_000));
    else finish();
  } else {
    process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id, error: { code: -32601, message: 'Unsupported fixture method or tool' } }) + '\n');
  }
}).on('close', () => {
  for (const timer of pending.values()) clearTimeout(timer);
});
