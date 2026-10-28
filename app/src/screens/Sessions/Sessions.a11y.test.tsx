import { act, cleanup, screen } from '@testing-library/react-native';
import type { QueryClient } from '@tanstack/react-query';
import { PixelRatio, Platform, StyleSheet } from 'react-native';
import type { TabScreenProps } from '../../app/navigation/routes';
import { resetSshStore } from '../../features/sessions/sshStore';
import { sessions } from '../../i18n/zh/sessions';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { FIXTURE_NOW } from '../../native/rscore/fixtures';
import { mockNavigation } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import { hit } from '../../theme/tokens';
import LiveRemoteCard from './LiveRemoteCard';
import type { RemoteSessionItem } from './useSessionsModel';
import SessionsScreen from './index';

const clients: QueryClient[] = [];
const item: RemoteSessionItem = {
  id: 's-live-1',
  hostId: 'h7v2m9q3k8x4tb1a',
  name: 'Studio Mac mini',
  elapsed: '42:18',
  statusLabel: sessions.remote.status.controlling,
  statusRelay: false,
  path: null,
  latencyMs: null,
  fpsLabel: null,
  displayLabel: sessions.remote.display(1, 2),
};

async function mount() {
  const view = await renderWithProviders(
    <SessionsScreen
      navigation={
        mockNavigation<'MainTabs'>() as unknown as TabScreenProps<'Sessions'>['navigation']
      }
      route={{ key: 'Sessions-test', name: 'Sessions' }}
    />,
    { core: createFakeCore() },
  );
  clients.push(view.queryClient);
  await act(() => jest.advanceTimersByTimeAsync(1));
  return view;
}

beforeEach(() => {
  jest.clearAllMocks();
  jest.useFakeTimers({ now: FIXTURE_NOW / 1000 });
  jest.replaceProperty(Platform, 'OS', 'ios');
  resetSshStore();
});

afterEach(async () => {
  await cleanup();
  for (const client of clients.splice(0)) client.clear();
  resetSshStore();
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('describes the remote mode and display in the thumbnail hint without the clock', async () => {
  const view = await renderWithProviders(
    <LiveRemoteCard
      item={item}
      onReturn={jest.fn()}
      onDisconnect={jest.fn()}
    />,
  );
  clients.push(view.queryClient);
  const label = sessions.remote.returnA11y(item.name);
  expect(screen.getByRole('button', { name: label })).toHaveProp(
    'accessibilityHint',
    '控制中，显示器 1 / 2',
  );
  expect(screen.getByLabelText(label).props.accessibilityHint).not.toContain(
    item.elapsed,
  );
  expect(
    screen.getByText(item.elapsed).props.accessibilityLiveRegion,
  ).toBeUndefined();

  await view.rerender(
    <LiveRemoteCard
      item={{
        ...item,
        displayLabel: null,
        statusLabel: sessions.remote.status.viewing,
        elapsed: '42:19',
      }}
      onReturn={jest.fn()}
      onDisconnect={jest.fn()}
    />,
  );
  expect(screen.getByLabelText(label)).toHaveProp(
    'accessibilityHint',
    sessions.remote.status.viewing,
  );
});

it.each(['ios', 'android'] as const)(
  'uses the platform background note and the minimum disconnect target on %s',
  async os => {
    jest.replaceProperty(Platform, 'OS', os);
    await mount();
    const expected =
      os === 'android'
        ? sessions.live.backgroundNoteAndroid
        : sessions.live.backgroundNote;
    const other =
      os === 'android'
        ? sessions.live.backgroundNote
        : sessions.live.backgroundNoteAndroid;
    expect(screen.getByText(expected)).toBeOnTheScreen();
    expect(screen.queryByText(other)).toBeNull();
    const button = screen.getByRole('button', { name: sessions.disconnectAll });
    expect(StyleSheet.flatten(button.props.style)).toMatchObject({
      minHeight: hit.min,
    });
    expect(StyleSheet.flatten(button.props.style).height).toBeUndefined();
    expect(StyleSheet.flatten(button.parent?.props.style)).toMatchObject({
      minHeight: hit.min,
      flexWrap: 'wrap',
    });
  },
);

it.each([2, 3])(
  'stacks the remote identity and actions only above font scale 2 (%s)',
  async scale => {
    jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(scale);
    const view = await renderWithProviders(
      <LiveRemoteCard
        item={item}
        onReturn={jest.fn()}
        onDisconnect={jest.fn()}
      />,
    );
    clients.push(view.queryClient);
    const footer = screen.getByTestId(`remote-card-${item.id}`).children[1];
    if (typeof footer === 'string') throw new Error('missing remote footer');
    expect(StyleSheet.flatten(footer.props.style)).toMatchObject({
      flexDirection: scale > 2 ? 'column' : 'row',
      alignItems: scale > 2 ? 'stretch' : 'center',
    });
    expect(screen.getByText(item.name).props.numberOfLines).toBeUndefined();
    expect(
      screen.getByRole('button', {
        name: sessions.remote.disconnectA11y(item.name),
      }),
    ).toBeOnTheScreen();
    expect(
      screen.getByRole('button', { name: sessions.remote.returnToSession }),
    ).toBeOnTheScreen();
  },
);
