import { AccessibilityInfo, Platform } from 'react-native';
import { act, cleanup, screen } from '@testing-library/react-native';
import type { RootScreenProps } from '../../app/navigation/routes';
import { clearPairFiles } from '../../features/pairing/pairFileCache';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { pairQrStudio } from '../../native/rscore/fixtures';
import type { CoreEvent, PairOutcome } from '../../native/rscore/types';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import PairWaitingScreen from '.';

const originalPlatform = Platform.OS;

function props(params: RootScreenProps<'PairWaiting'>['route']['params']) {
  return {
    navigation: mockNavigation<'PairWaiting'>(),
    route: mockRoute('PairWaiting', params),
  } as RootScreenProps<'PairWaiting'>;
}

async function renderPairWaiting() {
  const core = createFakeCore();
  const view = await renderWithProviders(
    <PairWaitingScreen {...props({ qrText: pairQrStudio })} />,
    { core, localDirectory: { peers: [] } },
  );
  await act(async () => {
    await jest.advanceTimersByTimeAsync(1);
  });
  return { ...view, core };
}

async function emit(core: ReturnType<typeof createFakeCore>, event: CoreEvent) {
  await act(async () => {
    core.emit(event);
  });
}

async function resolve(
  core: ReturnType<typeof createFakeCore>,
  outcome: PairOutcome,
) {
  await act(async () => {
    core.resolvePairing(outcome);
    await Promise.resolve();
  });
}

beforeEach(() => {
  jest.useFakeTimers();
  Object.defineProperty(Platform, 'OS', {
    configurable: true,
    value: 'ios',
  });
  jest.clearAllMocks();
});

afterEach(async () => {
  await cleanup();
  clearPairFiles();
  jest.useRealTimers();
  jest.restoreAllMocks();
  Object.defineProperty(Platform, 'OS', {
    configurable: true,
    value: originalPlatform,
  });
});

describe('PairWaiting accessibility', () => {
  it('marks the page and hero titles as headers and leaves permission note unlabelled', async () => {
    await renderPairWaiting();
    expect(screen.getByText('配对电脑')).toHaveProp(
      'accessibilityRole',
      'header',
    );
    expect(screen.getByText('正在和 Studio Mac mini 配对')).toHaveProp(
      'accessibilityRole',
      'header',
    );
    expect(screen.getByTestId('permission-note').props).not.toHaveProperty(
      'accessibilityLabel',
    );
  });

  it('announces local approval once on iOS and ignores countdown ticks', async () => {
    const announce = jest.spyOn(AccessibilityInfo, 'announceForAccessibility');
    const { core } = await renderPairWaiting();
    announce.mockClear();
    await emit(core, { type: 'pairing.progress', step: 'waitingLocal' });
    expect(announce).toHaveBeenCalledTimes(1);
    expect(announce).toHaveBeenCalledWith(
      '等待你在 Mac 上点「允许」，确认窗口已经弹在 Mac 屏幕正中间',
    );
    await act(async () => {
      jest.advanceTimersByTime(5000);
    });
    expect(announce).toHaveBeenCalledTimes(1);
  });

  it('uses an Android live region instead of an announcement', async () => {
    Object.defineProperty(Platform, 'OS', {
      configurable: true,
      value: 'android',
    });
    const announce = jest.spyOn(AccessibilityInfo, 'announceForAccessibility');
    const { core } = await renderPairWaiting();
    await emit(core, { type: 'pairing.progress', step: 'waitingLocal' });
    const status = screen.getByText(
      '等待你在 Mac 上点「允许」，确认窗口已经弹在 Mac 屏幕正中间',
    );
    expect(status).toHaveProp('accessibilityLiveRegion', 'polite');
    expect(announce).not.toHaveBeenCalled();
  });

  it('announces a failure title and detail', async () => {
    const announce = jest.spyOn(AccessibilityInfo, 'announceForAccessibility');
    const { core } = await renderPairWaiting();
    announce.mockClear();
    await resolve(core, { status: 'denied' });
    expect(announce).toHaveBeenCalledWith(
      expect.stringContaining('Mac 上拒绝了'),
    );
  });
});
