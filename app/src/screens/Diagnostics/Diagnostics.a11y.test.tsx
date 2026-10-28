import { act, cleanup, fireEvent, screen } from '@testing-library/react-native';
import type { RootScreenProps } from '../../app/navigation/routes';
import type { NetworkEntry } from '../../features/settings/schema';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { FakeCore } from '../../native/rscore/fakeCore';
import {
  computers,
  FIXTURE_NOW,
  netCompanyVpn,
} from '../../native/rscore/fixtures';
import type { CoreEvent, ProbeOutcome } from '../../native/rscore/types';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import {
  AccessibilityInfo,
  PixelRatio,
  Platform,
  StyleSheet,
} from 'react-native';
import { hit } from '../../theme/tokens';
import DiagnosticsScreen from '.';

jest.setTimeout(30_000);

const OFFICE_HOST_ID = 'w3k8p2x7m4q9tb6c';
const OFFICE_ADDR = '100.64.0.12';
const INTRANET_ADDR = '10.30.12.48';
const ADVICE =
  '会走内网 TCP 7400，23 ms 最快。公司网络拦了 UDP，所以是「TCP 模式」。';

const COMPANY_NETWORK: NetworkEntry = {
  id: 'company',
  name: '公司网络',
  cidrs: ['10.30.0.0/16'],
  vpnPools: ['10.99.0.0/16'],
};

function props(hostId?: string) {
  return {
    navigation: mockNavigation<'Diagnostics'>(),
    route: mockRoute(
      'Diagnostics',
      hostId === undefined ? undefined : { hostId },
    ),
  } as RootScreenProps<'Diagnostics'>;
}

function probe(
  core: FakeCore,
  probeId: string,
  event: Omit<CoreEvent & { type: 'probe.result' }, 'type' | 'probeId'>,
) {
  core.emit({
    type: 'probe.result',
    probeId,
    ...event,
  } as CoreEvent);
}

type CoreEventOfOutcome = ProbeOutcome;

function result(
  path: 'tailnetDirect' | 'tailnetDerp' | 'intranet',
  transport: 'quic' | 'tcp',
  addr: string,
  outcome: CoreEventOfOutcome,
  ms: number | null,
  reason?: string,
): Omit<CoreEvent & { type: 'probe.result' }, 'type' | 'probeId'> {
  return {
    hostId: OFFICE_HOST_ID,
    path,
    addr,
    port: 7400,
    transport,
    outcome,
    ms,
    reason,
    last: false,
  };
}

async function emitFour(core: FakeCore, probeId = 'p1') {
  const events = [
    result('tailnetDirect', 'quic', OFFICE_ADDR, 'timeout', 3000, '打洞失败'),
    result('tailnetDerp', 'quic', OFFICE_ADDR, 'ok', 48),
    result('intranet', 'quic', INTRANET_ADDR, 'refused', null),
    result('intranet', 'tcp', INTRANET_ADDR, 'ok', 23),
  ];
  for (const [index, event] of events.entries()) {
    await act(async () => {
      probe(core, probeId, {
        ...event,
        last: index === events.length - 1,
      });
    });
  }
}

async function renderScreen(hostId = OFFICE_HOST_ID) {
  const screenProps = props(hostId);
  const core = createFakeCore();
  const rendered = await renderWithProviders(
    <DiagnosticsScreen {...screenProps} />,
    {
      core,
      settings: { networks: [COMPANY_NETWORK] },
    },
  );
  await act(async () => {});
  await act(async () => {
    core.emit({ type: 'network.changed', status: netCompanyVpn });
  });
  return { ...rendered, core };
}

beforeEach(() => {
  jest.useFakeTimers();
  jest.setSystemTime(FIXTURE_NOW / 1000);
  jest.spyOn(Date.prototype, 'getTimezoneOffset').mockReturnValue(-480);
});

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
  jest.useRealTimers();
});

describe('iOS', () => {
  it('announces the finished advice once but not on first mount', async () => {
    const announce = jest
      .spyOn(AccessibilityInfo, 'announceForAccessibility')
      .mockImplementation(() => undefined);
    announce.mockClear();
    const { core } = await renderScreen();
    expect(announce).not.toHaveBeenCalled();

    await emitFour(core);

    expect(screen.getByText(ADVICE)).toBeOnTheScreen();
    expect(announce).toHaveBeenCalledTimes(1);
    expect(announce).toHaveBeenCalledWith(ADVICE);
  });

  it('describes the selected computer on the change action and picker rows', async () => {
    const studio = computers.find(
      computer => computer.name === 'Studio Mac mini',
    );
    const offline = computers.find(computer => !computer.online);
    expect(studio).toBeDefined();
    expect(offline).toBeDefined();
    await renderScreen(studio!.hostId);

    expect(screen.getByLabelText('换一台')).toHaveProp(
      'accessibilityHint',
      `现在测的是 ${studio!.name}`,
    );

    await fireEvent.press(screen.getByLabelText('换一台'));
    expect(screen.getByTestId(`host-picker-${studio!.hostId}`)).toHaveProp(
      'accessibilityHint',
      expect.stringMatching(/，当前选中$/),
    );
    expect(screen.getByTestId(`host-picker-${offline!.hostId}`)).toHaveProp(
      'accessibilityHint',
      '离线',
    );
  });

  it('leaves room for the status bar and bottom inset', async () => {
    await renderScreen();

    expect(
      StyleSheet.flatten(screen.getByTestId('diagnostics-screen').props.style),
    ).toMatchObject({ paddingTop: 47 });
    expect(
      StyleSheet.flatten(
        screen.getByTestId('diagnostics').props.contentContainerStyle,
      ),
    ).toMatchObject({ paddingBottom: 24 + 34 });
  });
});

describe('Android', () => {
  it('uses the live region instead of announcing the advice', async () => {
    jest.replaceProperty(Platform, 'OS', 'android');
    const announce = jest
      .spyOn(AccessibilityInfo, 'announceForAccessibility')
      .mockImplementation(() => undefined);
    announce.mockClear();
    const { core } = await renderScreen();
    expect(Platform.OS).toBe('android');

    await emitFour(core);

    expect(announce).not.toHaveBeenCalled();
    expect(screen.getByText(ADVICE)).toHaveProp(
      'accessibilityLiveRegion',
      'polite',
    );
  });

  it('sizes the back button for Android touch targets', async () => {
    jest.replaceProperty(Platform, 'OS', 'android');
    await renderScreen();

    expect(screen.getByLabelText('返回')).toHaveStyle({ minHeight: hit.min });
  });
});

it('wraps the diagnostics actions at the largest font size', async () => {
  jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
  await renderScreen();

  expect(
    StyleSheet.flatten(screen.getByTestId('diagnostics-actions').props.style),
  ).toMatchObject({ flexWrap: 'wrap' });
});
