import { StrictMode } from 'react';
import { AccessibilityInfo, AppState, Text } from 'react-native';
import { act, fireEvent, render, screen } from '@testing-library/react-native';
import { parseLaunchProps, setLaunchOptions } from '../../app/launchOptions';
import { createFakePlatform } from '../../native/platform/fakePlatform';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { MemoryKv } from '../storage/kv';
import { defaultDeps } from './appServices';
import type { AppServices } from './appServices';
import { bootServices, createFakeServices, selectCoreMode } from './coreBoot';
import BootScreen from './BootScreen';
import DataProviders from './DataProviders';

jest.mock('./coreBoot', () => ({
  ...jest.requireActual('./coreBoot'),
  bootServices: jest.fn(),
  selectCoreMode: jest.fn(),
}));
jest.mock('../../native/devharness/log', () => ({ harnessLog: jest.fn() }));

const mockBoot = jest.mocked(bootServices);
const mockMode = jest.mocked(selectCoreMode);

function services(): AppServices {
  return createFakeServices(parseLaunchProps({}), {
    os: 'ios',
    platform: createFakePlatform(),
    loadReal: jest.fn(),
    createKv: () => new MemoryKv(),
    now: () => 0,
  });
}

function deferred() {
  let resolve!: (value: AppServices) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<AppServices>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

beforeEach(() => {
  jest.clearAllMocks();
  mockBoot.mockReset();
  mockMode.mockReturnValue({ kind: 'real' });
  setLaunchOptions(parseLaunchProps({ dataDir: '/app/support/core' }));
});

afterEach(() => {
  jest.restoreAllMocks();
  jest.useRealTimers();
  setLaunchOptions(parseLaunchProps({}));
});

it('renders fake-mode children immediately without starting an async boot', async () => {
  mockMode.mockReturnValue({ kind: 'fake', reason: 'noDataDir' });
  setLaunchOptions(parseLaunchProps({}));
  await render(
    <DataProviders>
      <Text>页面</Text>
    </DataProviders>,
  );
  expect(screen.getByText('页面')).toBeOnTheScreen();
  expect(screen.queryByTestId('boot-screen')).toBeNull();
  expect(mockBoot).not.toHaveBeenCalled();
});

it('fails the boot instead of showing demo data in a release build without a data directory', async () => {
  const globals = globalThis as { __DEV__?: boolean };
  const dev = globals.__DEV__;
  globals.__DEV__ = false;
  try {
    mockMode.mockReturnValue({ kind: 'fake', reason: 'noDataDir' });
    setLaunchOptions(parseLaunchProps({}));
    await render(
      <DataProviders>
        <Text>页面</Text>
      </DataProviders>,
    );
    expect(screen.getByText('启动失败')).toBeOnTheScreen();
    expect(
      screen.getByText('io: data directory unavailable'),
    ).toBeOnTheScreen();
    expect(screen.queryByText('重试')).toBeNull();
    expect(screen.queryByText('页面')).toBeNull();
    expect(mockBoot).not.toHaveBeenCalled();
  } finally {
    globals.__DEV__ = dev;
  }
});

it('gates real-mode children on successful init', async () => {
  const pending = deferred();
  mockBoot.mockReturnValue(pending.promise);
  await render(
    <DataProviders>
      <Text>页面</Text>
    </DataProviders>,
  );
  expect(screen.getByTestId('boot-screen')).toBeOnTheScreen();
  expect(screen.queryByText('页面')).toBeNull();
  await act(async () => pending.resolve(services()));
  expect(screen.getByText('页面')).toBeOnTheScreen();
  expect(screen.queryByTestId('boot-screen')).toBeNull();
});

it('shows an accessible failure, limits the message and retries successfully', async () => {
  const first = deferred();
  const second = deferred();
  mockBoot
    .mockReturnValueOnce(first.promise)
    .mockReturnValueOnce(second.promise);
  const announce = jest.spyOn(AccessibilityInfo, 'announceForAccessibility');
  await render(
    <DataProviders>
      <Text>页面</Text>
    </DataProviders>,
  );
  await act(async () =>
    first.reject(new RsCoreError('network', 'x'.repeat(250))),
  );
  expect(screen.getByRole('header', { name: '启动失败' })).toBeOnTheScreen();
  expect(screen.getByText(`network: ${'x'.repeat(200)}`)).toBeOnTheScreen();
  expect(announce).toHaveBeenCalledWith(
    `启动失败，network: ${'x'.repeat(200)}`,
  );
  expect(screen.queryByText('页面')).toBeNull();
  await fireEvent.press(screen.getByRole('button', { name: '重试' }));
  expect(mockBoot).toHaveBeenCalledTimes(2);
  expect(screen.queryByText('启动失败')).toBeNull();
  await act(async () => second.resolve(services()));
  expect(screen.getByText('页面')).toBeOnTheScreen();
});

it('reuses the same loaded Core when retrying init', async () => {
  const actual = jest.requireActual<typeof import('./coreBoot')>('./coreBoot');
  const core = createFakeCore();
  core.failNext('init', new RsCoreError('internal', 'try again'));
  const load = jest.spyOn(defaultDeps, 'loadReal').mockReturnValue(core);
  mockBoot.mockImplementation(actual.bootServices);
  await render(
    <DataProviders>
      <Text>页面</Text>
    </DataProviders>,
  );
  expect(screen.getByText('启动失败')).toBeOnTheScreen();
  expect(core.calls.map(call => call.method)).toEqual(['init']);
  await fireEvent.press(screen.getByRole('button', { name: '重试' }));
  expect(screen.getByText('页面')).toBeOnTheScreen();
  expect(load).toHaveBeenCalledTimes(1);
  expect(core.calls.filter(call => call.method === 'init')).toHaveLength(2);
  expect(
    core.calls.findIndex(call => call.method === 'settings.set'),
  ).toBeGreaterThan(1);
});

it('boots only once across rerenders and StrictMode effect replay', async () => {
  const pending = deferred();
  mockBoot.mockReturnValue(pending.promise);
  const view = await render(
    <StrictMode>
      <DataProviders>
        <Text>页面</Text>
      </DataProviders>
    </StrictMode>,
  );
  await view.rerender(
    <StrictMode>
      <DataProviders>
        <Text>更新</Text>
      </DataProviders>
    </StrictMode>,
  );
  expect(mockBoot).toHaveBeenCalledTimes(1);
  await act(async () => pending.resolve(services()));
  expect(screen.getByText('更新')).toBeOnTheScreen();
  expect(mockBoot).toHaveBeenCalledTimes(1);
});

it('keeps one init call when StrictMode replays effects', async () => {
  const actual = jest.requireActual<typeof import('./coreBoot')>('./coreBoot');
  const core = createFakeCore();
  jest.spyOn(defaultDeps, 'loadReal').mockReturnValue(core);
  mockBoot.mockImplementation(actual.bootServices);
  await render(
    <StrictMode>
      <DataProviders>
        <Text>页面</Text>
      </DataProviders>
    </StrictMode>,
  );
  expect(screen.getByText('页面')).toBeOnTheScreen();
  expect(core.calls.filter(call => call.method === 'init')).toHaveLength(1);
});

it.each(['resolve', 'reject'] as const)(
  'cleans up app focus and ignores a late %s after unmount',
  async outcome => {
    const pending = deferred();
    mockBoot.mockReturnValue(pending.promise);
    const remove = jest.fn();
    jest.spyOn(AppState, 'addEventListener').mockReturnValue({ remove });
    const view = await render(
      <DataProviders>
        <Text>页面</Text>
      </DataProviders>,
    );
    await view.unmount();
    expect(remove).toHaveBeenCalledTimes(1);
    await act(async () => {
      if (outcome === 'resolve') {
        pending.resolve(services());
      } else {
        pending.reject(new RsCoreError('internal', 'late failure'));
      }
    });
    expect(mockBoot).toHaveBeenCalledTimes(1);
  },
);

it('delays the spinner for 600 ms and resets the delay on retry', async () => {
  jest.useFakeTimers();
  const view = await render(<BootScreen state="booting" />);
  expect(screen.queryByTestId('boot-spinner')).toBeNull();
  await act(async () => {
    jest.advanceTimersByTime(599);
  });
  expect(screen.queryByTestId('boot-spinner')).toBeNull();
  await act(async () => {
    jest.advanceTimersByTime(1);
  });
  expect(screen.getByTestId('boot-spinner')).toBeOnTheScreen();
  await view.rerender(
    <BootScreen state="failed" message="internal: init failed" />,
  );
  expect(screen.queryByTestId('boot-spinner')).toBeNull();
  await view.rerender(<BootScreen state="booting" />);
  expect(screen.queryByTestId('boot-spinner')).toBeNull();
  await act(async () => {
    jest.advanceTimersByTime(600);
  });
  expect(screen.getByTestId('boot-spinner')).toBeOnTheScreen();
  await view.unmount();
});
