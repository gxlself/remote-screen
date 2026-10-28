import {
  cleanup,
  fireEvent,
  screen,
  waitFor,
} from '@testing-library/react-native';
import type { Directory } from '../../native/rscore/types';
import { adapters } from '../../i18n/zh/adapters';
import { createFakeCore } from '../../native/rscore/fakeCore';
import {
  computers,
  directory as fixtureDirectory,
} from '../../native/rscore/fixtures';
import { installFakeAdapters } from './testing/fakeAdapters';
import { renderWithProviders } from '../../test/render';
import { MemoryKv } from '../storage/kv';
import { AdapterEntryRow } from './AdapterEntryRow';
import { rememberedMonitors } from './monitorMemory';

// The row reads the directory; these cases need fields (console, a stopped
// Host) only a Hub reported, so the directory is handed in as it was.
let mockDirectory: Directory | undefined;
jest.mock('../data/directoryQuery', () => ({
  ...jest.requireActual('../data/directoryQuery'),
  useDirectory: () => ({
    directory: mockDirectory,
    isLoading: false,
    error: null,
    refetch: () => Promise.resolve(),
  }),
}));

const HOST_ID = 'h7v2m9q3k8x4tb1a';
const WINDOWS_HOST_ID = 'w3k8p2x7m4q9tb6c';

async function renderRow(
  directory?: Directory,
  available = true,
  hostId = HOST_ID,
  kv = new MemoryKv(),
) {
  mockDirectory = directory ?? fixtureDirectory;
  const core = createFakeCore();
  installFakeAdapters(core, { available });
  const onOpen = jest.fn();
  const rendered = await renderWithProviders(
    <AdapterEntryRow hostId={hostId} onOpen={onOpen} />,
    { core, kv },
  );
  return { ...rendered, onOpen };
}

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
});

it('renders and opens the Mac VNC entry', async () => {
  const rendered = await renderRow();

  expect(await screen.findByText(adapters.entry.vnc)).toBeOnTheScreen();
  expect(screen.getByText(adapters.entry.hint('vnc'))).toBeOnTheScreen();
  await fireEvent.press(screen.getByTestId('adapter-entry'));

  expect(rendered.onOpen).toHaveBeenCalledWith({
    target: { protocol: 'vnc', hostId: HOST_ID, addr: '100.64.0.7' },
    name: 'Studio Mac mini',
    addrKind: 'tailnet',
    context: 'manual',
  });
});

it('renders and opens the Windows RDP entry', async () => {
  const office = computers.find(
    computer => computer.hostId === WINDOWS_HOST_ID,
  );
  const rendered = await renderRow(fixtureDirectory, true, WINDOWS_HOST_ID);

  expect(await screen.findByText(adapters.entry.rdp)).toBeOnTheScreen();
  expect(screen.getByText(adapters.entry.hint('rdp'))).toBeOnTheScreen();
  expect(screen.queryByText(adapters.entry.vnc)).toBeNull();
  await fireEvent.press(screen.getByTestId('adapter-entry'));

  expect(rendered.onOpen).toHaveBeenCalledWith({
    target: { protocol: 'rdp', hostId: WINDOWS_HOST_ID, addr: '100.64.0.12' },
    name: office?.name,
    addrKind: 'tailnet',
    context: 'manual',
  });
});

it('remembers the RDP monitor count for this computer', async () => {
  const kv = new MemoryKv();
  await renderRow(fixtureDirectory, true, WINDOWS_HOST_ID, kv);

  const row = await screen.findByTestId('adapter-monitors-row');
  expect(screen.getByText(adapters.monitors.value(1))).toBeOnTheScreen();
  await fireEvent.press(row);
  expect(screen.getByText(adapters.monitors.note)).toBeOnTheScreen();
  await fireEvent.press(screen.getByTestId('adapter-monitors-2'));

  expect(screen.queryByTestId('adapter-monitors-sheet')).toBeNull();
  expect(screen.getByText(adapters.monitors.value(2))).toBeOnTheScreen();
  expect(
    rememberedMonitors(kv, {
      protocol: 'rdp',
      hostId: WINDOWS_HOST_ID,
      addr: '100.64.0.12',
    }),
  ).toBe(2);
});

it('has no monitor count for Screen Sharing', async () => {
  await renderRow();

  expect(await screen.findByTestId('adapter-entry')).toBeOnTheScreen();
  expect(screen.queryByTestId('adapter-monitors-row')).toBeNull();
});

it('does not show the Windows entry without adapters', async () => {
  await renderRow(fixtureDirectory, false, WINDOWS_HOST_ID);

  await waitFor(() => {
    expect(screen.queryByTestId('adapter-entry')).toBeNull();
  });
  expect(screen.queryByText(adapters.entry.rdp)).toBeNull();
});

it('uses login-window and no-host contexts', async () => {
  const base = computers.find(computer => computer.hostId === HOST_ID);
  if (base === undefined) {
    throw new Error('fixture host missing');
  }

  const login = await renderRow({
    ...fixtureDirectory,
    computers: [{ ...base, console: { ...base.console!, loginWindow: true } }],
  });
  await screen.findByTestId('adapter-entry');
  await fireEvent.press(screen.getByTestId('adapter-entry'));
  expect(login.onOpen).toHaveBeenCalledWith(
    expect.objectContaining({ context: 'loginWindow' }),
  );

  await cleanup();
  const noHost = await renderRow({
    ...fixtureDirectory,
    computers: [{ ...base, hostRunning: false }],
  });
  await screen.findByTestId('adapter-entry');
  await fireEvent.press(screen.getByTestId('adapter-entry'));
  expect(noHost.onOpen).toHaveBeenCalledWith(
    expect.objectContaining({ context: 'noHost' }),
  );
});

it('hides unavailable, offline, addressless, and unknown computers', async () => {
  const unavailable = await renderRow(undefined, false);
  expect(unavailable.queryByTestId('adapter-entry')).toBeNull();
  await cleanup();

  const offline = await renderRow({
    ...fixtureDirectory,
    computers: fixtureDirectory.computers.map(computer =>
      computer.hostId === HOST_ID ? { ...computer, online: false } : computer,
    ),
  });
  await waitFor(() => {
    expect(offline.queryByTestId('adapter-entry')).toBeNull();
  });

  await cleanup();
  const noAddress = await renderRow({
    ...fixtureDirectory,
    computers: fixtureDirectory.computers.map(computer =>
      computer.hostId === HOST_ID ? { ...computer, addresses: [] } : computer,
    ),
  });
  await waitFor(() => {
    expect(noAddress.queryByTestId('adapter-entry')).toBeNull();
  });

  await cleanup();
  const missing = await renderRow({
    ...fixtureDirectory,
    computers: fixtureDirectory.computers.filter(
      computer => computer.hostId !== HOST_ID,
    ),
  });
  await waitFor(() => {
    expect(missing.queryByTestId('adapter-entry')).toBeNull();
  });
});
