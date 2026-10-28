import { act, cleanup, fireEvent, screen } from '@testing-library/react-native';
import { StackActions } from '@react-navigation/native';
import type { QueryClient } from '@tanstack/react-query';
import { adapters } from '../../i18n/zh/adapters';
import { native } from '../../i18n/zh/native';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { computers } from '../../native/rscore/fixtures';
import { ADAPTER_RS_E } from '../../native/rscore/types';
import type {
  AdapterErrorCode,
  AdapterErrorInfo,
  AdapterProtocol,
} from '../../native/rscore/types';
import { renderWithProviders } from '../../test/render';
import type { ManualComputer } from '../servers/manual';
import { AdapterError } from './AdapterError';

jest.mock('../../app/navigation/navigationRef', () => ({
  navigationRef: {
    navigate: jest.fn(),
    dispatch: jest.fn(),
    isReady: jest.fn(() => true),
  },
}));

const navigationRef = jest.requireMock('../../app/navigation/navigationRef')
  .navigationRef as { navigate: jest.Mock; dispatch: jest.Mock };

/** V08: the guide replaces the error page instead of stacking on it. */
function guideFor(params: {
  os: 'macos' | 'windows';
  name: string;
  entryKey?: string;
}) {
  return StackActions.replace('EnableSharing', params);
}

const studio = computers[0];
const office = computers[1];
const clients: QueryClient[] = [];

const serviceOff: AdapterErrorInfo = {
  code: 'refused',
  rsCode: ADAPTER_RS_E.serviceOff,
  message: 'connection refused',
};

function manualMac(name: string): ManualComputer & { os: 'macos' } {
  return {
    id: 'addr-192-168-31-9-5900',
    kind: 'computer',
    host: '192.168.31.9',
    port: 5900,
    hostId: null,
    name,
    networkId: null,
    addedAtUs: 1,
    os: 'macos',
    protocol: 'vnc',
  };
}

async function renderError(
  error: AdapterErrorInfo,
  protocol: AdapterProtocol,
  name: string,
  /** Computers saved on this phone, beside the paired Hosts. */
  manual: ManualComputer[] = [],
) {
  const core = createFakeCore();
  const handlers = {
    onRetry: jest.fn(),
    onBack: jest.fn(),
    onOtherApp: jest.fn(),
    onCopyCode: jest.fn(),
  };
  const view = await renderWithProviders(
    <AdapterError
      error={error}
      name={name}
      protocol={protocol}
      {...handlers}
    />,
    { core, localDirectory: { manual } },
  );
  clients.push(view.queryClient);
  // Let the directory and the saved computers load.
  await act(async () => {
    await jest.advanceTimersByTimeAsync(1);
  });
  return { ...view, handlers };
}

function buttonLabels(): string[] {
  return screen
    .getAllByRole('button')
    .map(button => button.props.accessibilityLabel as string);
}

beforeEach(() => {
  jest.useFakeTimers();
  navigationRef.navigate.mockReset();
  navigationRef.dispatch.mockReset();
});

afterEach(async () => {
  await cleanup();
  for (const client of clients.splice(0)) {
    client.clear();
  }
  jest.useRealTimers();
});

it('205 on Screen Sharing opens the macOS guide for that entry', async () => {
  const { handlers } = await renderError(serviceOff, 'vnc', studio.name);

  const labels = buttonLabels();
  const retry = labels.indexOf(adapters.errors.retry);
  const howTo = labels.indexOf(native.enable.howTo);
  const otherApp = labels.indexOf(adapters.errors.otherApp);
  expect(retry).toBeGreaterThanOrEqual(0);
  expect(howTo).toBeGreaterThan(retry);
  expect(otherApp).toBeGreaterThan(howTo);
  expect(labels.indexOf(adapters.errors.back)).toBeGreaterThan(otherApp);

  await fireEvent.press(screen.getByText(native.enable.howTo));
  expect(navigationRef.navigate).not.toHaveBeenCalled();
  expect(navigationRef.dispatch).toHaveBeenCalledTimes(1);
  expect(navigationRef.dispatch).toHaveBeenCalledWith(
    guideFor({
      os: 'macos',
      name: studio.name,
      entryKey: `host:${studio.hostId}`,
    }),
  );
  expect(handlers.onRetry).not.toHaveBeenCalled();
  expect(handlers.onOtherApp).not.toHaveBeenCalled();
});

it('205 on Remote Desktop opens the Windows guide', async () => {
  await renderError(serviceOff, 'rdp', office.name);

  await fireEvent.press(screen.getByText(native.enable.howTo));
  expect(navigationRef.dispatch).toHaveBeenCalledWith(
    guideFor({
      os: 'windows',
      name: office.name,
      entryKey: `host:${office.hostId}`,
    }),
  );
});

it('finds a saved computer by the name the session was opened with', async () => {
  const mac = manualMac('NAS Mac');
  await renderError(serviceOff, 'vnc', 'NAS Mac', [mac]);

  await fireEvent.press(screen.getByText(native.enable.howTo));
  expect(navigationRef.dispatch).toHaveBeenCalledWith(
    guideFor({
      os: 'macos',
      name: 'NAS Mac',
      entryKey: `manual:${mac.id}`,
    }),
  );
});

it('leaves out entryKey when no single entry matches', async () => {
  await renderError(serviceOff, 'vnc', studio.name, [manualMac(studio.name)]);
  await fireEvent.press(screen.getByText(native.enable.howTo));
  expect(navigationRef.dispatch).toHaveBeenLastCalledWith(
    guideFor({
      os: 'macos',
      name: studio.name,
    }),
  );
  await cleanup();

  await renderError(serviceOff, 'vnc', '100.64.0.99');
  await fireEvent.press(screen.getByText(native.enable.howTo));
  expect(navigationRef.dispatch).toHaveBeenLastCalledWith(
    guideFor({
      os: 'macos',
      name: '100.64.0.99',
    }),
  );
});

it.each<[AdapterErrorCode, number]>([
  ['timeout', ADAPTER_RS_E.unreachable],
  ['authFailed', ADAPTER_RS_E.authFailed],
  ['unsupportedSecurity', ADAPTER_RS_E.unsupported],
  ['nlaRequired', ADAPTER_RS_E.unsupported],
  ['certRejected', ADAPTER_RS_E.certRejected],
  ['serverClosed', ADAPTER_RS_E.broken],
])('%s (RS-E%i) has no how-to button', async (code, rsCode) => {
  await renderError({ code, rsCode, message: code }, 'vnc', studio.name);

  expect(screen.getByTestId('adapter-error')).toBeOnTheScreen();
  expect(screen.queryByText(native.enable.howTo)).toBeNull();
});

it('keeps retry and the external app as the two buttons for a timeout', async () => {
  await renderError(
    { code: 'timeout', rsCode: ADAPTER_RS_E.unreachable, message: 'timeout' },
    'rdp',
    office.name,
  );

  const labels = buttonLabels();
  expect(labels.indexOf(adapters.errors.retry)).toBeLessThan(
    labels.indexOf(adapters.errors.otherApp),
  );
  expect(navigationRef.navigate).not.toHaveBeenCalled();
});

async function renderWith(
  error: AdapterErrorInfo,
  extra: {
    onRetryWithPassword?: () => void;
    unavailable?: boolean;
    certTimedOut?: boolean;
  },
) {
  const view = await renderWithProviders(
    <AdapterError
      error={error}
      name={office.name}
      onBack={jest.fn()}
      onCopyCode={jest.fn()}
      onOtherApp={jest.fn()}
      onRetry={jest.fn()}
      protocol="rdp"
      {...extra}
    />,
  );
  clients.push(view.queryClient);
  return view;
}

it('nlaRequired leads with retrying under an account and password (V19)', async () => {
  const onRetryWithPassword = jest.fn();
  await renderWith(
    { code: 'nlaRequired', rsCode: ADAPTER_RS_E.unsupported, message: 'nla' },
    { onRetryWithPassword },
  );

  const labels = buttonLabels();
  expect(labels.indexOf(adapters.errors.retryWithPassword)).toBeLessThan(
    labels.indexOf(adapters.errors.otherApp),
  );
  await fireEvent.press(screen.getByText(adapters.errors.retryWithPassword));
  expect(onRetryWithPassword).toHaveBeenCalledTimes(1);
});

it('a build without adapters offers only another app (V45)', async () => {
  await renderWith(
    { code: 'io', rsCode: ADAPTER_RS_E.broken, message: 'no bindings' },
    { unavailable: true },
  );

  expect(screen.getByText(adapters.errors.unavailableTitle)).toBeOnTheScreen();
  expect(
    screen.getByText(adapters.errors.unavailableMessage),
  ).toBeOnTheScreen();
  expect(screen.queryByText(adapters.errors.retry)).toBeNull();
  expect(screen.getByText(adapters.errors.otherApp)).toBeOnTheScreen();
});

it('an unanswered certificate question has its own copy (V18)', async () => {
  await renderWith(
    {
      code: 'certRejected',
      rsCode: ADAPTER_RS_E.certRejected,
      message: 'timeout',
    },
    { certTimedOut: true },
  );

  expect(screen.getByText(adapters.errors.certTimeoutTitle)).toBeOnTheScreen();
  expect(
    screen.getByText(adapters.errors.certTimeoutMessage),
  ).toBeOnTheScreen();
});
