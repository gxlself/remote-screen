import type { Computer, CoreEventOf } from '../../native/rscore/types';
import {
  computers,
  MIN_HOST_VERSION,
  tailnetRunning,
} from '../../native/rscore/fixtures';
import {
  advice,
  applyEmbeddedPaths,
  applyProbeResult,
  derpNamesFor,
  expectedRows,
  finishRows,
  rowKey,
} from './probeRows';

const officeFixture = computers.find(
  (computer): computer is Computer => computer.node === 'office-win',
);
if (officeFixture === undefined) {
  throw new Error('office fixture missing');
}
const office: Computer = officeFixture;

const studioFixture = computers.find(
  (computer): computer is Computer => computer.node === 'studio-mac',
);
if (studioFixture === undefined) {
  throw new Error('studio fixture missing');
}
const studio: Computer = studioFixture;

function result(
  patch: Partial<CoreEventOf<'probe.result'>>,
): CoreEventOf<'probe.result'> {
  return {
    type: 'probe.result',
    probeId: 'p1',
    hostId: office.hostId,
    path: 'tailnetDirect',
    addr: '100.64.0.12',
    port: 7400,
    transport: 'quic',
    outcome: 'ok',
    ms: 20,
    last: false,
    ...patch,
  };
}

describe('expectedRows', () => {
  it('expands the office computer into its four candidate paths', () => {
    expect(expectedRows(office).map(row => row.key)).toEqual([
      rowKey('tailnetDirect', '100.64.0.12', 'quic'),
      rowKey('tailnetDerp', '100.64.0.12', 'quic'),
      rowKey('intranet', '10.30.12.48', 'quic'),
      rowKey('intranet', '10.30.12.48', 'tcp'),
    ]);
    expect(expectedRows(office)).toEqual([
      {
        key: rowKey('tailnetDirect', '100.64.0.12', 'quic'),
        path: 'tailnetDirect',
        transport: 'quic',
        addr: '100.64.0.12',
        port: 7400,
        state: 'pending',
        outcome: null,
        ms: null,
        reason: null,
      },
      {
        key: rowKey('tailnetDerp', '100.64.0.12', 'quic'),
        path: 'tailnetDerp',
        transport: 'quic',
        addr: '100.64.0.12',
        port: 7400,
        state: 'pending',
        outcome: null,
        ms: null,
        reason: null,
      },
      {
        key: rowKey('intranet', '10.30.12.48', 'quic'),
        path: 'intranet',
        transport: 'quic',
        addr: '10.30.12.48',
        port: 7400,
        state: 'pending',
        outcome: null,
        ms: null,
        reason: null,
      },
      {
        key: rowKey('intranet', '10.30.12.48', 'tcp'),
        path: 'intranet',
        transport: 'tcp',
        addr: '10.30.12.48',
        port: 7400,
        state: 'pending',
        outcome: null,
        ms: null,
        reason: null,
      },
    ]);
  });
});

describe('applyProbeResult', () => {
  it('fills known rows and appends an unknown address', () => {
    let rows = expectedRows(office);
    rows = applyProbeResult(
      rows,
      result({ outcome: 'timeout', ms: 3000, reason: '打洞失败' }),
    );
    expect(rows[0]).toMatchObject({
      state: 'fail',
      outcome: 'timeout',
      ms: 3000,
      reason: '打洞失败',
    });

    const appended = applyProbeResult(
      rows,
      result({ path: 'lan', addr: '192.168.31.99', outcome: 'ok', ms: 9 }),
    );
    expect(appended).toHaveLength(5);
    expect(appended[4]).toMatchObject({
      key: rowKey('lan', '192.168.31.99', 'quic'),
      path: 'lan',
      addr: '192.168.31.99',
      state: 'ok',
      ms: 9,
    });
  });

  it('keeps a free-form failure reason from the event', () => {
    const rows = applyProbeResult(
      expectedRows(office),
      result({ outcome: 'timeout', ms: 3000, reason: '打洞失败' }),
    );
    expect(rows[0].reason).toBe('打洞失败');
  });

  it('maps every non-ok outcome to fail', () => {
    for (const outcome of [
      'timeout',
      'refused',
      'fingerprintMismatch',
      'noRoute',
      'needsVpn',
      'skipped',
    ] as const) {
      const rows = applyProbeResult(expectedRows(office), result({ outcome }));
      expect(rows[0].state).toBe('fail');
      expect(rows[0].outcome).toBe(outcome);
    }
  });
});

describe('finishRows', () => {
  it('turns pending rows into timeout failures and keeps results', () => {
    const rows = applyProbeResult(
      expectedRows(office),
      result({ outcome: 'ok', ms: 23 }),
    );
    const finished = finishRows(rows);

    expect(finished[0]).toMatchObject({ state: 'ok', outcome: 'ok', ms: 23 });
    expect(finished.slice(1)).toEqual(
      rows.slice(1).map(row => ({
        ...row,
        state: 'fail',
        outcome: 'timeout',
        ms: null,
        reason: null,
      })),
    );
  });
});

describe('applyEmbeddedPaths', () => {
  it('moves a relayed peer result to the DERP row without mutating inputs', () => {
    const rows = applyProbeResult(
      expectedRows(office),
      result({ outcome: 'ok', ms: 48 }),
    );
    const snapshot = structuredClone(rows);

    const adjusted = applyEmbeddedPaths(rows, tailnetRunning);

    expect(adjusted[0]).toMatchObject({
      key: rowKey('tailnetDirect', '100.64.0.12', 'quic'),
      state: 'fail',
      outcome: 'timeout',
      ms: null,
      reason: 'holePunchFailed',
    });
    expect(adjusted[1]).toMatchObject({
      key: rowKey('tailnetDerp', '100.64.0.12', 'quic'),
      state: 'ok',
      outcome: 'ok',
      ms: 48,
      reason: null,
    });
    expect(adjusted.map(row => row.key)).toEqual(rows.map(row => row.key));
    expect(rows).toEqual(snapshot);
  });

  it('skips the DERP row when the peer is direct', () => {
    const rows = applyProbeResult(
      expectedRows(studio),
      result({
        path: 'tailnetDirect',
        addr: '100.64.0.7',
        outcome: 'ok',
        ms: 11,
      }),
    );
    const direct = rows[0];

    const adjusted = applyEmbeddedPaths(rows, tailnetRunning);

    expect(adjusted[0]).toEqual(direct);
    expect(adjusted[1]).toMatchObject({
      key: rowKey('tailnetDerp', '100.64.0.7', 'quic'),
      state: 'fail',
      outcome: 'skipped',
      ms: null,
      reason: 'skipped',
    });
  });

  it('leaves pending and failed direct rows unchanged', () => {
    const pending = expectedRows(office);
    expect(applyEmbeddedPaths(pending, tailnetRunning)).toEqual(pending);

    const failed = applyProbeResult(
      expectedRows(office),
      result({ outcome: 'timeout', ms: 3000, reason: '打洞失败' }),
    );
    expect(applyEmbeddedPaths(failed, tailnetRunning)).toEqual(failed);
  });

  it('returns the rows unchanged without tailnet status', () => {
    const rows = applyProbeResult(
      expectedRows(office),
      result({ outcome: 'ok', ms: 48 }),
    );
    expect(applyEmbeddedPaths(rows, null)).toEqual(rows);
  });
});

describe('derpNamesFor', () => {
  it('maps tailnet addresses to the peer DERP region name', () => {
    const rows = [...expectedRows(office), ...expectedRows(studio)];
    expect(derpNamesFor(rows, tailnetRunning)).toEqual({
      '100.64.0.12': '上海',
      '100.64.0.7': '上海',
    });
  });

  it('returns no names without tailnet status', () => {
    expect(derpNamesFor(expectedRows(office), null)).toEqual({});
  });
});

describe('advice', () => {
  it('is running until done and picks the fastest successful row', () => {
    const pending = expectedRows(office);
    expect(advice(pending, false)).toEqual({ kind: 'running' });

    let rows = applyProbeResult(
      pending,
      result({ outcome: 'timeout', ms: 3000, reason: '打洞失败' }),
    );
    rows = applyProbeResult(
      rows,
      result({ path: 'tailnetDerp', outcome: 'ok', ms: 48 }),
    );
    rows = applyProbeResult(
      rows,
      result({
        path: 'intranet',
        addr: '10.30.12.48',
        transport: 'quic',
        outcome: 'refused',
        ms: 3,
      }),
    );
    rows = applyProbeResult(
      rows,
      result({
        path: 'intranet',
        addr: '10.30.12.48',
        transport: 'tcp',
        outcome: 'ok',
        ms: 23,
      }),
    );

    const verdict = advice(rows, true);
    expect(verdict.kind).toBe('best');
    if (verdict.kind !== 'best') {
      throw new Error('expected best advice');
    }
    expect(verdict.row).toMatchObject({
      path: 'intranet',
      transport: 'tcp',
      ms: 23,
    });
    expect(verdict.tcpMode).toBe(true);
  });

  it('does not claim TCP mode when only DERP works', () => {
    const rows = applyProbeResult(
      expectedRows(office),
      result({ path: 'tailnetDerp', outcome: 'ok', ms: 48 }),
    );
    expect(advice(rows, true)).toMatchObject({
      kind: 'best',
      tcpMode: false,
      row: { path: 'tailnetDerp', ms: 48 },
    });
  });

  it('reports all failed and ignores failed rows when choosing best', () => {
    const allFailed = finishRows(expectedRows(office));
    expect(advice(allFailed, true)).toEqual({ kind: 'allFailed' });
    expect(advice(expectedRows(office), false)).toEqual({ kind: 'running' });
  });

  it('reports all failed when done with no rows', () => {
    expect(advice([], true)).toEqual({ kind: 'allFailed' });
  });
});

it('keeps the fixture host version in sync with the screen footer', () => {
  expect(MIN_HOST_VERSION).toBe('0.3.0');
});
