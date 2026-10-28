import type { FakeCore } from '../native/rscore/fakeCore';
import {
  FIXTURE_NOW,
  computers as fixtureComputers,
  tailnetRunning,
} from '../native/rscore/fixtures';
import type {
  Computer,
  PairedHost,
  SyncValue,
  TailnetPeer,
} from '../native/rscore/types';
import { LOCAL_REACH_KEY } from '../features/data/localReach';
import { LOCAL_SYNC_PREFIX } from '../features/data/localSync';
import type { SyncNs } from '../features/data/localSync';
import type { ManualEntry, ServersValue } from '../features/servers/manual';
import type { KvStore } from '../features/storage/kv';

const DAY_US = 24 * 60 * 60 * 1_000_000;

/** What an older version with a Hub left in MMKV (decision 9). */
export const LEFTOVER_HUB_URL = 'http://rs-hub:7300';

/** Writes the raw MMKV entry, so it still applies once the setting is gone. */
export function leaveHubUrl(kv: KvStore): KvStore {
  kv.setJSON('settings.hub.url', LEFTOVER_HUB_URL);
  return kv;
}

/** Saves a synced namespace on this phone, as an earlier write would have. */
export function seedLocalSync(kv: KvStore, ns: SyncNs, saved: SyncValue): void {
  kv.setJSON(`${LOCAL_SYNC_PREFIX}${ns}`, saved);
}

/**
 * The next read of `ns` returns `stale`: another writer on this phone landed
 * after it, so the write that follows meets a conflict. Restored by
 * jest.restoreAllMocks().
 */
export function readStaleOnce(kv: KvStore, ns: SyncNs, stale: SyncValue): void {
  const getJSON = kv.getJSON.bind(kv);
  let pending = true;
  jest.spyOn(kv, 'getJSON').mockImplementation((key: string) => {
    if (pending && key === `${LOCAL_SYNC_PREFIX}${ns}`) {
      pending = false;
      return stale;
    }
    return getJSON(key);
  });
}

/**
 * Makes every later write of `ns` to this phone's storage throw `error`; other
 * keys are still written. Restored by jest.restoreAllMocks().
 */
export function failLocalSync(
  kv: KvStore,
  ns: SyncNs,
  error: Error = new Error('write failed'),
): void {
  const setJSON = kv.setJSON.bind(kv);
  jest.spyOn(kv, 'setJSON').mockImplementation((key, value) => {
    if (key === `${LOCAL_SYNC_PREFIX}${ns}`) {
      throw error;
    }
    setJSON(key, value);
  });
}

/** The record Rust keeps for a paired Host, with what pairing learned. */
export function pairedHostOf(computer: Computer): PairedHost {
  const seenAt = computer.lastSeenAt ?? FIXTURE_NOW;
  return {
    hostId: computer.hostId,
    fp: computer.fp,
    name: computer.name,
    addresses: computer.addresses.map(address =>
      address.addr.includes(':')
        ? `[${address.addr}]:${address.port}`
        : `${address.addr}:${address.port}`,
    ),
    pairedAt: Math.min(seenAt, FIXTURE_NOW) - 30 * DAY_US,
    os: computer.os,
    osVersion: computer.osVersion ?? null,
    hostVersion: computer.hostVersion,
    features: computer.features,
    displays: computer.displays.map(display => ({ ...display })),
    lastSeenAt: computer.lastSeenAt,
  };
}

/** A tailnet node as the in-app tailnet lists it. */
export function tailnetPeer(overrides: Partial<TailnetPeer>): TailnetPeer {
  const hostname = overrides.hostname ?? 'node';
  return {
    hostname,
    dnsName: `${hostname}.ts.example.com.`,
    ips: [],
    os: 'macOS',
    online: true,
    tags: [],
    direct: true,
    curAddr: '',
    relay: '',
    peerRelay: '',
    lastHandshakeUs: 0,
    rxBytes: 0,
    txBytes: 0,
    ...overrides,
  };
}

export type LocalDirectorySeed = {
  /**
   * Shown as paired Hosts, online as the last reach probe found them. Only
   * `paired` computers are kept. Defaults to the fixture computers.
   */
  computers?: Computer[];
  /** Manual computers and servers saved on this phone. */
  manual?: ManualEntry[];
  /**
   * Starts the in-app tailnet with these peers, beside the fixture peers of
   * the seeded Hosts, which give them their node names. Mac and Windows peers
   * show as tailnet computers under their node name; `tag:server` peers are
   * SSH servers.
   */
  peers?: TailnetPeer[];
};

/**
 * Seeds everything the local directory reads: paired Hosts in the core, their
 * last reach in MMKV, manual entries and the in-app tailnet.
 */
export function seedLocalDirectory(
  core: FakeCore,
  kv: KvStore,
  seed: LocalDirectorySeed = {},
): void {
  const paired = (seed.computers ?? fixtureComputers).filter(
    computer => computer.paired,
  );
  core.setFixtures({ pairedHosts: paired.map(pairedHostOf) });
  kv.setJSON(
    LOCAL_REACH_KEY,
    Object.fromEntries(
      paired.map(computer => [
        computer.hostId,
        {
          online: computer.online && computer.hostRunning,
          at: computer.lastSeenAt ?? FIXTURE_NOW,
        },
      ]),
    ),
  );
  if (seed.manual !== undefined) {
    const value: ServersValue = { v: 1, entries: seed.manual, tailscaleSsh: {} };
    seedLocalSync(kv, 'servers', { version: 1, value });
  }
  if (seed.peers !== undefined) {
    const pairedIps = new Set(
      paired.flatMap(computer => computer.addresses.map(a => a.addr)),
    );
    const hostPeers = tailnetRunning.peers.filter(peer =>
      peer.ips.some(ip => pairedIps.has(ip)),
    );
    core.setTailnet({
      available: true,
      status: { ...tailnetRunning, peers: [...hostPeers, ...seed.peers] },
    });
  }
}
