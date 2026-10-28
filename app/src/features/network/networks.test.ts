import { computers } from '../../native/rscore/fixtures';
import {
  netCompanyVpn,
  netNoTailnet,
  netTailnetOk,
} from '../../native/rscore/fixtures';
import type { NetworkEntry } from '../settings/schema';
import { judgeNetwork } from './judge';
import {
  EMPTY_NETWORKS,
  formToNetwork,
  networkState,
  networkToForm,
  newNetworkId,
  parseCidrList,
  parseNetworks,
  removeNetwork,
  sameNetworks,
  suggestFromHosts,
  upsertNetwork,
  validateNetworkForm,
} from './networks';

const companyNetwork: NetworkEntry = {
  id: 'n1',
  name: '公司网络',
  cidrs: ['10.30.0.0/16'],
  vpnPools: ['10.99.0.0/16'],
};

const homeNetwork: NetworkEntry = {
  id: 'home',
  name: '家里',
  cidrs: ['192.168.31.0/24'],
  vpnPools: [],
};

describe('parseNetworks', () => {
  it('drops malformed entries and invalid CIDRs', () => {
    const parsed = parseNetworks({
      v: 1,
      items: [
        {
          id: 'n1',
          name: '公司网络',
          cidrs: ['10.30.0.0/16', 'x'],
          vpnPools: ['10.99.0.0/16'],
        },
        { name: '缺 id' },
      ],
    });
    expect(parsed.items).toHaveLength(1);
    expect(parsed.items[0]).toEqual({
      id: 'n1',
      name: '公司网络',
      cidrs: ['10.30.0.0/16'],
      vpnPools: ['10.99.0.0/16'],
    });
  });

  it.each([null, undefined, {}, { v: 2, items: [] }, { v: 1, items: 'nope' }])(
    'returns empty for %p',
    raw => {
      expect(parseNetworks(raw)).toEqual(EMPTY_NETWORKS);
    },
  );

  it('drops entries without a name and replaces malformed lists', () => {
    expect(
      parseNetworks({
        v: 1,
        items: [
          { id: 'n1', cidrs: [] },
          { id: 'n2', name: 'Office', cidrs: 'x', vpnPools: ['bad'] },
        ],
      }).items,
    ).toEqual([{ id: 'n2', name: 'Office', cidrs: [], vpnPools: [] }]);
  });
});

describe('mutations', () => {
  it('adds and replaces an entry by id', () => {
    const added = upsertNetwork(EMPTY_NETWORKS, companyNetwork);
    expect(added.items).toEqual([companyNetwork]);

    const renamed = { ...companyNetwork, name: '新公司' };
    expect(upsertNetwork(added, renamed).items).toEqual([renamed]);
  });

  it('removes an entry by id', () => {
    const value = { v: 1 as const, items: [companyNetwork, homeNetwork] };
    expect(removeNetwork(value, 'n1').items).toEqual([homeNetwork]);
    expect(removeNetwork(value, 'missing').items).toHaveLength(2);
  });
});

describe('newNetworkId', () => {
  it('uses a slug for Latin names', () => {
    expect(newNetworkId('Office', [])).toBe('net-office');
    expect(newNetworkId('Home Office!', [])).toBe('net-home-office');
  });

  it('numbers a duplicate slug', () => {
    expect(newNetworkId('Office', ['net-office'])).toBe('net-office-2');
    expect(newNetworkId('Office', ['net-office', 'net-office-2'])).toBe(
      'net-office-3',
    );
  });

  it('uses net-1, net-2 for names without Latin characters', () => {
    expect(newNetworkId('公司网络', [])).toBe('net-1');
    expect(newNetworkId('公司网络', ['net-1'])).toBe('net-2');
  });
});

describe('parseCidrList', () => {
  it('splits on commas, whitespace, and newlines while deduplicating', () => {
    expect(
      parseCidrList('10.30.0.0/16, 10.31.0.0/16\n10.30.0.0/16 bad'),
    ).toEqual({
      ok: ['10.30.0.0/16', '10.31.0.0/16'],
      bad: ['bad'],
    });
  });
});

describe('validateNetworkForm', () => {
  it('requires a name and at least one CIDR, and validates VPN pools', () => {
    expect(
      validateNetworkForm(
        { name: '', cidrs: '', vpnPools: 'x' },
        { existingNames: [] },
      ),
    ).toEqual({
      name: 'required',
      cidrs: 'required',
      vpnPools: 'badCidr',
    });
  });

  it('rejects duplicate names case-insensitively', () => {
    expect(
      validateNetworkForm(
        { name: ' office ', cidrs: '10.30.0.0/16', vpnPools: '' },
        { existingNames: ['Office'] },
      ),
    ).toEqual({ name: 'duplicateName' });
  });

  it('does not count the edited entry as a duplicate', () => {
    expect(
      validateNetworkForm(
        { name: 'Office', cidrs: '10.30.0.0/16', vpnPools: '' },
        { existingNames: ['Office'], editingName: 'office' },
      ),
    ).toEqual({});
  });

  it('rejects a mixture of valid and invalid CIDRs', () => {
    expect(
      validateNetworkForm(
        { name: 'Office', cidrs: '10.30.0.0/16 bad', vpnPools: '' },
        { existingNames: [] },
      ),
    ).toEqual({ cidrs: 'badCidr' });
  });
});

describe('form conversion', () => {
  it('round-trips with comma-space separators', () => {
    const form = {
      name: ' 公司网络 ',
      cidrs: '10.30.0.0/16, 10.31.0.0/16',
      vpnPools: '10.99.0.0/16',
    };
    const entry = formToNetwork(form, 'n1');
    expect(entry).toEqual({
      id: 'n1',
      name: '公司网络',
      cidrs: ['10.30.0.0/16', '10.31.0.0/16'],
      vpnPools: ['10.99.0.0/16'],
    });
    expect(networkToForm(entry)).toEqual({
      name: '公司网络',
      cidrs: '10.30.0.0/16, 10.31.0.0/16',
      vpnPools: '10.99.0.0/16',
    });
  });
});

describe('suggestFromHosts', () => {
  it('groups intranet and LAN CIDRs by host name and sorts them', () => {
    expect(suggestFromHosts(computers, [])).toEqual([
      { cidr: '10.30.0.0/16', hostNames: ['工位 Windows'] },
      { cidr: '192.168.31.0/24', hostNames: ['Studio Mac mini'] },
    ]);
  });

  it('drops a CIDR already covered by a registered network', () => {
    expect(suggestFromHosts(computers, [companyNetwork])).toEqual([
      { cidr: '192.168.31.0/24', hostNames: ['Studio Mac mini'] },
    ]);
  });

  it('drops a CIDR whose network address is inside a registered CIDR', () => {
    expect(
      suggestFromHosts(computers, [
        {
          id: 'wide',
          name: 'Wide office',
          cidrs: ['10.0.0.0/8'],
          vpnPools: [],
        },
      ]),
    ).toEqual([{ cidr: '192.168.31.0/24', hostNames: ['Studio Mac mini'] }]);
  });

  it('deduplicates host names and ignores non-LAN address kinds', () => {
    const first = {
      ...computers[0],
      addresses: [
        {
          kind: 'lan' as const,
          addr: '192.168.31.20',
          port: 7400,
          cidr: '192.168.31.0/24',
        },
      ],
    };
    const second = { ...first, hostId: 'second' };
    expect(suggestFromHosts([first, second], [])).toEqual([
      { cidr: '192.168.31.0/24', hostNames: ['Studio Mac mini'] },
    ]);
    expect(suggestFromHosts([computers[3]], [])).toEqual([]);
  });
});

describe('networkState', () => {
  it('maps a company VPN verdict to vpn', () => {
    expect(
      networkState(
        companyNetwork,
        judgeNetwork(netCompanyVpn, [companyNetwork]),
      ),
    ).toBe('vpn');
  });

  it('maps an at-company verdict to inside', () => {
    expect(
      networkState(homeNetwork, judgeNetwork(netTailnetOk, [homeNetwork])),
    ).toBe('inside');
  });

  it('returns none for an unrelated verdict', () => {
    expect(
      networkState(companyNetwork, judgeNetwork(netNoTailnet, [homeNetwork])),
    ).toBe('none');
  });
});

describe('sameNetworks', () => {
  it('compares entries structurally', () => {
    expect(sameNetworks([companyNetwork], [{ ...companyNetwork }])).toBe(true);
    expect(sameNetworks([companyNetwork], [homeNetwork])).toBe(false);
  });
});
