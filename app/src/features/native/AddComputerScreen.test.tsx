import {
  act,
  cleanup,
  fireEvent,
  screen,
  waitFor,
} from '@testing-library/react-native';
import { Linking, Platform } from 'react-native';
import type { RootScreenProps } from '../../app/navigation/routes';
import { native } from '../../i18n/zh/native';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { FakeCore } from '../../native/rscore/fakeCore';
import type { CoreEvent } from '../../native/rscore/types';
import type {
  BonjourEvent,
  BonjourService,
} from '../../native/specs/NativeRsBonjour';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import { installFakeAdapters } from '../adapters/testing/fakeAdapters';
import { localSyncGet } from '../data/localSync';
import type { ServersValue } from '../servers/manual';
import AddComputerScreen, { BONJOUR_WAIT_MS } from './AddComputerScreen';

let mockBonjourAvailable = true;
let mockBonjourListener: ((event: BonjourEvent) => void) | null = null;
const mockBonjourStop = jest.fn();

jest.mock('../../native/bonjour/bonjour', () => ({
  bonjourAvailable: () => mockBonjourAvailable,
  browseScreenSharing: (onEvent: (event: BonjourEvent) => void) => {
    mockBonjourListener = onEvent;
    return mockBonjourStop;
  },
}));
jest.mock('../../app/navigation/navigationRef', () => ({
  navigationRef: {
    navigate: jest.fn(),
    dispatch: jest.fn(),
    isReady: jest.fn(() => true),
  },
}));

const navigationRef = jest.requireMock('../../app/navigation/navigationRef')
  .navigationRef as { navigate: jest.Mock; dispatch: jest.Mock };

jest.setTimeout(20000);

function service(name: string, addrs: string[], port = 5900): BonjourService {
  return { name, host: `${name.replace(/\s/g, '-')}.local`, port, addrs };
}

async function bonjour(kind: 'found' | 'lost', item: BonjourService) {
  await act(async () => {
    mockBonjourListener?.({ kind, service: item });
    await Promise.resolve();
  });
}

async function emit(core: FakeCore, event: CoreEvent) {
  await act(async () => {
    core.emit(event);
    await Promise.resolve();
  });
}

function found(addr: string, port: number, scanId = '1'): CoreEvent {
  return { type: 'discover.found', scanId, addr, port };
}

function done(scanId = '1'): CoreEvent {
  return { type: 'discover.done', scanId, scanned: 254, cancelled: false };
}

async function renderScreen(core: FakeCore = createFakeCore()) {
  const navigation = mockNavigation<'AddComputer'>();
  const view = await renderWithProviders(
    <AddComputerScreen
      navigation={navigation}
      route={mockRoute('AddComputer')}
    />,
    { core },
  );
  await act(async () => {
    await Promise.resolve();
  });
  return {
    ...view,
    navigation: navigation as RootScreenProps<'AddComputer'>['navigation'] & {
      [k: string]: jest.Mock;
    },
    core,
  };
}

function keyboardBehavior(instance: { unstable_fiber?: unknown }) {
  let fiber = instance.unstable_fiber as
    | { memoizedProps?: Record<string, unknown>; return?: unknown }
    | null
    | undefined;
  while (fiber != null) {
    if (fiber.memoizedProps?.behavior !== undefined) {
      return fiber.memoizedProps.behavior;
    }
    fiber = fiber.return as typeof fiber;
  }
  return undefined;
}

async function press(name: string) {
  await fireEvent.press(screen.getByRole('button', { name }));
  await act(async () => {
    await Promise.resolve();
  });
}

beforeEach(() => {
  mockBonjourAvailable = true;
  mockBonjourListener = null;
  mockBonjourStop.mockReset();
  navigationRef.navigate.mockReset();
  navigationRef.dispatch.mockReset();
});

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
});

it('adds and removes Bonjour Macs, one row per name', async () => {
  await renderScreen();

  expect(screen.getByText(native.add.lanMacs)).toBeOnTheScreen();
  // Bonjour needs a moment; it says it is looking rather than "none found".
  expect(screen.getByText(native.add.lanSearching)).toBeOnTheScreen();
  expect(screen.queryByText(native.add.lanEmpty)).toBeNull();

  await bonjour(
    'found',
    service('Mac mini', ['192.168.31.40', '192.168.31.42']),
  );
  await bonjour('found', service('MacBook Air', ['192.168.31.41']));
  await bonjour('found', service('Mac mini', ['192.168.31.40']));

  expect(screen.getAllByText('Mac mini')).toHaveLength(1);
  expect(screen.getByText('192.168.31.40')).toBeOnTheScreen();
  expect(screen.getByText('192.168.31.41')).toBeOnTheScreen();
  expect(screen.queryByText(native.add.lanEmpty)).toBeNull();

  await bonjour('lost', service('Mac mini', ['192.168.31.40']));

  expect(screen.queryByText('Mac mini')).toBeNull();
  expect(screen.getByText('MacBook Air')).toBeOnTheScreen();
});

it('scans the LAN and lists each found address with progress', async () => {
  const { core } = await renderScreen();

  await press(native.add.scan);
  expect(core.calls).toContainEqual({
    method: 'discover.scanLan',
    args: [[5900, 3389]],
  });
  expect(screen.getByText(native.add.scanning)).toBeOnTheScreen();

  await emit(core, found('192.168.31.50', 3389));
  expect(screen.getByText('正在扫描… 找到 1 台')).toBeOnTheScreen();
  await emit(core, found('192.168.31.51', 5900));
  await emit(core, found('192.168.31.99', 5900, 'other-scan'));
  await emit(core, done());

  expect(screen.getByText('找到 2 台')).toBeOnTheScreen();
  expect(
    screen.getByRole('button', { name: '192.168.31.50，Windows · 远程桌面' }),
  ).toBeOnTheScreen();
  expect(
    screen.getByRole('button', { name: '192.168.31.51，Mac · 屏幕共享' }),
  ).toBeOnTheScreen();
  expect(screen.queryByText('192.168.31.99')).toBeNull();
  expect(
    screen.getByRole('button', { name: native.add.scan }),
  ).toBeOnTheScreen();
});

it('treats an address with both 5900 and 3389 as a Windows PC', async () => {
  const { core } = await renderScreen();

  await press(native.add.scan);
  // A Mac never listens on 3389: this is Windows with a VNC server too.
  await emit(core, found('192.168.31.60', 5900));
  await emit(core, found('192.168.31.60', 3389));
  await emit(core, found('192.168.31.61', 3389));
  await emit(core, found('192.168.31.61', 5900));
  await emit(core, done());

  expect(screen.getByText('找到 2 台')).toBeOnTheScreen();
  expect(
    screen.getByRole('button', { name: '192.168.31.60，Windows · 远程桌面' }),
  ).toBeOnTheScreen();
  expect(
    screen.getByRole('button', { name: '192.168.31.61，Windows · 远程桌面' }),
  ).toBeOnTheScreen();
});

it('cancels a running scan when pressed again', async () => {
  const { core } = await renderScreen();

  await press(native.add.scan);
  await press('停止扫描');

  expect(core.calls).toContainEqual({
    method: 'discover.cancelScan',
    args: ['1'],
  });
  expect(
    screen.getByRole('button', { name: native.add.scan }),
  ).toBeOnTheScreen();
});

it('stops Bonjour and cancels the scan when leaving the page', async () => {
  const view = await renderScreen();

  await press(native.add.scan);
  expect(mockBonjourStop).not.toHaveBeenCalled();
  await view.unmount();

  expect(mockBonjourStop).toHaveBeenCalledTimes(1);
  expect(view.core.calls).toContainEqual({
    method: 'discover.cancelScan',
    args: ['1'],
  });
});

it('saves a chosen Mac as a manual computer and connects to it', async () => {
  const core = createFakeCore();
  installFakeAdapters(core);
  const { kv } = await renderScreen(core);
  await bonjour('found', service('Mac mini', ['192.168.31.40']));

  await press('Mac mini，192.168.31.40');
  expect(screen.getByText('添加这台电脑')).toBeOnTheScreen();
  expect(screen.getByLabelText('名字')).toHaveDisplayValue('Mac mini');
  await fireEvent.changeText(screen.getByLabelText('名字'), ' 书房 Mac ');
  await press(native.add.save);

  await waitFor(() =>
    expect(navigationRef.dispatch).toHaveBeenCalledWith({
      type: 'REPLACE',
      payload: {
        name: 'AdapterSession',
        params: {
          target: { protocol: 'vnc', addr: '192.168.31.40' },
          name: '书房 Mac',
          addrKind: 'manual',
          context: 'native',
        },
      },
    }),
  );
  expect(navigationRef.navigate).not.toHaveBeenCalled();
  expect(localSyncGet<ServersValue>(kv, 'servers')?.value).toMatchObject({
    entries: [
      {
        kind: 'computer',
        host: '192.168.31.40',
        port: 5900,
        hostId: null,
        name: '书房 Mac',
        os: 'macos',
        protocol: 'vnc',
      },
    ],
  });
  expect(screen.queryByText('添加这台电脑')).toBeNull();
});

it('marks addresses already in the computer list as added and connects them', async () => {
  const { core } = await renderScreen();
  // 192.168.31.20 and 10.30.12.48 belong to paired Hosts.
  await bonjour('found', service('Studio Mac mini', ['192.168.31.20']));
  await press(native.add.scan);
  await emit(core, found('10.30.12.48', 3389));
  await emit(core, done());

  expect(
    screen.getByRole('button', {
      name: 'Studio Mac mini，192.168.31.20，已添加',
    }),
  ).toBeEnabled();
  const windows = screen.getByRole('button', {
    name: '10.30.12.48，Windows · 远程桌面，已添加',
  });
  expect(windows).toBeEnabled();

  // An added row opens that computer instead of adding it a second time.
  await fireEvent.press(windows);
  expect(screen.queryByText('添加这台电脑')).toBeNull();
  await waitFor(() =>
    expect(navigationRef.dispatch).toHaveBeenCalledWith(
      expect.objectContaining({ type: 'REPLACE' }),
    ),
  );
});

it('hides Bonjour and scanning without their native modules', async () => {
  mockBonjourAvailable = false;
  const core = createFakeCore();
  core.discover = undefined;
  const { navigation } = await renderScreen(core);

  expect(screen.queryByText(native.add.lanMacs)).toBeNull();
  expect(screen.queryByTestId('add-computer-bonjour')).toBeNull();
  expect(screen.queryByTestId('add-computer-scan')).toBeNull();
  expect(screen.queryByText(native.add.scan)).toBeNull();
  expect(mockBonjourListener).toBeNull();

  await press(native.add.manual);
  expect(navigation.navigate).toHaveBeenCalledWith('AddByAddress', undefined, {
    pop: true,
  });
  await press(native.add.installHost);
  expect(navigation.navigate).toHaveBeenCalledWith('PairAdd', undefined, {
    pop: true,
  });
});

it('keeps the page still on iOS and insets only the form scroll', async () => {
  jest.replaceProperty(Platform, 'OS', 'ios');
  await renderScreen();

  expect(
    keyboardBehavior(screen.getByTestId('add-computer-form')),
  ).toBeUndefined();
  const scroll = screen.getByTestId('add-computer-form-scroll');
  expect(scroll).toHaveProp('automaticallyAdjustKeyboardInsets', true);
  expect(scroll).toHaveProp('keyboardDismissMode', 'interactive');
  expect(scroll).toHaveProp('keyboardShouldPersistTaps', 'handled');
  expect(scroll).not.toContainElement(screen.getByLabelText('返回'));
  expect(scroll).toContainElement(
    screen.getByRole('button', { name: native.add.manual }),
  );
});

it('uses padding keyboard avoidance on android', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  await renderScreen();

  expect(keyboardBehavior(screen.getByTestId('add-computer-form'))).toBe(
    'padding',
  );
  expect(screen.getByTestId('add-computer-form-scroll')).toContainElement(
    screen.getByRole('button', { name: native.add.manual }),
  );
});

it('says nothing was found only after Bonjour had time, with a way to settings', async () => {
  jest.useFakeTimers();
  try {
    const openSettings = jest
      .spyOn(Linking, 'openSettings')
      .mockResolvedValue(undefined);
    await renderScreen();
    expect(
      screen.getByTestId('add-computer-bonjour-searching'),
    ).toBeOnTheScreen();
    await act(async () => {
      await jest.advanceTimersByTimeAsync(BONJOUR_WAIT_MS);
    });
    expect(screen.queryByTestId('add-computer-bonjour-searching')).toBeNull();
    expect(screen.getByText(native.add.lanEmpty)).toBeOnTheScreen();
    expect(screen.getByText(native.add.lanEmptyHint)).toBeOnTheScreen();
    await fireEvent.press(screen.getByTestId('add-computer-open-settings'));
    expect(openSettings).toHaveBeenCalledTimes(1);
  } finally {
    jest.useRealTimers();
  }
});

it('suggests the address flow when a scan finds nothing', async () => {
  const { core } = await renderScreen();
  await press(native.add.scan);
  expect(screen.getByTestId('add-computer-scan-spinner')).toBeOnTheScreen();
  await emit(core, done());
  expect(screen.queryByTestId('add-computer-scan-spinner')).toBeNull();
  expect(
    screen.getByText(`${native.add.scanFound(0)}。${native.add.scanEmptyHint}`),
  ).toBeOnTheScreen();
});

it('keeps the name dialog busy until the session opens', async () => {
  const core = createFakeCore();
  installFakeAdapters(core);
  const { queryClient } = await renderScreen(core);
  await bonjour('found', service('Mac mini', ['192.168.31.40']));
  await press('Mac mini，192.168.31.40');
  expect(screen.getByLabelText('名字')).toHaveProp('returnKeyType', 'go');
  let finishSave: () => void = () => undefined;
  jest.spyOn(queryClient, 'invalidateQueries').mockReturnValue(
    new Promise<void>(resolve => {
      finishSave = resolve;
    }),
  );
  await fireEvent.press(screen.getByTestId('add-computer-confirm'));
  await act(async () => {
    await Promise.resolve();
  });
  // Still saving: the dialog stays up with its button held.
  expect(screen.getByText('添加这台电脑')).toBeOnTheScreen();
  expect(screen.getByTestId('add-computer-confirm')).toBeBusy();
  expect(
    screen.getByRole('button', { name: '取消' }),
  ).toBeDisabled();
  expect(navigationRef.dispatch).not.toHaveBeenCalled();
  await act(async () => {
    finishSave();
  });
  await waitFor(() => expect(navigationRef.dispatch).toHaveBeenCalled());
});
