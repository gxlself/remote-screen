import type { Computer } from '../../native/rscore/types';
import type { NetworkEntry } from '../settings/schema';
import { ipInCidr, isValidCidr, parseIpv4 } from './cidr';
import type { NetVerdict } from './judge';

export type NetworksValue = { v: 1; items: NetworkEntry[] };

export const EMPTY_NETWORKS: NetworksValue = { v: 1, items: [] };

type JsonObject = Record<string, unknown>;

function isPlainObject(value: unknown): value is JsonObject {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    return false;
  }
  const prototype = Object.getPrototypeOf(value);
  return prototype === Object.prototype || prototype === null;
}

function validCidrs(value: unknown): string[] {
  if (!Array.isArray(value)) {
    return [];
  }
  return value.filter(
    (item): item is string => typeof item === 'string' && isValidCidr(item),
  );
}

function parseNetworkEntry(value: unknown): NetworkEntry | null {
  if (!isPlainObject(value)) {
    return null;
  }
  const { id, name } = value;
  if (
    typeof id !== 'string' ||
    id.trim() === '' ||
    typeof name !== 'string' ||
    name.trim() === ''
  ) {
    return null;
  }
  return {
    id,
    name,
    cidrs: validCidrs(value.cidrs),
    vpnPools: validCidrs(value.vpnPools),
  };
}

/** Parse a synced networks value, dropping malformed entries and CIDRs. */
export function parseNetworks(raw: unknown): NetworksValue {
  if (!isPlainObject(raw) || raw.v !== 1 || !Array.isArray(raw.items)) {
    return EMPTY_NETWORKS;
  }
  const items: NetworkEntry[] = [];
  for (const item of raw.items) {
    const entry = parseNetworkEntry(item);
    if (entry !== null) {
      items.push(entry);
    }
  }
  return { v: 1, items };
}

export function upsertNetwork(
  value: NetworksValue,
  entry: NetworkEntry,
): NetworksValue {
  const index = value.items.findIndex(item => item.id === entry.id);
  if (index === -1) {
    return { v: 1, items: [...value.items, entry] };
  }
  return {
    v: 1,
    items: value.items.map((item, itemIndex) =>
      itemIndex === index ? entry : item,
    ),
  };
}

export function removeNetwork(value: NetworksValue, id: string): NetworksValue {
  return { v: 1, items: value.items.filter(item => item.id !== id) };
}

function slugify(name: string): string {
  return name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '');
}

export function newNetworkId(name: string, taken: string[]): string {
  const used = new Set(taken);
  const slug = slugify(name);
  if (slug === '') {
    let suffix = 1;
    while (used.has(`net-${suffix}`)) {
      suffix += 1;
    }
    return `net-${suffix}`;
  }

  const base = `net-${slug}`;
  if (!used.has(base)) {
    return base;
  }
  let suffix = 2;
  while (used.has(`${base}-${suffix}`)) {
    suffix += 1;
  }
  return `${base}-${suffix}`;
}

export type NetworkForm = { name: string; cidrs: string; vpnPools: string };

/** Split comma/space/newline-separated CIDRs, deduplicating while preserving order. */
export function parseCidrList(text: string): { ok: string[]; bad: string[] } {
  const ok: string[] = [];
  const bad: string[] = [];
  const seen = new Set<string>();
  for (const token of text.split(/[,\s]+/)) {
    if (token === '' || seen.has(token)) {
      continue;
    }
    seen.add(token);
    (isValidCidr(token) ? ok : bad).push(token);
  }
  return { ok, bad };
}

type NetworkFormError = 'required' | 'badCidr' | 'duplicateName';

export function validateNetworkForm(
  form: NetworkForm,
  ctx: { existingNames: string[]; editingName?: string },
): Partial<Record<'name' | 'cidrs' | 'vpnPools', NetworkFormError>> {
  const errors: Partial<
    Record<'name' | 'cidrs' | 'vpnPools', NetworkFormError>
  > = {};
  const name = form.name.trim();
  if (name === '') {
    errors.name = 'required';
  } else {
    const normalized = name.toLowerCase();
    const editingName = ctx.editingName?.trim().toLowerCase();
    const duplicate = ctx.existingNames.some(existing => {
      const candidate = existing.trim().toLowerCase();
      return candidate === normalized && candidate !== editingName;
    });
    if (duplicate) {
      errors.name = 'duplicateName';
    }
  }

  const cidrs = parseCidrList(form.cidrs);
  if (cidrs.bad.length > 0) {
    errors.cidrs = 'badCidr';
  } else if (cidrs.ok.length === 0) {
    errors.cidrs = 'required';
  }

  if (parseCidrList(form.vpnPools).bad.length > 0) {
    errors.vpnPools = 'badCidr';
  }
  return errors;
}

export function formToNetwork(form: NetworkForm, id: string): NetworkEntry {
  return {
    id,
    name: form.name.trim(),
    cidrs: parseCidrList(form.cidrs).ok,
    vpnPools: parseCidrList(form.vpnPools).ok,
  };
}

export function networkToForm(entry: NetworkEntry): NetworkForm {
  return {
    name: entry.name,
    cidrs: entry.cidrs.join(', '),
    vpnPools: entry.vpnPools.join(', '),
  };
}

export type CidrSuggestion = { cidr: string; hostNames: string[] };

function networkAddress(cidr: string): string | null {
  const slash = cidr.indexOf('/');
  if (slash === -1) {
    return null;
  }
  const address = parseIpv4(cidr.slice(0, slash));
  const prefix = Number(cidr.slice(slash + 1));
  if (address === null || !Number.isInteger(prefix)) {
    return null;
  }
  const blockSize = 2 ** (32 - prefix);
  const network = address - (address % blockSize);
  return [24, 16, 8, 0]
    .map(shift => Math.floor(network / 2 ** shift) % 256)
    .join('.');
}

function isCovered(cidr: string, existingCidrs: string[]): boolean {
  if (existingCidrs.includes(cidr)) {
    return true;
  }
  const address = networkAddress(cidr);
  if (address === null) {
    return false;
  }
  return existingCidrs.some(
    registered => isValidCidr(registered) && ipInCidr(address, registered),
  );
}

export function suggestFromHosts(
  computers: Computer[],
  existing: NetworkEntry[],
): CidrSuggestion[] {
  const byCidr = new Map<string, string[]>();
  for (const computer of computers) {
    for (const address of computer.addresses) {
      if (
        (address.kind !== 'intranet' && address.kind !== 'lan') ||
        address.cidr === undefined ||
        !isValidCidr(address.cidr)
      ) {
        continue;
      }
      const hostNames = byCidr.get(address.cidr) ?? [];
      if (!hostNames.includes(computer.name)) {
        hostNames.push(computer.name);
      }
      byCidr.set(address.cidr, hostNames);
    }
  }

  const registered = existing.flatMap(entry => entry.cidrs);
  return [...byCidr.entries()]
    .filter(([cidr]) => !isCovered(cidr, registered))
    .map(([cidr, hostNames]) => ({ cidr, hostNames }))
    .sort((a, b) => (a.cidr < b.cidr ? -1 : a.cidr > b.cidr ? 1 : 0));
}

export function networkState(
  entry: NetworkEntry,
  verdict: NetVerdict,
): 'vpn' | 'inside' | 'none' {
  if (verdict.companyVpn?.network.id === entry.id) {
    return 'vpn';
  }
  if (verdict.atCompany?.id === entry.id) {
    return 'inside';
  }
  return 'none';
}

export function sameNetworks(a: NetworkEntry[], b: NetworkEntry[]): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}
