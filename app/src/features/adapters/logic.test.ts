import { createFakeCore } from '../../native/rscore/fakeCore';
import { ADAPTER_RS_E } from '../../native/rscore/types';
import type { AdapterEvent, AdapterViewState, CredInput } from './logic';
import {
  adapterProtocolFor,
  adapterReducer,
  adaptersAvailable,
  BUILTIN_PROTOCOLS,
  dockForPoint,
  fingerprintLines,
  isNetworkDrop,
  fallbackTargetFor,
  initialAdapterState,
  isEncryptedPath,
  isLatin1,
  targetForAddress,
  targetForComputer,
  toCredentials,
  validateCredentials,
} from './logic';
import { installFakeAdapters } from './testing/fakeAdapters';

const credentials: CredInput = { user: '', password: '', domain: '' };

function stateEvent(
  sessionId: string,
  patch: Partial<Extract<AdapterEvent, { type: 'adapter.state' }>> = {},
): Extract<AdapterEvent, { type: 'adapter.state' }> {
  return {
    type: 'adapter.state',
    sessionId,
    protocol: 'vnc',
    phase: 'connecting',
    credError: false,
    ...patch,
  };
}

function reduceEvents(
  events: AdapterEvent[],
  state: AdapterViewState = initialAdapterState,
): AdapterViewState {
  return events.reduce(
    (current, event) => adapterReducer(current, { type: 'event', event }),
    state,
  );
}

it('selects protocols and addresses without carrying the RSP port', () => {
  expect(adapterProtocolFor('macos')).toBe('vnc');
  expect(adapterProtocolFor('windows')).toBe('rdp');
  expect(BUILTIN_PROTOCOLS).toEqual(['vnc', 'rdp']);

  expect(
    targetForComputer({
      hostId: 'h1',
      os: 'macos',
      addresses: [
        { kind: 'lan', addr: '192.168.31.20', port: 7400 },
        { kind: 'tailnet', addr: '100.64.0.7', port: 7400 },
      ],
    }),
  ).toEqual({
    target: { protocol: 'vnc', hostId: 'h1', addr: '100.64.0.7' },
    addrKind: 'tailnet',
  });
  expect(
    targetForComputer({
      hostId: 'h1',
      os: 'windows',
      addresses: [
        { kind: 'lan', addr: '192.168.31.20', port: 7400 },
        { kind: 'intranet', addr: '10.30.4.8', port: 7400 },
      ],
    })?.target,
  ).toEqual({ protocol: 'rdp', hostId: 'h1', addr: '10.30.4.8' });
  expect(
    targetForComputer({ hostId: 'h1', os: 'macos', addresses: [] }),
  ).toBeNull();
});

it('normalizes manual targets and validates ports', () => {
  expect(targetForAddress('rdp', '[fd7a::1]', 3389)).toEqual({
    protocol: 'rdp',
    addr: 'fd7a::1',
  });
  expect(targetForAddress('vnc', ' 192.168.31.5 ', 5901)).toEqual({
    protocol: 'vnc',
    addr: '192.168.31.5',
    port: 5901,
  });
  expect(targetForAddress('vnc', 'hs.example.com')).toEqual({
    protocol: 'vnc',
    addr: 'hs.example.com',
  });
  expect(targetForAddress('vnc', '')).toBeNull();
  expect(targetForAddress('vnc', 'x', 0)).toBeNull();
  expect(targetForAddress('vnc', 'x', 70000)).toBeNull();
  expect(targetForAddress('vnc', 'x', 1.5)).toBeNull();
});

it('validates credentials by trimmed fields, utf8 bytes, and controls', () => {
  expect(validateCredentials(['user', 'password'], credentials)).toEqual({
    user: 'empty',
    password: 'empty',
  });
  expect(
    validateCredentials(['user', 'password'], {
      user: 'é'.repeat(129),
      password: 'pw',
      domain: '',
    }),
  ).toEqual({ user: 'length' });
  expect(
    validateCredentials(['user', 'password'], {
      user: 'é'.repeat(128),
      password: 'pw',
      domain: '',
    }),
  ).toEqual({});
  expect(
    validateCredentials(['user', 'password'], {
      user: 'alice',
      password: 'a'.repeat(1025),
      domain: '',
    }),
  ).toEqual({ password: 'length' });
  expect(
    validateCredentials(['user', 'password'], {
      user: 'a\nb',
      password: 'a\tb',
      domain: '',
    }),
  ).toEqual({ user: 'control', password: 'control' });
  expect(
    validateCredentials(['user', 'password'], {
      user: 'alice',
      password: 'a\u0085',
      domain: '',
    }),
  ).toEqual({ password: 'control' });
  expect(
    validateCredentials(['user', 'password', 'domain'], {
      user: '  alice  ',
      password: 'pw',
      domain: 'x'.repeat(257),
    }),
  ).toEqual({ domain: 'length' });
  expect(
    validateCredentials(['password'], {
      user: '',
      password: 'pw',
      domain: '',
    }),
  ).toEqual({});
});

it('builds credentials and path helpers', () => {
  expect(
    toCredentials(
      ['user', 'password', 'domain'],
      { user: ' alice ', password: ' pw ', domain: ' ' },
      true,
    ),
  ).toEqual({ user: 'alice', password: ' pw ', domain: undefined, save: true });
  expect(toCredentials(['password'], credentials, false)).toEqual({
    user: undefined,
    password: '',
    domain: undefined,
    save: false,
  });
  expect(isEncryptedPath('rdp', 'lan')).toBe(true);
  expect(isEncryptedPath('vnc', 'tailnet')).toBe(true);
  expect(isEncryptedPath('vnc', 'lan')).toBe(false);
  expect(isEncryptedPath('vnc', 'manual')).toBe(false);
  expect(isEncryptedPath('vnc', undefined)).toBe(false);
  expect(isLatin1('héllo')).toBe(true);
  expect(isLatin1('你好')).toBe(false);
  expect(isLatin1('')).toBe(true);
  expect(
    fallbackTargetFor({ protocol: 'vnc', addr: '100.64.0.7', port: 5901 }),
  ).toEqual({ os: 'macos', addr: '100.64.0.7', port: 5901 });
  expect(fallbackTargetFor({ protocol: 'rdp', addr: '10.30.12.48' })).toEqual({
    os: 'windows',
    addr: '10.30.12.48',
  });
});

it('checks adapter availability defensively', () => {
  const core = createFakeCore();
  expect(adaptersAvailable(core)).toBe(false);
  const adapters = installFakeAdapters(core);
  expect(adaptersAvailable(core)).toBe(true);
  adapters.setAvailable(false);
  expect(adaptersAvailable(core)).toBe(false);
  adapters.setAvailable(true);
  adapters.available = () => {
    throw new Error('old native package');
  };
  expect(adaptersAvailable(core)).toBe(false);
});

it('buffers pre-open events by id and caps the queue', () => {
  const pending = Array.from({ length: 33 }, (_, index) =>
    stateEvent(`p${index}`, { phase: 'connecting' }),
  );
  const withPending = reduceEvents(pending);
  expect(withPending.pending).toHaveLength(32);
  expect(withPending.pending[0]?.sessionId).toBe('p1');

  const state = reduceEvents([
    stateEvent('X', {
      phase: 'needsCredentials',
      credFields: ['password'],
    }),
    stateEvent('Y', { phase: 'authenticating' }),
  ]);
  const opened = adapterReducer(state, { type: 'opened', id: 'X' });
  expect(opened.phase).toBe('needsCredentials');
  expect(opened.credFields).toEqual(['password']);
  expect(opened.pending).toEqual([]);
  expect(
    adapterReducer(opened, {
      type: 'event',
      event: stateEvent('Y', { phase: 'ended' }),
    }),
  ).toBe(opened);
});

it('handles certificate prompts, terminal state, and open errors', () => {
  let state = adapterReducer(initialAdapterState, { type: 'opened', id: 'X' });
  state = adapterReducer(state, {
    type: 'event',
    event: {
      type: 'adapter.certPrompt',
      sessionId: 'X',
      fp: 'aa:bb',
      oldFp: 'cc:dd',
    },
  });
  expect(state.cert).toEqual({ fp: 'aa:bb', oldFp: 'cc:dd' });
  state = adapterReducer(state, {
    type: 'event',
    event: stateEvent('X', {
      phase: 'streaming',
      width: 1920,
      height: 1080,
      serverName: 'mac',
    }),
  });
  expect(state.cert).toBeNull();
  expect(state.width).toBe(1920);
  state = adapterReducer(state, {
    type: 'event',
    event: stateEvent('X', {
      phase: 'ended',
      error: {
        code: 'authFailed',
        rsCode: ADAPTER_RS_E.authFailed,
        message: 'bad',
      },
    }),
  });
  expect(state.error?.rsCode).toBe(ADAPTER_RS_E.authFailed);
  const failed = adapterReducer(initialAdapterState, {
    type: 'openFailed',
    message: 'boom',
  });
  expect(failed).toMatchObject({
    phase: 'ended',
    error: { code: 'io', rsCode: ADAPTER_RS_E.broken, message: 'boom' },
  });
});

it('only timeouts and broken links count as network drops (V01)', () => {
  expect(isNetworkDrop('timeout')).toBe(true);
  expect(isNetworkDrop('io')).toBe(true);
  for (const code of [
    'refused',
    'authFailed',
    'unsupportedSecurity',
    'nlaRequired',
    'certRejected',
    'serverClosed',
    'cancelled',
  ] as const) {
    expect(isNetworkDrop(code)).toBe(false);
  }
});

it('lost marks the session ended with the error and drops the prompt', () => {
  const error = {
    code: 'io' as const,
    rsCode: ADAPTER_RS_E.broken,
    message: 'x',
  };
  const state = adapterReducer(
    { ...initialAdapterState, phase: 'streaming', cert: { fp: 'aa' } },
    { type: 'lost', error },
  );
  expect(state).toMatchObject({ phase: 'ended', error, cert: null });
});

it('docks the toolbar by where it is dropped (V04)', () => {
  expect(dockForPoint(10, 900)).toBe('left');
  expect(dockForPoint(299, 900)).toBe('left');
  expect(dockForPoint(450, 900)).toBe('bottom');
  expect(dockForPoint(601, 900)).toBe('right');
  expect(dockForPoint(899, 900)).toBe('right');
});

it('splits fingerprints into lines of eight bytes (V47)', () => {
  expect(fingerprintLines('aa:bb')).toEqual(['aa:bb']);
  const bytes = Array.from({ length: 32 }, (_, index) =>
    index.toString(16).padStart(2, '0'),
  );
  const lines = fingerprintLines(bytes.join(':'));
  expect(lines).toHaveLength(4);
  expect(lines[0]).toBe(bytes.slice(0, 8).join(':'));
  expect(lines[3]).toBe(bytes.slice(24).join(':'));
});
