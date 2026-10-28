import { act, cleanup, waitFor } from '@testing-library/react-native';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import { ADAPTER_RS_E } from '../../native/rscore/types';
import type { AdapterTarget } from '../../native/rscore/types';
import { renderWithProviders } from '../../test/render';
import { installFakeAdapters } from './testing/fakeAdapters';
import {
  useAdapterSession,
  type AdapterSessionResult,
} from './useAdapterSession';

const vncTarget: AdapterTarget = {
  protocol: 'vnc',
  hostId: 'h1',
  addr: '100.64.0.7',
};
const opts = { viewWidth: 390, viewHeight: 844, scale: 3 };

let current: AdapterSessionResult | null = null;
let clipboard: string[] = [];

function Probe({ target = vncTarget }: { target?: AdapterTarget }) {
  current = useAdapterSession({
    target,
    opts,
    onClipboard: text => clipboard.push(text),
  });
  return null;
}

async function flush(): Promise<void> {
  await act(async () => {
    await Promise.resolve();
  });
}

afterEach(async () => {
  await cleanup();
  current = null;
  clipboard = [];
});

it('opens on mount and exposes the adapter session id', async () => {
  const core = createFakeCore();
  const adapters = installFakeAdapters(core);
  await renderWithProviders(<Probe />, { core });
  await flush();

  expect(adapters.calls[0]).toEqual({
    method: 'open',
    args: [vncTarget, opts],
  });
  expect(current?.state.sessionId).toBe('4611686018427387905');
  expect(current?.state.phase).toBe('connecting');
});

it('buffers events received before open resolves', async () => {
  const core = createFakeCore();
  const adapters = installFakeAdapters(core);
  adapters.holdNextOpen();
  await renderWithProviders(<Probe />, { core });
  await act(async () => {
    core.emit({
      type: 'adapter.state',
      sessionId: '4611686018427387905',
      protocol: 'vnc',
      phase: 'needsCredentials',
      credFields: ['password'],
      credError: false,
    });
  });
  await act(async () => {
    adapters.resolveOpen();
    await Promise.resolve();
  });
  expect(current?.state.phase).toBe('needsCredentials');
  expect(current?.state.credFields).toEqual(['password']);
});

it('submits trimmed credentials for the active session', async () => {
  const core = createFakeCore();
  const adapters = installFakeAdapters(core);
  await renderWithProviders(<Probe />, { core });
  await flush();
  await act(async () => {
    core.emit({
      type: 'adapter.state',
      sessionId: '4611686018427387905',
      protocol: 'vnc',
      phase: 'needsCredentials',
      credFields: ['user', 'password'],
      credError: false,
    });
  });
  current?.submit({ user: ' alice ', password: 'pw', domain: '' }, true);
  expect(adapters.calls.at(-1)).toEqual({
    method: 'submitCredentials',
    args: [
      '4611686018427387905',
      { user: 'alice', password: 'pw', domain: undefined, save: true },
    ],
  });
});

it('handles certificate trust and ended errors', async () => {
  const core = createFakeCore();
  const adapters = installFakeAdapters(core);
  await renderWithProviders(<Probe />, { core });
  await flush();
  await act(async () => {
    core.emit({
      type: 'adapter.certPrompt',
      sessionId: '4611686018427387905',
      fp: 'aa:bb',
    });
  });
  expect(current?.state.cert).toEqual({ fp: 'aa:bb' });
  await act(async () => {
    current?.trustCert(true);
  });
  expect(adapters.calls.at(-1)).toEqual({
    method: 'trustCert',
    args: ['4611686018427387905', true],
  });
  expect(current?.state.cert).toBeNull();

  await act(async () => {
    core.emit({
      type: 'adapter.state',
      sessionId: '4611686018427387905',
      protocol: 'vnc',
      phase: 'ended',
      credError: false,
      error: {
        code: 'authFailed',
        rsCode: ADAPTER_RS_E.authFailed,
        message: 'bad password',
      },
    });
  });
  expect(current?.state.error?.code).toBe('authFailed');
});

it('closes on unmount, including an open that resolves afterwards', async () => {
  const core = createFakeCore();
  const adapters = installFakeAdapters(core);
  const rendered = await renderWithProviders(<Probe />, { core });
  await flush();
  await rendered.unmount();
  expect(adapters.calls.at(-1)).toEqual({
    method: 'close',
    args: ['4611686018427387905'],
  });

  const core2 = createFakeCore();
  const adapters2 = installFakeAdapters(core2);
  adapters2.holdNextOpen();
  const rendered2 = await renderWithProviders(<Probe />, { core: core2 });
  await rendered2.unmount();
  await act(async () => {
    adapters2.resolveOpen();
    await Promise.resolve();
  });
  expect(adapters2.calls.at(-1)).toEqual({
    method: 'close',
    args: ['4611686018427387905'],
  });
});

it('retries with saved credentials and ignores old events', async () => {
  const core = createFakeCore();
  const adapters = installFakeAdapters(core);
  await renderWithProviders(<Probe />, { core });
  await flush();
  await act(async () => {
    current?.retry();
    await Promise.resolve();
  });
  expect(adapters.calls.slice(-2)).toEqual([
    { method: 'close', args: ['4611686018427387905'] },
    { method: 'open', args: [vncTarget, { ...opts, useSaved: true }] },
  ]);
  await act(async () => {
    core.emit({
      type: 'adapter.state',
      sessionId: '4611686018427387905',
      protocol: 'vnc',
      phase: 'ended',
      credError: false,
    });
  });
  expect(current?.state.sessionId).toBe('4611686018427387906');
  expect(current?.state.phase).toBe('connecting');
});

it('sends clipboard according to protocol and native support', async () => {
  const core = createFakeCore();
  installFakeAdapters(core);
  await renderWithProviders(<Probe />, { core });
  await flush();
  expect(current?.sendClipboard('')).toBe('empty');
  expect(current?.sendClipboard('héllo')).toBe('sent');
  // V21: VNC's Latin-1 limit is its own result, not "unsupported".
  expect(current?.sendClipboard('你好')).toBe('latin1Only');
  core.failNext(
    'sessions.clipboardSend',
    new RsCoreError('unsupported', 'clipboard'),
  );
  expect(current?.sendClipboard('x')).toBe('unsupported');
  core.failNext('sessions.clipboardSend', new Error('x'));
  expect(current?.sendClipboard('x')).toBe('failed');

  const rdpCore = createFakeCore();
  installFakeAdapters(rdpCore);
  await renderWithProviders(
    <Probe target={{ protocol: 'rdp', addr: 'win' }} />,
    {
      core: rdpCore,
    },
  );
  await flush();
  expect(current?.sendClipboard('你好')).toBe('sent');
});

it('filters clipboard and screen events by active session', async () => {
  const core = createFakeCore();
  const adapters = installFakeAdapters(core);
  await renderWithProviders(<Probe />, { core });
  await flush();
  await act(async () => {
    adapters.emitScreens('4611686018427387905', [
      { x: 0, y: 0, width: 1920, height: 1080 },
    ]);
  });
  await waitFor(() => expect(current?.screens).toHaveLength(1));
  await act(async () => {
    core.emit({
      type: 'adapter.clipboard',
      sessionId: '4611686018427387905',
      text: 'hi',
    });
    core.emit({
      type: 'adapter.clipboard',
      sessionId: 'other',
      text: 'no',
    });
  });
  expect(clipboard).toEqual(['hi']);
});

it('reports rejected opens and type-password failures', async () => {
  const core = createFakeCore();
  const adapters = installFakeAdapters(core);
  adapters.holdNextOpen();
  await renderWithProviders(<Probe />, { core });
  await act(async () => {
    adapters.rejectOpen(new RsCoreError('unsupported', 'no bindings'));
    await Promise.resolve();
  });
  expect(current?.state.phase).toBe('ended');
  expect(current?.state.error?.rsCode).toBe(ADAPTER_RS_E.broken);

  const core2 = createFakeCore();
  const adapters2 = installFakeAdapters(core2);
  await renderWithProviders(<Probe />, { core: core2 });
  await flush();
  expect(current?.typePassword()).toBe(true);
  adapters2.failNext('typePassword', new Error('gone'));
  expect(current?.typePassword()).toBe(false);
});

it('resizes active sessions and records selecting a screen', async () => {
  const core = createFakeCore();
  const adapters = installFakeAdapters(core);
  await renderWithProviders(<Probe />, { core });
  await flush();
  current?.resize(800, 600, 2);
  adapters.selectScreen('4611686018427387905', 1);
  expect(adapters.calls.slice(-2)).toEqual([
    { method: 'resize', args: ['4611686018427387905', 800, 600, 2] },
    { method: 'selectScreen', args: ['4611686018427387905', 1] },
  ]);
});

it('isAlive checks the native session list', async () => {
  const core = createFakeCore();
  const adapters = installFakeAdapters(core);
  await renderWithProviders(<Probe />, { core });
  await flush();

  expect(current?.isAlive()).toBe(true);
  adapters.dropSession('4611686018427387905');
  expect(current?.isAlive()).toBe(false);
  adapters.failNext('list', new Error('gone'));
  expect(current?.isAlive()).toBeNull();
});

it('end closes the session and shows it as ended with the error', async () => {
  const core = createFakeCore();
  const adapters = installFakeAdapters(core);
  await renderWithProviders(<Probe />, { core });
  await flush();
  const error = {
    code: 'io' as const,
    rsCode: ADAPTER_RS_E.broken,
    message: 'gone',
  };

  await act(async () => current?.end(error));
  expect(adapters.calls.at(-1)).toEqual({
    method: 'close',
    args: ['4611686018427387905'],
  });
  expect(current?.state.phase).toBe('ended');
  expect(current?.state.error).toEqual(error);
});

it('retry can skip the saved credentials and submit reports failures', async () => {
  const core = createFakeCore();
  const adapters = installFakeAdapters(core);
  await renderWithProviders(<Probe />, { core });
  await flush();

  adapters.failNext('submitCredentials', new Error('gone'));
  expect(current?.submit({ user: '', password: 'pw', domain: '' }, false)).toBe(
    false,
  );
  expect(core.calls).toContainEqual({
    method: 'logs.write',
    args: ['warn', 'adapters', 'submit credentials failed'],
  });
  expect(current?.submit({ user: '', password: 'pw', domain: '' }, false)).toBe(
    true,
  );

  await act(async () => current?.retry({ useSaved: false }));
  await flush();
  expect(adapters.calls.filter(call => call.method === 'open').at(-1)).toEqual({
    method: 'open',
    args: [vncTarget, { ...opts, useSaved: false }],
  });
});

it('sendCtrlAltDel reports whether it was sent', async () => {
  const core = createFakeCore();
  const adapters = installFakeAdapters(core);
  await renderWithProviders(
    <Probe target={{ protocol: 'rdp', addr: 'win' }} />,
    { core },
  );
  await flush();

  expect(current?.sendCtrlAltDel()).toBe(true);
  adapters.failNext('sendCtrlAltDel', new Error('gone'));
  expect(current?.sendCtrlAltDel()).toBe(false);
});
