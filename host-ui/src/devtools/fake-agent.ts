import { chmod, rm, stat } from 'node:fs/promises';
import { createServer, connect, type Server, type Socket } from 'node:net';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { createInterface } from 'node:readline';

import type { ApprovalKind, ApprovalRequest, EventName, PairingResultEvent } from '../ipc/events';
import { FakeIpc } from '../ipc/fake';
import {
  APPROVAL_CONNECTION,
  APPROVAL_PAIRING,
  buildFixture,
  HOST_ID,
  HOST_NAME,
} from '../ipc/fixtures';
import { SCENARIOS } from '../ipc/scenarios';
import type { RpcErrorShape } from '../ipc/transport';
import type { DeviceSummary, SessionSummary } from '../ipc/types';

export type FakeAgentCommand =
  | 'start'
  | 'stop'
  | 'viewer'
  | 'handoff'
  | 'approval'
  | 'connection'
  | 'pair-ok'
  | 'update'
  | 'perm-off'
  | 'perm-on';

export interface FakeAgent {
  readonly fake: FakeIpc;
  command(cmd: string): boolean;
  clientCount(): number;
  close(): Promise<void>;
}

export interface FakeAgentOptions {
  socketPath: string;
  scenario?: string;
}

interface JsonRpcRequest {
  jsonrpc?: unknown;
  id?: unknown;
  method?: unknown;
  params?: unknown;
}

interface JsonRpcErrorBody {
  code: number;
  message: string;
  data?: unknown;
}

const MAX_LINE_BYTES = 1024 * 1024;
const AGENT_COMMANDS: readonly FakeAgentCommand[] = [
  'start',
  'stop',
  'viewer',
  'handoff',
  'approval',
  'connection',
  'pair-ok',
  'update',
  'perm-off',
  'perm-on',
];

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isRpcErrorShape(value: unknown): value is RpcErrorShape {
  return isRecord(value) && typeof value.code === 'number' && typeof value.message === 'string';
}

function isFakeAgentCommand(value: string): value is FakeAgentCommand {
  return (AGENT_COMMANDS as readonly string[]).includes(value);
}

function isApprovalKind(value: unknown): value is ApprovalKind {
  return value === 'connection' || value === 'pairing' || value === 'takeover';
}

function encode(value: unknown): string {
  return `${JSON.stringify(value)}\n`;
}

function responseError(id: unknown, error: JsonRpcErrorBody): string {
  return encode({ jsonrpc: '2.0', id: id ?? null, error });
}

function responseResult(id: unknown, result: unknown): string {
  return encode({ jsonrpc: '2.0', id: id ?? null, result });
}

function rpcError(error: unknown): JsonRpcErrorBody {
  if (isRpcErrorShape(error)) {
    return {
      code: error.code,
      message: error.message,
      ...(error.data === undefined ? {} : { data: error.data }),
    };
  }
  return { code: -32603, message: 'internal error' };
}

function notification(method: EventName, params: unknown): string {
  return encode({ jsonrpc: '2.0', method, params });
}

function controllerSession(nowUs: number): SessionSummary {
  const session = buildFixture(0).sessions[0];
  if (!session) {
    throw new Error('fixture is missing its controller session');
  }
  return {
    ...session,
    role: 'controller',
    connected_at_us: nowUs,
  };
}

function viewerSession(nowUs: number): SessionSummary {
  return {
    session_id: 's-2',
    device_id: 'm4ntq8z2c7v5xa1d',
    device_name: '小明的 iPad',
    role: 'viewer',
    connected_at_us: nowUs,
  };
}

function toErrorCode(error: NodeJS.ErrnoException): string | undefined {
  return error.code;
}

async function canConnect(socketPath: string): Promise<boolean> {
  return new Promise<boolean>((resolveConnection) => {
    const socket = connect(socketPath);
    let settled = false;
    const finish = (connected: boolean): void => {
      if (settled) {
        return;
      }
      settled = true;
      socket.destroy();
      resolveConnection(connected);
    };
    socket.once('connect', () => finish(true));
    socket.once('error', () => finish(false));
  });
}

async function removeStaleSocket(socketPath: string): Promise<void> {
  try {
    await stat(socketPath);
    if (await canConnect(socketPath)) {
      throw new Error(`fake agent is already serving on ${socketPath}`);
    }
    await rm(socketPath);
  } catch (error) {
    const code = toErrorCode(error as NodeJS.ErrnoException);
    if (code === 'ENOENT') {
      return;
    }
    throw error;
  }
}

async function listen(server: Server, socketPath: string): Promise<void> {
  await new Promise<void>((resolveListen, rejectListen) => {
    const onError = (error: NodeJS.ErrnoException): void => {
      server.off('listening', onListening);
      if (error.code === 'EADDRINUSE') {
        rejectListen(new Error(`fake agent is already serving on ${socketPath}`));
      } else {
        rejectListen(error);
      }
    };
    const onListening = (): void => {
      server.off('error', onError);
      resolveListen();
    };
    server.once('error', onError);
    server.once('listening', onListening);
    server.listen(socketPath);
  });
}

export async function startFakeAgent(options: FakeAgentOptions): Promise<FakeAgent> {
  const scenario = options.scenario ?? 'idle';
  const fake = FakeIpc.fromScenario(scenario);
  const clients = new Set<Socket>();
  const pendingApprovals = new Map<string, ApprovalKind>();
  let requestId = 0;
  let closed = false;
  let listening = false;
  let closingServer: Promise<void> | null = null;
  let closePromise: Promise<void> | null = null;

  const send = (socket: Socket, line: string): void => {
    if (!socket.destroyed && socket.writable) {
      socket.write(line);
    }
  };

  const broadcast = (method: EventName, params: unknown): void => {
    const line = notification(method, params);
    for (const client of clients) {
      send(client, line);
    }
  };

  fake.subscribe((method, params) => {
    if (isRecord(params) && typeof params.request_id === 'string') {
      if (method === 'event.approvalRequested' && isApprovalKind(params.kind)) {
        pendingApprovals.set(params.request_id, params.kind);
        const sequence = /^r-(\d+)$/.exec(params.request_id)?.[1];
        if (sequence !== undefined) {
          requestId = Math.max(requestId, Number(sequence));
        }
      } else if (method === 'event.approvalClosed') {
        pendingApprovals.delete(params.request_id);
      }
    }
    if (!closed) {
      broadcast(method as EventName, params);
    }
  });

  const approval = (request: ApprovalRequest): void => {
    requestId += 1;
    fake.emit('event.approvalRequested', {
      ...request,
      request_id: `r-${requestId}`,
    });
  };

  const sessionsChanged = (): void => {
    fake.emit('event.sessionsChanged', { sessions: fake.state.sessions });
  };

  const start = (): void => {
    if (fake.state.sessions.length > 0) {
      return;
    }
    fake.state.sessions.push(controllerSession(Date.now() * 1000));
    sessionsChanged();
  };

  const stop = (): void => {
    fake.state.sessions = [];
    sessionsChanged();
  };

  const viewer = (): void => {
    if (fake.state.sessions.some((session) => session.session_id === 's-2')) {
      return;
    }
    fake.state.sessions.push(viewerSession(Date.now() * 1000));
    sessionsChanged();
  };

  const handoff = (): void => {
    const controller = fake.state.sessions.find((session) => session.role === 'controller');
    const viewerSessionEntry = fake.state.sessions.find((session) => session.role === 'viewer');
    if (!controller || !viewerSessionEntry) {
      return;
    }
    controller.role = 'viewer';
    viewerSessionEntry.role = 'controller';
    fake.emit('event.handoff', { session_id: viewerSessionEntry.session_id });
    sessionsChanged();
  };

  const pairOk = (): void => {
    const now = Date.now();
    const device: DeviceSummary = {
      device_id: 'n5fq2m8x7c3vta9b',
      name: '新的 iPhone',
      permission: 'view_only',
      paths: ['tailnet'],
      last_connected_at_us: now * 1000,
    };
    fake.state.devices.push(device);
    const result: PairingResultEvent = {
      status: 1,
      host_id: HOST_ID,
      host_name: HOST_NAME,
      attempts_left: 0,
      device_fp: '',
    };
    fake.emit('event.pairingResult', result);
  };

  const update = (): void => {
    fake.state.update = {
      state: 'waiting_session_end',
      version: '0.3.2',
      notes: '修复：TCP 模式下偶发花屏\n新增：「内网连接每次确认」',
    };
    fake.emit('event.updateState', { state: fake.state.update.state });
  };

  const permissions = (enabled: boolean): void => {
    fake.state.status.permissions = {
      screen_recording: enabled,
      accessibility: enabled,
    };
    fake.emit('event.permissionsChanged', {});
  };

  const commandHandlers: Record<FakeAgentCommand, () => void> = {
    start,
    stop,
    viewer,
    handoff,
    approval: () => approval(APPROVAL_PAIRING),
    connection: () => approval(APPROVAL_CONNECTION),
    'pair-ok': pairOk,
    update,
    'perm-off': () => permissions(false),
    'perm-on': () => permissions(true),
  };

  const command = (cmd: string): boolean => {
    if (!isFakeAgentCommand(cmd)) {
      return false;
    }
    commandHandlers[cmd]();
    return true;
  };

  const handleRequest = async (socket: Socket, line: string): Promise<void> => {
    let parsed: unknown;
    try {
      parsed = JSON.parse(line);
    } catch {
      send(socket, responseError(null, { code: -32700, message: 'parse error' }));
      return;
    }

    const request = isRecord(parsed) ? (parsed as JsonRpcRequest) : null;
    const hasId = request !== null && Object.hasOwn(request, 'id');
    if (request === null || request.jsonrpc !== '2.0' || typeof request.method !== 'string') {
      send(
        socket,
        responseError(hasId ? request.id : null, { code: -32600, message: 'invalid request' }),
      );
      return;
    }

    const id = request.id;
    try {
      const result = await fake.call(request.method, request.params ?? null);
      if (hasId) {
        send(socket, responseResult(id, result));
      }
    } catch (error) {
      if (hasId) {
        send(socket, responseError(id, rpcError(error)));
      }
    }
  };

  const server = createServer((socket) => {
    if (closed) {
      socket.destroy();
      return;
    }
    clients.add(socket);
    let buffered = Buffer.alloc(0);
    let processing = Promise.resolve();

    socket.on('data', (chunk: Buffer) => {
      buffered = Buffer.concat([buffered, chunk]);
      for (;;) {
        const newline = buffered.indexOf(0x0a);
        if (newline < 0) {
          if (buffered.length > MAX_LINE_BYTES) {
            clients.delete(socket);
            socket.destroy();
          }
          return;
        }

        const rawLine = buffered.subarray(0, newline);
        const contentEnd =
          rawLine.length > 0 && rawLine[rawLine.length - 1] === 0x0d
            ? rawLine.length - 1
            : rawLine.length;
        if (contentEnd > MAX_LINE_BYTES) {
          clients.delete(socket);
          socket.destroy();
          return;
        }
        const line = rawLine.subarray(0, contentEnd).toString('utf8');
        buffered = buffered.subarray(newline + 1);
        if (line.trim().length === 0) {
          continue;
        }
        processing = processing.then(() => handleRequest(socket, line)).catch(() => undefined);
      }
    });

    socket.on('error', () => {
      clients.delete(socket);
    });
    socket.on('close', () => {
      clients.delete(socket);
    });
  });

  const stopServer = (): Promise<void> => {
    if (!listening) {
      return Promise.resolve();
    }
    closingServer ??= new Promise<void>((resolveClose, rejectClose) => {
      server.close((error) => {
        if (error) {
          rejectClose(error);
        } else {
          resolveClose();
        }
      });
    });
    return closingServer;
  };

  try {
    await removeStaleSocket(options.socketPath);
    await listen(server, options.socketPath);
    listening = true;
    await chmod(options.socketPath, 0o600);
  } catch (error) {
    if (listening) {
      await stopServer().catch(() => undefined);
      await rm(options.socketPath, { force: true }).catch(() => undefined);
    }
    throw error;
  }

  return {
    fake,
    command,
    clientCount: () => clients.size,
    close(): Promise<void> {
      closePromise ??= (async () => {
        closed = true;
        for (const client of clients) {
          client.destroy();
        }
        clients.clear();
        for (const [approvalId, kind] of pendingApprovals) {
          fake.emit('event.approvalClosed', {
            request_id: approvalId,
            kind,
            outcome: 'cancelled',
          });
        }
        pendingApprovals.clear();
        await stopServer();
        await rm(options.socketPath, { force: true });
      })();
      return closePromise;
    },
  };
}

interface ParsedArguments {
  socketPath: string | null;
  scenario: string;
}

function parseArguments(argv: readonly string[]): ParsedArguments {
  let socketPath: string | null = null;
  let scenario = 'idle';
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === '--socket') {
      socketPath = argv[index + 1] ?? null;
      index += 1;
    } else if (argument === '--scenario') {
      scenario = argv[index + 1] ?? scenario;
      index += 1;
    }
  }
  return { socketPath: socketPath ?? process.env.RS_AGENT_SOCK ?? null, scenario };
}

function usage(): void {
  process.stderr.write(
    'usage: pnpm -s fake-agent --socket <path> [--scenario <name>]\n' +
      `scenarios: ${SCENARIOS.join(', ')}\n` +
      'commands: quit, start, stop, viewer, handoff, approval, connection, pair-ok, update, perm-off, perm-on\n',
  );
}

async function main(): Promise<void> {
  const { socketPath, scenario } = parseArguments(process.argv.slice(2));
  if (!socketPath) {
    usage();
    process.exitCode = 2;
    return;
  }

  const agent = await startFakeAgent({ socketPath, scenario });
  process.stdout.write(`fake agent listening on ${socketPath} (scenario ${scenario})\n`);

  let closing = false;
  const close = async (): Promise<void> => {
    if (closing) {
      return;
    }
    closing = true;
    rl.close();
    await agent.close();
  };

  const rl = createInterface({ input: process.stdin, crlfDelay: Infinity });
  rl.on('line', (line) => {
    void (async () => {
      const cmd = line.trim();
      if (cmd === 'quit') {
        await close();
        return;
      }
      if (cmd.length === 0) {
        return;
      }
      if (agent.command(cmd)) {
        process.stdout.write(`ok ${cmd}\n`);
      } else {
        process.stdout.write(`unknown command: ${cmd}\ncommands: ${AGENT_COMMANDS.join(', ')}\n`);
      }
    })();
  });

  process.once('SIGINT', () => {
    void close();
  });
}

const invokedPath = process.argv[1] === undefined ? null : resolve(process.argv[1]);
if (invokedPath !== null && pathToFileURL(invokedPath).href === import.meta.url) {
  await main();
}
