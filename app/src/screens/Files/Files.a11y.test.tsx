import {
  act,
  cleanup,
  fireEvent,
  screen,
  within,
} from '@testing-library/react-native';
import type { QueryClient } from '@tanstack/react-query';
import {
  AccessibilityInfo,
  PixelRatio,
  Platform,
  StyleSheet,
} from 'react-native';
import type { TabScreenProps } from '../../app/navigation/routes';
import {
  createFakeFiles,
  srcStudio,
  transferFixtures,
} from '../../features/files/fakeFiles';
import { FilesProvider } from '../../features/files/FilesContext';
import { transferRow } from '../../features/files/transferView';
import type { Transfer } from '../../features/files/filesApi';
import { files } from '../../i18n/zh/files';
import { filesBrowse } from '../../i18n/zh/filesBrowse';
import { FIXTURE_NOW } from '../../native/rscore/fixtures';
import { mockNavigation } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import { hit, space } from '../../theme/tokens';
import { BrowseView } from './BrowseView';
import { SourceChips } from './SourceChips';
import { TransferCard } from './TransferCard';
import { TransfersView } from './TransfersView';
import FilesScreen from './index';

const clients: QueryClient[] = [];
const names = { source: () => 'Studio Mac mini', phone: 'iPhone' };

async function tick() {
  await act(() => jest.advanceTimersByTimeAsync(1));
}

beforeEach(() => {
  jest.clearAllMocks();
  jest.useFakeTimers({ now: FIXTURE_NOW / 1000 });
  jest.replaceProperty(Platform, 'OS', 'ios');
});

afterEach(async () => {
  await cleanup();
  for (const client of clients.splice(0)) client.clear();
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it.each([
  ['active', 'running', 'pause'],
  ['active', 'paused', 'resume'],
  ['stalled', 'interrupted', 'resume'],
  ['stalled', 'failed', 'retry'],
] as const)(
  'focuses %s transfer copy without swallowing its %s action (%s)',
  async (variant, state, action) => {
    const transfer: Transfer = { ...transferFixtures[0], state };
    const row = transferRow(transfer, names);
    const onAction = jest.fn();
    const view = await renderWithProviders(
      <TransferCard row={row} variant={variant} onAction={onAction} />,
    );
    clients.push(view.queryClient);

    const copy = screen.getByLabelText(row.a11y);
    expect(copy).toHaveProp('accessible', true);
    expect(screen.getAllByLabelText(row.a11y)).toHaveLength(1);
    expect(within(copy).queryByRole('button')).toBeNull();
    const actionLabel =
      action === 'pause'
        ? files.a11y.pause(row.title)
        : action === 'resume'
        ? files.card.resume(row.title)
        : files.card.retry(row.title);
    const button = screen.getByRole('button', { name: actionLabel });
    await fireEvent.press(button);
    expect(onAction).toHaveBeenCalledTimes(1);

    if (variant === 'active') {
      const progress = screen.getByRole('progressbar', {
        name: files.card.progress(row.title),
      });
      expect(progress).toHaveProp('accessible', true);
      expect(progress).toHaveProp('accessibilityValue', {
        min: 0,
        max: 100,
        now: row.pct,
      });
      expect(progress.props.accessibilityLiveRegion).toBeUndefined();
    }
  },
);

it('marks the active transfers section as a heading', async () => {
  const fake = createFakeFiles();
  const view = await renderWithProviders(
    <FilesProvider files={fake} picker={fake}>
      <TransfersView onBrowse={jest.fn()} onMessage={jest.fn()} />
    </FilesProvider>,
  );
  clients.push(view.queryClient);
  expect(
    screen.getByRole('header', { name: files.transfers.activeTitle(2) }),
  ).toBeOnTheScreen();
});

it.each(['ios', 'android'] as const)(
  'focuses the current folder after entering and going up, but not loading or switching sources (%s)',
  async os => {
    jest.replaceProperty(Platform, 'OS', os);
    const focus = jest.spyOn(AccessibilityInfo, 'sendAccessibilityEvent');
    const fake = createFakeFiles();
    const view = await renderWithProviders(
      <FilesProvider files={fake} picker={fake}>
        <BrowseView />
      </FilesProvider>,
      { localDirectory: {} },
    );
    clients.push(view.queryClient);
    await screen.findByRole('header', { name: 'alice' });
    await tick();
    expect(focus).not.toHaveBeenCalled();
    expect(
      StyleSheet.flatten(
        screen.getByLabelText(filesBrowse.pathLabel).props.style,
      ).minHeight,
    ).toBe(hit.min);

    await fireEvent.press(
      screen.getByRole('button', { name: /^桌面，文件夹/ }),
    );
    const desktop = await screen.findByRole('header', { name: '桌面' });
    expect(desktop).toHaveProp('accessible', true);
    await tick();
    expect(focus).toHaveBeenCalledTimes(1);
    expect(focus).toHaveBeenLastCalledWith(
      expect.objectContaining({
        props: expect.objectContaining({
          accessibilityLabel: desktop.props.accessibilityLabel,
          accessibilityRole: 'header',
          accessible: true,
        }),
      }),
      'focus',
    );

    await fireEvent.press(
      screen.getByRole('button', { name: filesBrowse.a11y.up }),
    );
    const home = await screen.findByRole('header', { name: 'alice' });
    await tick();
    expect(focus).toHaveBeenCalledTimes(2);
    expect(focus).toHaveBeenLastCalledWith(
      expect.objectContaining({
        props: expect.objectContaining({
          accessibilityLabel: home.props.accessibilityLabel,
          accessibilityRole: 'header',
          accessible: true,
        }),
      }),
      'focus',
    );

    await fireEvent.press(screen.getByRole('button', { name: 'gpu-train-02' }));
    await screen.findByText('checkpoints');
    await tick();
    expect(focus).toHaveBeenCalledTimes(2);
  },
);

it.each(['ios', 'android'] as const)(
  'sizes source chips from hit.min and preserves their button selection on %s',
  async os => {
    jest.replaceProperty(Platform, 'OS', os);
    const view = await renderWithProviders(
      <SourceChips
        chips={[
          {
            key: 'studio',
            label: 'Studio Mac mini',
            source: srcStudio,
            kind: 'computer',
          },
        ]}
        selectedKey="studio"
        onSelect={jest.fn()}
      />,
    );
    clients.push(view.queryClient);
    const chip = screen.getByRole('button', { name: 'Studio Mac mini' });
    expect(chip).toHaveProp('accessibilityState', { selected: true });
    expect(StyleSheet.flatten(chip.props.style).minHeight).toBe(hit.min);
    expect(StyleSheet.flatten(chip.props.style).height).toBeUndefined();
  },
);

it('lets the file page header grow and wrap at maximum font scale', async () => {
  jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
  const fake = createFakeFiles();
  const view = await renderWithProviders(
    <FilesProvider files={fake} picker={fake}>
      <FilesScreen
        navigation={
          mockNavigation<'MainTabs'>() as unknown as TabScreenProps<'Files'>['navigation']
        }
        route={{ key: 'Files-test', name: 'Files' }}
      />
    </FilesProvider>,
  );
  clients.push(view.queryClient);
  const headerStyle = StyleSheet.flatten(
    screen.getByRole('header', { name: files.title }).parent?.props.style,
  );
  expect(headerStyle).toMatchObject({
    minHeight: 44,
    flexWrap: 'wrap',
    rowGap: space.sm,
  });
  expect(headerStyle.height).toBeUndefined();
  expect(
    screen.getByRole('button', { name: files.sendFile }),
  ).toBeOnTheScreen();
});
