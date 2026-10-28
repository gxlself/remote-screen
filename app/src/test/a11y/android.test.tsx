import { Platform } from 'react-native';
import { act, cleanup } from '@testing-library/react-native';

jest.replaceProperty(Platform, 'OS', 'android');

jest.mock('../../screens/Session/VideoSlot', () => {
  const { View: MockView } =
    jest.requireActual<typeof import('react-native')>('react-native');
  function VideoSlot(
    props: import('../../screens/Session/slots').VideoSlotProps,
  ) {
    return (
      <MockView
        accessible
        accessibilityRole="image"
        accessibilityLabel={props.accessibilityLabel}
        testID="a11y-video"
      />
    );
  }
  return { __esModule: true, default: VideoSlot, VideoSlot };
});
jest.mock('../../screens/Session/KeyboardSlot', () => {
  const { View: MockView } =
    jest.requireActual<typeof import('react-native')>('react-native');
  function KeyboardSlot() {
    return <MockView testID="a11y-keyboard" />;
  }
  return { __esModule: true, default: KeyboardSlot, KeyboardSlot };
});
jest.mock('../../screens/Terminal/TerminalSlot', () => {
  const { View: MockView } =
    jest.requireActual<typeof import('react-native')>('react-native');
  function TerminalSlot(
    props: import('../../screens/Terminal/TerminalSlot').TerminalSlotProps,
  ) {
    return (
      <MockView
        accessible
        accessibilityLabel={props.accessibilityLabel}
        testID="a11y-terminal"
      />
    );
  }
  return { TerminalSlot };
});

// These requires run after the platform override, including StyleSheet.create.
const { renderWithProviders } =
  require('../render') as typeof import('../render');
const { mockNavigation, mockRoute } =
  require('../navigation') as typeof import('../navigation');
const { mockPhoneDimensions, mockWindowDimensions } =
  require('../dimensions') as typeof import('../dimensions');
const { computers, FIXTURE_NOW } =
  require('../../native/rscore/fixtures') as typeof import('../../native/rscore/fixtures');
const { pages, sessionCases, endedCases, LOCAL_DIRECTORY } =
  require('./pages') as typeof import('./pages');
const { resetSshStore } =
  require('../../features/sessions/sshStore') as typeof import('../../features/sessions/sshStore');
const {
  controlDescription,
  expectLabels,
  swallowedControls,
  touchSize,
  visibleControls,
  withoutAllowed,
} = require('./helpers') as typeof import('./helpers');
const ALLOWED_SWALLOW: {
  page: string;
  control: string;
  container: string;
  reason: string;
}[] = [
  {
    page: 'Settings',
    control: '(无)',
    container: '光标速度 1.4× 慢：精细；快：快速划过时加速',
    reason:
      '读屏结构例外：光标速度整行提供 adjustable 的增减动作，两个视觉按钮由该元素统一读屏操作。',
  },
];

type SmallTarget = {
  page: string;
  control: string;
  dimension: 'width' | 'height';
  size: number;
  reason: string;
};
const SMALL_TARGETS_ANDROID: SmallTarget[] = [];
const usedSmall = new Set<SmallTarget>();
const usedSwallow = new Set<(typeof ALLOWED_SWALLOW)[number]>();
const names = [
  'Home narrow',
  'Home wide',
  'DeviceSheet',
  'PairAdd scan',
  'PairAdd code',
  'PairWaiting',
  'PairDone',
  'Settings',
  'AddServer',
  'AddByAddress',
  'OnboardTailnet',
  'OnboardProtect',
];
const androidCases = [
  ...pages
    .filter(page => names.includes(page.name))
    .map(page => ({
      name: page.name,
      mount: async () => {
        restoreDimensions();
        restoreDimensions = page.wide
          ? mockWindowDimensions({ width: 1024, height: 768 })
          : mockPhoneDimensions();
        // Use the same provider defaults as pages.mount, while keeping all modules Android-first.
        const options = page.options?.();
        const view = await renderWithProviders(page.ui(), {
          localDirectory: LOCAL_DIRECTORY,
          ...options,
          settings: {
            'session.coachShown': true,
            'tailnet.enabled': true,
            ...options?.settings,
          },
        });
        clients.push(view.queryClient);
        await act(() => jest.advanceTimersByTimeAsync(1));
        await view.findAllByText(page.ready);
        return view;
      },
    })),
  ...sessionCases.filter(item => item.name === 'Session Mac toolbar phone'),
  ...endedCases.filter(item =>
    ['SessionEnded E160', 'SessionEnded E171'].includes(item.name),
  ),
];
const clients: Array<{ clear(): void }> = [];
let restoreDimensions: () => void;
jest.setTimeout(30_000);
beforeEach(() => {
  jest.replaceProperty(Platform, 'OS', 'android');
  jest.useFakeTimers();
  jest.setSystemTime(FIXTURE_NOW / 1000);
  restoreDimensions = mockPhoneDimensions();
  resetSshStore();
});
afterEach(async () => {
  await cleanup();
  clients.splice(0).forEach(client => client.clear());
  // Session/ended fixtures use the shared mounting registry.
  const shared = require('./pages') as typeof import('./pages');
  await shared.cleanupA11y();
  restoreDimensions();
  jest.restoreAllMocks();
  jest.useRealTimers();
});
afterAll(() => {
  expect(SMALL_TARGETS_ANDROID.filter(item => !usedSmall.has(item))).toEqual(
    [],
  );
  expect(
    ALLOWED_SWALLOW.filter(
      item => names.includes(item.page) && !usedSwallow.has(item),
    ),
  ).toEqual([]);
});

it('loads the Android 48 dp hit token before page modules', () => {
  expect(require('../../theme/tokens').hit.min).toBe(48);
  expect(computers.length).toBeGreaterThan(0);
  expect(typeof mockNavigation).toBe('function');
  expect(typeof mockRoute).toBe('function');
});

it.each(androidCases)(
  '$name labels Android controls, keeps 48 dp targets and separates containers',
  async item => {
    const view = await item.mount();
    expectLabels(view.root);
    const findings: Omit<SmallTarget, 'reason'>[] = [];
    for (const node of visibleControls(view.root)) {
      const size = touchSize(node);
      for (const dimension of ['width', 'height'] as const) {
        const value = size[dimension];
        if (value !== undefined && value < 48) {
          findings.push({
            page: item.name,
            control: controlDescription(node),
            dimension,
            size: value,
          });
        }
      }
    }
    expect(withoutAllowed(findings, SMALL_TARGETS_ANDROID, usedSmall)).toEqual(
      [],
    );
    expect(
      withoutAllowed(
        swallowedControls(view.root).map(finding => ({
          page: item.name,
          ...finding,
        })),
        ALLOWED_SWALLOW,
        usedSwallow,
      ),
    ).toEqual([]);
  },
);
