import { computers } from '../../native/rscore/fixtures';
import type { CoreEventOf } from '../../native/rscore/types';
import {
  IDLE_PROBE,
  RS_DEFAULT_PORT,
  addrVerdict,
  applyDiscover,
  applyProbe,
  helloOf,
  matchComputer,
  nativeOffers,
  parseAddressForm,
  sharingOff,
  startProbe,
  timeoutProbe,
} from './addressProbe';
import type { AddrProbe, ProbeHello } from './addressProbe';

const OFFICE_HOST_ID = 'w3k8p2x7m4q9tb6c';
const OFFICE_FP = 'tNTNZkRY8uF2nmpKHGT4leM_1nS8w5M-dd37_l0BbIg';

function result(
  probeId: string,
  outcome: CoreEventOf<'probe.result'>['outcome'] = 'ok',
  port = 7400,
  extra: Partial<CoreEventOf<'probe.result'>> = {},
): CoreEventOf<'probe.result'> {
  return {
    type: 'probe.result',
    probeId,
    hostId: null,
    path: 'manual',
    addr: '10.30.12.48',
    port,
    transport: 'tcp',
    outcome,
    ms: 23,
    last: true,
    ...extra,
  };
}

function withHello(
  event: CoreEventOf<'probe.result'>,
  hello: Partial<ProbeHello> | unknown,
): CoreEventOf<'probe.result'> {
  return { ...event, hello } as CoreEventOf<'probe.result'>;
}

describe('parseAddressForm', () => {
  it('defaults an empty port to 7400', () => {
    expect(parseAddressForm('10.30.12.48', '')).toEqual({
      host: '10.30.12.48',
      port: RS_DEFAULT_PORT,
    });
  });

  it('trims the host and accepts an explicit port', () => {
    expect(parseAddressForm('  office-win  ', ' 3389 ')).toEqual({
      host: 'office-win',
      port: 3389,
    });
  });

  it.each(['0', '65536', '99999', 'abc', '22.5'])(
    'rejects invalid port %p',
    port => {
      expect(parseAddressForm('10.30.12.48', port)).toEqual({
        error: 'badPort',
      });
    },
  );

  it('rejects a malformed host', () => {
    expect(parseAddressForm('a b', '')).toEqual({ error: 'badHost' });
  });

  it('takes the port from host:port unless the port field is filled', () => {
    expect(parseAddressForm('office-mac:5900', '')).toEqual({
      host: 'office-mac',
      port: 5900,
    });
    expect(parseAddressForm('office-mac:5900', '7401')).toEqual({
      host: 'office-mac',
      port: 7401,
    });
    expect(parseAddressForm('office-mac:0', '')).toEqual({ error: 'badHost' });
  });

  it('accepts IPv6 bare or in brackets', () => {
    expect(parseAddressForm('fd7a:115c:a1e0::3', '')).toEqual({
      host: 'fd7a:115c:a1e0::3',
      port: RS_DEFAULT_PORT,
    });
    expect(parseAddressForm('[fd7a:115c:a1e0::3]:3389', '')).toEqual({
      host: 'fd7a:115c:a1e0::3',
      port: 3389,
    });
    expect(parseAddressForm('[fd7a:115c:a1e0::3]', '')).toEqual({
      host: 'fd7a:115c:a1e0::3',
      port: RS_DEFAULT_PORT,
    });
  });
});

describe('helloOf', () => {
  it('returns null for a normal probe event', () => {
    expect(helloOf(result('p1'))).toBeNull();
  });

  it('reads a complete hello extension', () => {
    const hello: ProbeHello = {
      hostId: OFFICE_HOST_ID,
      name: '工位 Windows',
      os: 'windows',
      hostVersion: '0.3.0',
      fp: OFFICE_FP,
    };
    expect(helloOf(withHello(result('p1'), hello))).toEqual(hello);
  });

  it('treats a probe.result that only carries hostId as a Remote Host', () => {
    expect(
      helloOf(result('p1', 'ok', 7400, { hostId: OFFICE_HOST_ID })),
    ).toEqual({ hostId: OFFICE_HOST_ID });
    expect(
      helloOf(
        withHello(result('p1', 'ok', 7400, { hostId: OFFICE_HOST_ID }), {
          name: 'partial',
        }),
      ),
    ).toEqual({ hostId: OFFICE_HOST_ID });
  });

  it('returns null when any hello field is missing or mistyped', () => {
    expect(
      helloOf(
        withHello(result('p1'), {
          hostId: OFFICE_HOST_ID,
          name: '工位 Windows',
          os: 'windows',
          hostVersion: '0.3.0',
        }),
      ),
    ).toBeNull();
    expect(
      helloOf(
        withHello(result('p1'), {
          hostId: OFFICE_HOST_ID,
          name: '工位 Windows',
          os: 'linux',
          hostVersion: '0.3.0',
          fp: OFFICE_FP,
        }),
      ),
    ).toBeNull();
  });
});

describe('matchComputer', () => {
  it('matches a registered address without hello', () => {
    const match = matchComputer('10.30.12.48', computers, null);
    expect(match?.computer.name).toBe('工位 Windows');
    expect(match?.by).toBe('address');
  });

  it('prefers hostId and calls it fp when fingerprints agree', () => {
    const match = matchComputer('10.30.12.48', computers, {
      hostId: OFFICE_HOST_ID,
      name: '工位 Windows',
      os: 'windows',
      hostVersion: '0.3.0',
      fp: OFFICE_FP,
    });
    expect(match?.computer.name).toBe('工位 Windows');
    expect(match?.by).toBe('fp');
  });

  it('reports a hostId match when the fingerprint changed', () => {
    const match = matchComputer('10.30.12.48', computers, {
      hostId: OFFICE_HOST_ID,
      name: '工位 Windows',
      os: 'windows',
      hostVersion: '0.3.0',
      fp: 'different',
    });
    expect(match?.computer.name).toBe('工位 Windows');
    expect(match?.by).toBe('hostId');
  });

  it('counts a hostId-only hello as a fingerprint match', () => {
    const match = matchComputer('10.30.99.9', computers, {
      hostId: OFFICE_HOST_ID,
    });
    expect(match?.computer.name).toBe('工位 Windows');
    expect(match?.by).toBe('fp');
  });

  it('returns null for an unknown address without hello', () => {
    expect(matchComputer('10.30.99.9', computers, null)).toBeNull();
  });
});

describe('applyProbe', () => {
  it('stays running until rs, rdp and ssh final results arrive', () => {
    const started = startProbe(1_000);
    const afterRs = applyProbe(started, 'rs', result('p1'), 1_023);
    expect(afterRs.state).toBe('running');
    expect(afterRs.hello).toBeNull();
    expect(afterRs.elapsedMs).toBeNull();
  });

  it('becomes done with elapsed time when all three final results arrive', () => {
    let state = startProbe(1_000);
    state = applyProbe(state, 'rs', result('p1'), 1_023);
    state = applyProbe(state, 'rdp', result('p2', 'ok', 3389), 1_120);
    state = applyProbe(state, 'ssh', result('p3', 'timeout', 22), 1_400);
    expect(state).toMatchObject({
      state: 'done',
      elapsedMs: 400,
      rdp: 'open',
      ssh: 'closed',
    });
  });

  it('maps reflected and other failures to closed', () => {
    let state = startProbe(1_000);
    state = applyProbe(state, 'rs', result('p1'), 1_000);
    state = applyProbe(state, 'rdp', result('p2', 'refused', 3389), 1_000);
    state = applyProbe(state, 'ssh', result('p3', 'noRoute', 22), 1_000);
    expect(state.rdp).toBe('closed');
    expect(state.ssh).toBe('closed');
  });

  it('ignores preliminary results and only keeps the last one', () => {
    let state = startProbe(1_000);
    state = applyProbe(
      state,
      'rdp',
      result('p2', 'timeout', 3389, { last: false }),
      1_100,
    );
    expect(state.rdp).toBeNull();
    state = applyProbe(state, 'rdp', result('p2', 'ok', 3389), 1_200);
    expect(state.rdp).toBe('open');
  });

  it('ignores duplicates and events after done', () => {
    let state = startProbe(1_000);
    state = applyProbe(state, 'rs', result('p1'), 1_100);
    state = applyProbe(state, 'rs', result('p1b'), 1_200);
    expect(state.rs?.probeId).toBe('p1');
  });

  it('does not accept a late event after timeout', () => {
    const timedOut = timeoutProbe(startProbe(1_000), 6_000);
    expect(applyProbe(timedOut, 'rs', result('p1'), 6_100)).toBe(timedOut);
  });
});

describe('timeoutProbe', () => {
  it('marks missing ports closed and records elapsed time', () => {
    const state = timeoutProbe(startProbe(1_000), 6_000);
    expect(state).toMatchObject({
      state: 'done',
      rs: null,
      rdp: 'closed',
      ssh: 'closed',
      elapsedMs: 5_000,
    });
  });

  it('leaves idle and done states untouched', () => {
    expect(timeoutProbe(IDLE_PROBE, 6_000)).toBe(IDLE_PROBE);
    const done: AddrProbe = {
      ...startProbe(1_000),
      state: 'done',
      elapsedMs: 10,
    };
    expect(timeoutProbe(done, 6_000)).toBe(done);
  });
});

describe('addrVerdict', () => {
  it('mirrors idle and running', () => {
    expect(addrVerdict(IDLE_PROBE, null)).toBe('idle');
    expect(addrVerdict(startProbe(1_000), null)).toBe('running');
  });

  it('maps a successful probe to known or new', () => {
    const done: AddrProbe = {
      ...startProbe(1_000),
      state: 'done',
      rs: result('p1'),
    };
    const match = matchComputer('10.30.12.48', computers, null);
    expect(addrVerdict(done, match)).toBe('known');
    expect(addrVerdict(done, null)).toBe('new');
  });

  it('maps any failure to notFound', () => {
    const done: AddrProbe = {
      ...startProbe(1_000),
      state: 'done',
      rs: result('p1', 'refused'),
    };
    expect(addrVerdict(done, null)).toBe('notFound');
  });
});

describe('applyDiscover', () => {
  function rsDone(discover: boolean): AddrProbe {
    let state = startProbe(1_000, discover);
    state = applyProbe(state, 'rs', result('p1', 'refused'), 1_010);
    return applyProbe(state, 'ssh', result('p2', 'timeout', 22), 1_020);
  }

  it('waits for 5900 and 3389 when probing through discover', () => {
    let state = rsDone(true);
    state = applyDiscover(state, 'rdp', 'refused', 1_100);
    expect(state.state).toBe('running');
    state = applyDiscover(state, 'vnc', 'open', 1_200);
    expect(state).toMatchObject({
      state: 'done',
      rdp: 'refused',
      vnc: 'open',
      elapsedMs: 200,
    });
  });

  it('maps mismatch and timeout to closed and keeps the first result', () => {
    let state = rsDone(true);
    state = applyDiscover(state, 'vnc', 'mismatch', 1_100);
    state = applyDiscover(state, 'vnc', 'open', 1_110);
    state = applyDiscover(state, 'rdp', 'timeout', 1_120);
    expect(state).toMatchObject({ vnc: 'closed', rdp: 'closed' });
  });

  it('is ignored without discover and after a timeout', () => {
    const legacy = startProbe(1_000);
    expect(applyDiscover(legacy, 'vnc', 'open', 1_100)).toBe(legacy);
    const timedOut = timeoutProbe(startProbe(1_000, true), 6_000);
    expect(timedOut).toMatchObject({ vnc: 'closed', rdp: 'closed' });
    expect(applyDiscover(timedOut, 'vnc', 'open', 6_100)).toBe(timedOut);
  });

  it('settles SSH from the TCP probe once 5900 and 3389 are in', () => {
    let state = startProbe(1_000, true);
    state = applyProbe(state, 'rs', result('p1', 'refused'), 1_010);
    state = applyDiscover(state, 'rdp', 'timeout', 1_020);
    state = applyDiscover(state, 'vnc', 'refused', 1_030);
    expect(state).toMatchObject({ state: 'running', ssh: null });
    state = applyDiscover(state, 'ssh', 'open', 1_040);
    expect(state).toMatchObject({ state: 'done', ssh: 'open', elapsedMs: 40 });
  });

  it.each(['refused', 'timeout', 'mismatch'] as const)(
    'maps a %s SSH TCP probe to closed and keeps the first result',
    outcome => {
      const running = startProbe(1_000, true);
      const state = applyDiscover(running, 'ssh', outcome, 1_010);
      expect(state.ssh).toBe('closed');
      expect(applyDiscover(state, 'ssh', 'open', 1_020).ssh).toBe('closed');
    },
  );

  it('ignores an SSH TCP probe on the legacy path', () => {
    const legacy = startProbe(1_000);
    expect(applyDiscover(legacy, 'ssh', 'open', 1_100)).toBe(legacy);
  });

  it('does not wait for 5900 on the legacy path', () => {
    let state = startProbe(1_000);
    state = applyProbe(state, 'rs', result('p1'), 1_010);
    state = applyProbe(state, 'rdp', result('p2', 'ok', 3389), 1_020);
    state = applyProbe(state, 'ssh', result('p3', 'timeout', 22), 1_030);
    expect(state).toMatchObject({ state: 'done', vnc: null, rdp: 'open' });
    expect(timeoutProbe(startProbe(1_000), 6_000).vnc).toBeNull();
  });
});

describe('nativeOffers and sharingOff', () => {
  function done(vnc: AddrProbe['vnc'], rdp: AddrProbe['rdp']): AddrProbe {
    return {
      ...startProbe(1_000, true),
      state: 'done',
      rs: result('p1', 'refused'),
      ssh: 'closed',
      vnc,
      rdp,
    };
  }

  it('offers Screen Sharing before Remote Desktop', () => {
    expect(nativeOffers(done('open', 'open'))).toEqual([
      { os: 'macos', protocol: 'vnc', port: 5900 },
      { os: 'windows', protocol: 'rdp', port: 3389 },
    ]);
    expect(nativeOffers(done('closed', 'open'))).toEqual([
      { os: 'windows', protocol: 'rdp', port: 3389 },
    ]);
  });

  it('reports refused services only when nothing native is open', () => {
    expect(sharingOff(done('refused', 'closed'), null)).toEqual(['macos']);
    expect(sharingOff(done('refused', 'refused'), null)).toEqual([
      'macos',
      'windows',
    ]);
    expect(sharingOff(done('refused', 'refused'), 'windows')).toEqual([
      'windows',
    ]);
    expect(sharingOff(done('open', 'refused'), null)).toEqual([]);
  });

  it('offers nothing on the legacy path or while running', () => {
    const legacy: AddrProbe = {
      ...done(null, 'open'),
      discover: false,
    };
    expect(nativeOffers(legacy)).toEqual([]);
    expect(sharingOff(legacy, null)).toEqual([]);
    expect(nativeOffers(startProbe(1_000, true))).toEqual([]);
  });
});
