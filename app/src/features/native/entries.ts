import type { Directory } from '../../native/rscore/types';
import type { ManualComputer } from '../servers/manual';
import type { ComputerEntry } from './types';

type NativeOs = 'macos' | 'windows';
type NativeProtocol = 'vnc' | 'rdp';

function isNativeOs(value: unknown): value is NativeOs {
  return value === 'macos' || value === 'windows';
}

function compareNames(left: string, right: string): number {
  return left.localeCompare(right, undefined, { sensitivity: 'base' });
}

/** Merge the directory and saved, host-less computers for the Home list. */
export function buildEntries(
  dir: Directory,
  manual: ManualComputer[],
): ComputerEntry[] {
  const computers = dir.computers ?? [];
  const others = dir.others ?? [];
  const hostNodes = new Set(computers.map(computer => computer.node));
  const directoryAddresses = new Set(
    computers.flatMap(computer =>
      computer.addresses.map(address => address.addr),
    ),
  );
  for (const device of others) {
    for (const address of device.addrs) {
      directoryAddresses.add(address);
    }
  }

  const hosts: ComputerEntry[] = computers.map(computer => ({
    kind: 'host',
    key: `host:${computer.hostId}`,
    computer,
  }));

  const tailnet: ComputerEntry[] = others
    .filter(device => !hostNodes.has(device.node))
    .map(device => ({
      kind: 'tailnet' as const,
      key: `tailnet:${device.node}`,
      device,
    }))
    .sort((left, right) => {
      if (left.device.online !== right.device.online) {
        return left.device.online ? -1 : 1;
      }
      return compareNames(left.device.name, right.device.name);
    });

  const manuals: ComputerEntry[] = manual
    .filter(
      (computer): computer is ManualComputer & { os: NativeOs } =>
        computer.hostId === null &&
        isNativeOs(computer.os) &&
        !directoryAddresses.has(computer.host),
    )
    .map(computer => ({
      kind: 'manual' as const,
      key: `manual:${computer.id}`,
      manual: computer,
    }))
    .sort((left, right) => compareNames(entryName(left), entryName(right)));

  return [...hosts, ...tailnet, ...manuals];
}

export function entryName(entry: ComputerEntry): string {
  switch (entry.kind) {
    case 'host':
      return entry.computer.name;
    case 'tailnet':
      return entry.device.name;
    case 'manual':
      return entry.manual.name ?? entry.manual.host;
  }
}

export function entryOs(entry: ComputerEntry): NativeOs {
  switch (entry.kind) {
    case 'host':
      return entry.computer.os;
    case 'tailnet':
      return entry.device.os;
    case 'manual':
      return entry.manual.os;
  }
}

export function entryProtocol(entry: ComputerEntry): NativeProtocol {
  return entryOs(entry) === 'macos' ? 'vnc' : 'rdp';
}
