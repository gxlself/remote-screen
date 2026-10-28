import { PixelRatio, StyleSheet } from 'react-native';
import { act, cleanup, screen } from '@testing-library/react-native';
import type { RootScreenProps } from '../../app/navigation/routes';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { directory } from '../../native/rscore/fixtures';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import PairDoneScreen from '.';

const HOST_ID = 'h7v2m9q3k8x4tb1a';

function props(): RootScreenProps<'PairDone'> {
  return {
    navigation: mockNavigation<'PairDone'>(),
    route: mockRoute('PairDone', {
      hostId: HOST_ID,
      hostName: 'Studio Mac mini',
      permission: 'unattended',
      path: 'tailnetDirect',
      latencyMs: 6,
    }),
  } as RootScreenProps<'PairDone'>;
}

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
});

describe('PairDone accessibility', () => {
  it('marks the paired title as a header', async () => {
    await renderWithProviders(<PairDoneScreen {...props()} />, {
      core: createFakeCore({ directory }),
    });
    expect(screen.getByText('Studio Mac mini 已配对')).toHaveProp(
      'accessibilityRole',
      'header',
    );
  });

  it('wraps permission pills and lets them grow at a large font scale', async () => {
    jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
    await renderWithProviders(<PairDoneScreen {...props()} />, {
      core: createFakeCore({ directory }),
    });
    await act(async () => {
      await Promise.resolve();
    });
    const pill = screen.getByText('允许控制');
    const pills = pill.parent?.parent;
    expect(pills).toBeDefined();
    expect(StyleSheet.flatten(pill.parent?.props.style)).toMatchObject({
      minHeight: 28,
    });
    expect(StyleSheet.flatten(pills?.props.style)).toMatchObject({
      flexWrap: 'wrap',
    });
  });
});
