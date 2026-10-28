import type { Computer, Server } from '../../native/rscore/types';
import type { ComputerEntry } from '../native/types';

export function matchesQuery(query: string, fields: string[]): boolean {
  const needle = query.trim().toLocaleLowerCase();
  if (needle === '') {
    return true;
  }
  return fields.some(field => field.toLocaleLowerCase().includes(needle));
}

/**
 * What Home searches on a computer, the same on phones and iPad: names, nodes
 * and addresses. OS strings are not user-facing tags and would make "windows"
 * match every Windows PC.
 */
export function entrySearchFields(entry: ComputerEntry): string[] {
  switch (entry.kind) {
    case 'host':
      return [
        entry.computer.name,
        entry.computer.node,
        ...entry.computer.addresses.map(address => address.addr),
      ];
    case 'tailnet':
      return [entry.device.name, entry.device.node, ...entry.device.addrs];
    case 'manual':
      return [entry.manual.name ?? '', entry.manual.host];
  }
}

export function filterComputers(
  computers: Computer[],
  query: string,
): Computer[] {
  return computers.filter(computer =>
    matchesQuery(query, [
      computer.name,
      computer.node,
      computer.os,
      ...computer.addresses.map(address => address.addr),
    ]),
  );
}

export function filterServers(servers: Server[], query: string): Server[] {
  return servers.filter(server =>
    matchesQuery(query, [server.node, server.addr, server.os]),
  );
}
