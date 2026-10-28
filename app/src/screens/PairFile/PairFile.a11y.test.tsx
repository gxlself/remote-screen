import { act, cleanup, fireEvent, screen } from '@testing-library/react-native';
import {
  AccessibilityInfo,
  PixelRatio,
  Platform,
  StyleSheet,
} from 'react-native';
import { fpDisplay } from '../../features/format/fingerprint';
import { clearPairFiles } from '../../features/pairing/pairFileCache';
import { pairFile } from '../../i18n/zh/pairFile';
import { createFakeCore } from '../../native/rscore/fakeCore';
import {
  FIXTURE_NOW,
  pairFileSummary,
  pairFileText,
} from '../../native/rscore/fixtures';
import type { PairFileSummary } from '../../native/rscore/types';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import PairFileScreen from './index';

async function renderScreen(summary = pairFileSummary) {
  const props = {
    navigation: mockNavigation<'PairFile'>(),
    route: mockRoute('PairFile'),
  };
  const view = await renderWithProviders(<PairFileScreen {...props} />, {
    core: createFakeCore({ pairFiles: { [pairFileText]: summary } }),
  });
  return { ...view, props };
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

async function read() {
  await fireEvent.changeText(
    screen.getByTestId('pair-file-input'),
    pairFileText,
  );
  await fireEvent.press(screen.getByTestId('pair-file-read'));
  await act(() => jest.advanceTimersByTimeAsync(1));
}

beforeEach(() => {
  jest.useFakeTimers({ now: FIXTURE_NOW / 1000 });
  jest.replaceProperty(Platform, 'OS', 'ios');
});

afterEach(async () => {
  await cleanup();
  clearPairFiles();
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it.each(['ios', 'android'] as const)(
  'uses the %s pairing-file instructions and an expanding header',
  async os => {
    jest.replaceProperty(Platform, 'OS', os);
    await renderScreen();
    expect(
      screen.getByText(
        os === 'android' ? pairFile.introAndroid : pairFile.intro,
      ),
    ).toBeOnTheScreen();
    expect(
      screen.getByRole('header', { name: pairFile.title }).parent,
    ).toHaveStyle({ minHeight: 52 });
    if (os === 'android') {
      expect(screen.queryByText(pairFile.intro)).toBeNull();
    }
  },
);

it.each([false, true])(
  'groups the full summary for reading, including expiry (expired=%s)',
  async expired => {
    await renderScreen({ ...pairFileSummary, expired });
    await read();
    const summary = screen.getByTestId('pair-file-summary');
    expect(summary).toHaveProp('accessible', true);
    expect(summary).toHaveProp('accessibilityLiveRegion', 'polite');
    expect(summary.props.accessibilityLabel).toBe(
      `配对文件摘要，电脑 ${pairFileSummary.hostName}，指纹 ${fpDisplay(
        pairFileSummary.fp,
      )
        .split(' ')
        .join('，')}，地址 ${pairFileSummary.addrs[0]}，10月20日 03:08 前有效${
        expired ? '，已过期' : ''
      }`,
    );
    expect(
      screen.getByRole('button', { name: pairFile.buttons.pair }),
    ).toBeOnTheScreen();
  },
);

it.each(['ios', 'android'] as const)(
  'announces a newly loaded summary once on %s using its platform mechanism',
  async os => {
    jest.replaceProperty(Platform, 'OS', os);
    const announce = jest
      .spyOn(AccessibilityInfo, 'announceForAccessibility')
      .mockImplementation(() => {});
    announce.mockClear();
    const { rerender, props } = await renderScreen();
    expect(announce).not.toHaveBeenCalled();
    await read();
    const label =
      screen.getByTestId('pair-file-summary').props.accessibilityLabel;
    await rerender(<PairFileScreen {...props} />);
    expect(announce.mock.calls.filter(([text]) => text === label)).toHaveLength(
      os === 'ios' ? 1 : 0,
    );
  },
);

it('omits an absent address from the spoken summary', async () => {
  const summary: PairFileSummary = { ...pairFileSummary, addrs: [] };
  await renderScreen(summary);
  await read();
  expect(
    screen.getByTestId('pair-file-summary').props.accessibilityLabel,
  ).not.toContain('地址');
});

it('stacks the input actions and wraps summary rows at font scale 3', async () => {
  jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
  await renderScreen();
  const actions = screen.getByTestId('pair-file-read').parent;
  expect(actions).toHaveStyle({
    flexDirection: 'column',
    alignItems: 'stretch',
  });
  expect(actions).toBe(screen.getByTestId('pair-file-paste').parent);
  await read();
  for (const testID of ['pair-file-fingerprint', 'pair-file-address']) {
    expect(screen.getByTestId(testID).parent).toHaveStyle({ flexWrap: 'wrap' });
  }
  for (const label of [
    pairFile.summary.fingerprint,
    pairFile.summary.address,
  ]) {
    expect(screen.getByText(label)).toHaveStyle({ minWidth: 36 });
    expect(
      StyleSheet.flatten(screen.getByText(label).props.style).width,
    ).toBeUndefined();
  }
});

it('keeps the header still on iOS and insets only the form scroll', async () => {
  await renderScreen();
  await read();
  expect(
    keyboardBehavior(screen.getByTestId('pair-file-screen')),
  ).toBeUndefined();
  const scroll = screen.getByTestId('pair-file-screen-scroll');
  expect(scroll).toHaveProp('automaticallyAdjustKeyboardInsets', true);
  expect(scroll).toHaveProp('keyboardDismissMode', 'interactive');
  expect(scroll).toHaveProp('keyboardShouldPersistTaps', 'handled');
  expect(scroll).toContainElement(screen.getByTestId('pair-file-input'));
  expect(scroll).toContainElement(screen.getByTestId('pair-file-pair'));
  expect(scroll).not.toContainElement(
    screen.getByRole('header', { name: pairFile.title }),
  );
});

it('uses padding keyboard avoidance on android', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  await renderScreen();
  expect(keyboardBehavior(screen.getByTestId('pair-file-screen'))).toBe(
    'padding',
  );
});
