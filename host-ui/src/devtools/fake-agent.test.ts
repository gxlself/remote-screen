// @vitest-environment node
import { connect, type Socket } from 'node:net';
import { writeFileSync } from 'node:fs';
import { stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';

import { startFakeAgent, type FakeAgent } from './fake-agent';

interface TestClient {
  socket: Socket;
  next(): Promise<Record<string, unknown>>;
  closed(): Promise<void>;
  send(line: string): void;
  close(): void;
}

interface Notification {
  jsonrpc: '2.0';
  method: string;
  params: Record<string, unknown>;
}

const agents: FakeAgent[] = [];
const clients: TestClient[] = [];
let socketCounter = 0;

Object.defineProperty(globalThis, 'localStorage', {
  configurable: true,
  value: { clear: () => undefined },
});
Object.defineProperty(globalThis, 'location', {
  configurable: true,
  value: { hash: '' },
});

function socketPath(): string {
  socketCounter += 1;
  return join(tmpdir(), `rs-fa-${process.pid}-${socketCounter}.sock`);
}

function staleSocketPath(): string {
  const path = socketPath();
  writeFileSync(path, '');
  return path;
}

async function launch(options: Parameters<typeof startFakeAgent>[0]): Promise<FakeAgent> {
  const agent = await startFakeAgent(options);
  agents.push(agent);
  return agent;
}

function connectClient(path: string): Promise<TestClient> {
  return new Promise<TestClient>((resolveConnection, rejectConnection) => {
    const socket = connect(path);
    const queued: Record<string, unknown>[] = [];
    const waiters: Array<(value: Record<string, unknown>) => void> = [];
    const closeWaiters: Array<() => void> = [];
    let buffered = '';

    socket.setEncoding('utf8');
    socket.once('close', () => {
      for (const resolveClosed of closeWaiters.splice(0)) {
        resolveClosed();
      }
    });
    socket.on('data', (chunk: string) => {
      buffered += chunk;
      for (;;) {
        const newline = buffered.indexOf('\n');
        if (newline < 0) {
          break;
        }
        const line = buffered.slice(0, newline).trim();
        buffered = buffered.slice(newline + 1);
        if (line.length === 0) {
          continue;
        }
        const value = JSON.parse(line) as Record<string, unknown>;
        const waiter = waiters.shift();
        if (waiter) {
          waiter(value);
        } else {
          queued.push(value);
        }
      }
    });

    const client: TestClient = {
      socket,
      next: () =>
        new Promise<Record<string, unknown>>((resolveNext, rejectNext) => {
          const value = queued.shift();
          if (value) {
            resolveNext(value);
            return;
          }
          const timer = setTimeout(
            () => rejectNext(new Error('timed out waiting for response')),
            2000,
          );
          waiters.push((nextValue) => {
            clearTimeout(timer);
            resolveNext(nextValue);
          });
        }),
      send: (line: string) => socket.write(line),
      closed: () =>
        new Promise<void>((resolveClosed) => {
          if (socket.destroyed) {
            resolveClosed();
          } else {
            closeWaiters.push(resolveClosed);
          }
        }),
      close: () => socket.destroy(),
    };
    clients.push(client);
    socket.once('connect', () => setImmediate(() => resolveConnection(client)));
    socket.once('error', rejectConnection);
  });
}

async function request(
  client: TestClient,
  method: string,
  params?: unknown,
): Promise<Record<string, unknown>> {
  client.send(`${JSON.stringify({ jsonrpc: '2.0', id: 1, method, params })}\n`);
  return client.next();
}

async function nextNotification(client: TestClient): Promise<Notification> {
  const message = await client.next();
  expect(message.id).toBeUndefined();
  expect(typeof message.method).toBe('string');
  return message as unknown as Notification;
}

async function waitForClientCount(agent: FakeAgent, expected: number): Promise<void> {
  for (let attempt = 0; attempt < 200; attempt += 1) {
    if (agent.clientCount() === expected) {
      return;
    }
    await new Promise((resolveWait) => setTimeout(resolveWait, 5));
  }
  throw new Error(`timed out waiting for ${expected} clients`);
}

afterEach(async () => {
  for (const client of clients.splice(0)) {
    client.close();
  }
  for (const agent of agents.splice(0)) {
    await agent.close();
  }
});

describe('fake agent', () => {
  it('answers status.get in the idle scenario', async () => {
    const path = socketPath();
    await launch({ socketPath: path });
    const client = await connectClient(path);

    client.send('{"jsonrpc":"2.0","id":1,"method":"status.get"}\n');
    const response = await client.next();

    expect(response.id).toBe(1);
    expect((response.result as Record<string, unknown>).version).toBe('0.3.1');
    expect((response.result as Record<string, unknown>).sessions).toEqual([]);
  });

  it('serves the controlled scenario and maps not_found errors', async () => {
    const path = socketPath();
    await launch({ socketPath: path, scenario: 'controlled' });
    const client = await connectClient(path);

    const sessions = await request(client, 'sessions.list', null);
    expect((sessions.result as Record<string, unknown>).sessions).toHaveLength(1);

    client.send(
      '{"id":2,"method":"sessions.disconnect","params":{"session_id":"nope"},"jsonrpc":"2.0"}\n',
    );
    const response = await client.next();

    expect((response.error as Record<string, unknown>).code).toBe(-32000);
    expect(((response.error as Record<string, unknown>).data as Record<string, unknown>).code).toBe(
      'not_found',
    );
  });

  it('returns JSON-RPC parse and request errors', async () => {
    const path = socketPath();
    await launch({ socketPath: path });
    const client = await connectClient(path);

    client.send('not json\n');
    expect(await client.next()).toMatchObject({ id: null, error: { code: -32700 } });

    client.send('{"jsonrpc":"1.0","id":3,"method":"x"}\n');
    expect(await client.next()).toMatchObject({ id: 3, error: { code: -32600 } });

    client.send('{"jsonrpc":"2.0","id":4,"method":"does.not.exist"}\n');
    expect(await client.next()).toMatchObject({ id: 4, error: { code: -32601 } });
  });

  it('handles pipelined requests in order', async () => {
    const path = socketPath();
    await launch({ socketPath: path });
    const client = await connectClient(path);

    client.send(
      '{"jsonrpc":"2.0","id":10,"method":"status.get"}\n' +
        '{"jsonrpc":"2.0","id":11,"method":"sessions.list"}\n',
    );

    const first = await client.next();
    const second = await client.next();
    expect(first.id).toBe(10);
    expect(second.id).toBe(11);
  });

  it('skips empty lines and drops a connection over the 1 MiB line limit', async () => {
    const path = socketPath();
    await launch({ socketPath: path });
    const client = await connectClient(path);

    client.send('\n\n{"jsonrpc":"2.0","id":12,"method":"status.get"}\n');
    expect(await client.next()).toMatchObject({ id: 12 });

    const oversized = await connectClient(path);
    oversized.send(`${'x'.repeat(1024 * 1024 + 1)}\n`);
    await oversized.closed();
  });

  it('starts a controller and broadcasts sessionsChanged', async () => {
    const path = socketPath();
    const agent = await launch({ socketPath: path });
    const first = await connectClient(path);
    const second = await connectClient(path);
    await waitForClientCount(agent, 2);

    expect(agent.command('start')).toBe(true);
    expect(await nextNotification(first)).toMatchObject({
      method: 'event.sessionsChanged',
      params: { sessions: [expect.objectContaining({ role: 'controller' })] },
    });
    expect((await nextNotification(second)).method).toBe('event.sessionsChanged');
    expect(
      ((await request(first, 'sessions.list', null)).result as Record<string, unknown>).sessions,
    ).toHaveLength(1);
    expect(agent.clientCount()).toBe(2);
  });

  it('increments approval request ids across commands', async () => {
    const path = socketPath();
    const agent = await launch({ socketPath: path, scenario: 'controlled' });
    const client = await connectClient(path);

    agent.command('approval');
    agent.command('approval');

    const first = await nextNotification(client);
    const second = await nextNotification(client);
    expect(first.method).toBe('event.approvalRequested');
    expect(first.params.request_id).toBe('r-1');
    expect(second.method).toBe('event.approvalRequested');
    expect(second.params.request_id).toBe('r-2');
  });

  it('hands off from the controller to the viewer', async () => {
    const path = socketPath();
    const agent = await launch({ socketPath: path, scenario: 'controlled' });
    const client = await connectClient(path);

    agent.command('viewer');
    await nextNotification(client);
    agent.command('handoff');

    expect((await nextNotification(client)).method).toBe('event.handoff');
    expect((await nextNotification(client)).method).toBe('event.sessionsChanged');
    const sessions = (
      (await request(client, 'sessions.list', null)).result as {
        sessions: Array<{
          session_id: string;
          role: string;
        }>;
      }
    ).sessions;
    expect(sessions.find((session) => session.session_id === 's-2')?.role).toBe('controller');
  });

  it('rejects unknown commands', async () => {
    const path = socketPath();
    const agent = await launch({ socketPath: path });

    expect(agent.command('bogus')).toBe(false);
  });

  it('secures and removes the socket file', async () => {
    const path = socketPath();
    const agent = await launch({ socketPath: path });

    expect((await stat(path)).mode & 0o777).toBe(0o600);
    await agent.close();
    agents.splice(agents.indexOf(agent), 1);
    await expect(stat(path)).rejects.toMatchObject({ code: 'ENOENT' });
  });

  it('replaces stale files and refuses a live socket', async () => {
    const stale = staleSocketPath();
    const agent = await launch({ socketPath: stale });
    await expect(stat(stale)).resolves.toBeDefined();
    await agent.close();

    const live = socketPath();
    const first = await launch({ socketPath: live });
    await expect(startFakeAgent({ socketPath: live })).rejects.toThrow('already serving');
    await first.close();
  });

  it('answers approval.respond and closes the pending approval', async () => {
    const path = socketPath();
    const agent = await launch({ socketPath: path });
    const client = await connectClient(path);

    agent.command('connection');
    const requested = await nextNotification(client);
    expect(requested).toMatchObject({
      method: 'event.approvalRequested',
      params: { request_id: 'r-1', kind: 'connection', timeout_s: 30 },
    });

    client.send(
      `${JSON.stringify({
        jsonrpc: '2.0',
        id: 7,
        method: 'approval.respond',
        params: { request_id: 'r-1', allow: true },
      })}\n`,
    );
    const closed = await nextNotification(client);
    expect(closed).toEqual({
      jsonrpc: '2.0',
      method: 'event.approvalClosed',
      params: { request_id: 'r-1', kind: 'connection', outcome: 'allowed' },
    });
    expect(await client.next()).toMatchObject({ id: 7, result: {} });
  });

  it('answers approval.pending until the request is answered', async () => {
    const path = socketPath();
    const agent = await launch({ socketPath: path });
    const client = await connectClient(path);

    expect((await request(client, 'approval.pending', null)).result).toEqual({ requests: [] });

    agent.command('connection');
    const requested = await nextNotification(client);
    const pending = await request(client, 'approval.pending', null);
    expect(pending.result).toEqual({ requests: [requested.params] });

    client.send(
      `${JSON.stringify({
        jsonrpc: '2.0',
        id: 8,
        method: 'approval.respond',
        params: { request_id: 'r-1', allow: false },
      })}\n`,
    );
    expect((await nextNotification(client)).method).toBe('event.approvalClosed');
    expect(await client.next()).toMatchObject({ id: 8, result: {} });

    expect((await request(client, 'approval.pending', null)).result).toEqual({ requests: [] });
  });
});
