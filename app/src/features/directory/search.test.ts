import { computers, servers } from '../../native/rscore/fixtures';
import type { ComputerEntry } from '../native/types';
import {
  entrySearchFields,
  filterComputers,
  filterServers,
  matchesQuery,
} from './search';

describe('matchesQuery', () => {
  it.each([
    ['', ['a', 'b'], true],
    ['   ', ['a'], true],
    ['  PROD  ', ['prod-api-01'], true],
    ['api', ['prod-api-01', 'Ubuntu'], true],
    ['ubuntu', ['prod-api-01', 'Ubuntu 24.04'], true],
    ['not-there', ['prod-api-01'], false],
  ])('matches %p in %p: %s', (query, fields, expected) => {
    expect(matchesQuery(query, fields)).toBe(expected);
  });
});

describe('filterComputers', () => {
  it('matches a computer name', () => {
    expect(filterComputers(computers, 'studio').map(item => item.name)).toEqual(
      ['Studio Mac mini'],
    );
  });

  it('matches an address prefix', () => {
    expect(filterComputers(computers, '10.30').map(item => item.name)).toEqual([
      '工位 Windows',
    ]);
  });

  it('matches node and OS fields case-insensitively', () => {
    expect(filterComputers(computers, 'XL-MBP')).toHaveLength(1);
    expect(filterComputers(computers, 'windows')).toHaveLength(2);
  });

  it('returns all computers for an empty query', () => {
    expect(filterComputers(computers, '')).toHaveLength(computers.length);
  });

  it('returns none for an unknown query', () => {
    expect(filterComputers(computers, '不存在')).toEqual([]);
  });
});

describe('filterServers', () => {
  it('matches node, address and OS', () => {
    expect(filterServers(servers, ' PROD ').map(item => item.node)).toEqual([
      'prod-api-01',
    ]);
    expect(filterServers(servers, '100.64.0.32')).toHaveLength(1);
    expect(filterServers(servers, 'debian')).toHaveLength(1);
  });
});

describe('entrySearchFields', () => {
  it('searches a Host computer by name, node and addresses, not its OS', () => {
    const computer = computers[0];
    const fields = entrySearchFields({
      kind: 'host',
      key: `host:${computer.hostId}`,
      computer,
    });

    expect(fields).toEqual([
      computer.name,
      computer.node,
      ...computer.addresses.map(address => address.addr),
    ]);
    expect(fields).not.toContain(computer.os);
  });

  it('searches tailnet and manual computers by name and address', () => {
    const tailnet = {
      kind: 'tailnet',
      key: 'tailnet:office-mac',
      device: {
        name: 'office-mac',
        node: 'office-mac',
        addrs: ['100.64.0.40'],
      },
    } as unknown as ComputerEntry;
    expect(entrySearchFields(tailnet)).toEqual([
      'office-mac',
      'office-mac',
      '100.64.0.40',
    ]);

    const manual = {
      kind: 'manual',
      key: 'manual:lab-pc',
      manual: { name: null, host: '192.168.31.50' },
    } as unknown as ComputerEntry;
    expect(entrySearchFields(manual)).toEqual(['', '192.168.31.50']);
    expect(matchesQuery('192.168', entrySearchFields(manual))).toBe(true);
  });
});
