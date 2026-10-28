import type { SshAuth } from '../../native/rscore/types';

export type ManualServer = {
  id: string;
  kind: 'ssh';
  name: string;
  host: string;
  port: number;
  user: string;
  auth: SshAuth;
  jump: string | null;
  networkId: string | null;
  addedAtUs: number;
  updatedAtUs: number;
};

export type ManualComputer = {
  id: string;
  kind: 'computer';
  host: string;
  port: number;
  hostId: string | null;
  name: string | null;
  networkId: string | null;
  addedAtUs: number;
  /** V10：免安装电脑。hostId 为 null 时必填才会进首页。 */
  os?: 'macos' | 'windows';
  protocol?: 'vnc' | 'rdp';
};

export type ManualEntry = ManualServer | ManualComputer;

export type ServersValue = {
  v: 1;
  entries: ManualEntry[];
  tailscaleSsh: Record<string, boolean>;
};

export const EMPTY_SERVERS: ServersValue = {
  v: 1,
  entries: [],
  tailscaleSsh: {},
};

type JsonObject = Record<string, unknown>;

function isObject(value: unknown): value is JsonObject {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function isString(value: unknown): value is string {
  return typeof value === 'string';
}

function isNumber(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value);
}

function isNullableString(value: unknown): value is string | null {
  return value === null || isString(value);
}

function isAuth(value: unknown): value is SshAuth {
  return value === 'tailscale' || value === 'deviceKey' || value === 'password';
}

function parseServer(value: JsonObject): ManualServer | null {
  if (
    !isString(value.id) ||
    value.kind !== 'ssh' ||
    !isString(value.name) ||
    !isString(value.host) ||
    !isNumber(value.port) ||
    !isString(value.user) ||
    !isAuth(value.auth) ||
    !isNullableString(value.jump) ||
    !isNullableString(value.networkId) ||
    !isNumber(value.addedAtUs) ||
    !isNumber(value.updatedAtUs)
  ) {
    return null;
  }

  return {
    id: value.id,
    kind: 'ssh',
    name: value.name,
    host: value.host,
    port: value.port,
    user: value.user,
    auth: value.auth,
    jump: value.jump,
    networkId: value.networkId,
    addedAtUs: value.addedAtUs,
    updatedAtUs: value.updatedAtUs,
  };
}

function parseComputer(value: JsonObject): ManualComputer | null {
  if (
    !isString(value.id) ||
    value.kind !== 'computer' ||
    !isString(value.host) ||
    !isNumber(value.port) ||
    !isNullableString(value.hostId) ||
    !isNullableString(value.name) ||
    !isNullableString(value.networkId) ||
    !isNumber(value.addedAtUs)
  ) {
    return null;
  }

  return {
    id: value.id,
    kind: 'computer',
    host: value.host,
    port: value.port,
    hostId: value.hostId,
    name: value.name,
    networkId: value.networkId,
    addedAtUs: value.addedAtUs,
    ...(value.os === 'macos' || value.os === 'windows' ? { os: value.os } : {}),
    ...(value.protocol === 'vnc' || value.protocol === 'rdp'
      ? { protocol: value.protocol }
      : {}),
  };
}

function parseEntry(value: unknown): ManualEntry | null {
  if (!isObject(value)) {
    return null;
  }
  if (value.kind === 'ssh') {
    return parseServer(value);
  }
  if (value.kind === 'computer') {
    return parseComputer(value);
  }
  return null;
}

function parseTailscaleSsh(value: unknown): Record<string, boolean> {
  if (!isObject(value)) {
    return {};
  }

  const result: Record<string, boolean> = {};
  for (const [node, enabled] of Object.entries(value)) {
    if (typeof enabled === 'boolean') {
      result[node] = enabled;
    }
  }
  return result;
}

export function parseServersValue(raw: unknown): ServersValue {
  if (!isObject(raw) || raw.v !== 1 || !Array.isArray(raw.entries)) {
    return EMPTY_SERVERS;
  }

  const entries: ManualEntry[] = [];
  for (const item of raw.entries) {
    const entry = parseEntry(item);
    if (entry !== null) {
      entries.push(entry);
    }
  }

  return {
    v: 1,
    entries,
    tailscaleSsh: parseTailscaleSsh(raw.tailscaleSsh),
  };
}

export function upsertEntry(v: ServersValue, e: ManualEntry): ServersValue {
  const index = v.entries.findIndex(entry => entry.id === e.id);
  const entries =
    index === -1
      ? [...v.entries, e]
      : v.entries.map((entry, i) => (i === index ? e : entry));
  return { ...v, entries };
}

export function removeEntry(v: ServersValue, id: string): ServersValue {
  const entries = v.entries.filter(entry => entry.id !== id);
  return entries.length === v.entries.length ? v : { ...v, entries };
}

function slugify(value: string, fallback: string): string {
  const slug = value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/-+/g, '-')
    .replace(/^-|-$/g, '');
  return slug === '' ? fallback : slug;
}

function availableId(base: string, taken: string[]): string {
  const used = new Set(taken);
  if (!used.has(base)) {
    return base;
  }

  let suffix = 2;
  while (used.has(`${base}-${suffix}`)) {
    suffix += 1;
  }
  return `${base}-${suffix}`;
}

export function newServerId(name: string, taken: string[]): string {
  return availableId(`manual-${slugify(name, 'server')}`, taken);
}

export function newComputerId(
  host: string,
  port: number,
  taken: string[],
): string {
  return availableId(`addr-${slugify(host, 'host')}-${port}`, taken);
}

export function manualServers(v: ServersValue): ManualServer[] {
  return v.entries.filter(
    (entry): entry is ManualServer => entry.kind === 'ssh',
  );
}

export function manualComputers(v: ServersValue): ManualComputer[] {
  return v.entries.filter(
    (entry): entry is ManualComputer => entry.kind === 'computer',
  );
}
