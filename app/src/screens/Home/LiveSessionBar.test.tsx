import { fireEvent, screen } from '@testing-library/react-native';
import { liveSession } from '../../native/rscore/fixtures';
import { renderWithProviders } from '../../test/render';
import { LiveSessionBar } from './LiveSessionBar';
import type { LiveBarSession } from './LiveSessionBar';

const adapter = {
  id: 'adapter-1',
  hostId: 'adapter:adapter-1',
  hostName: '家里台式机',
  startedAt: Date.now() * 1000,
  path: null,
  adapter: {
    phase: 'streaming',
    params: { target: { protocol: 'rdp', addr: '192.168.1.20' } },
  },
} satisfies LiveBarSession;

it('shows a Host session as before', async () => {
  await renderWithProviders(
    <LiveSessionBar bottom={0} onPress={jest.fn()} session={liveSession} />,
  );
  expect(screen.getByText('Studio Mac mini')).toBeOnTheScreen();
  expect(screen.getByText('控制中')).toBeOnTheScreen();
});

it('shows a folded-away screen sharing session and returns to it (V33)', async () => {
  const onPress = jest.fn();
  await renderWithProviders(
    <LiveSessionBar bottom={0} onPress={onPress} session={adapter} />,
  );
  expect(screen.getByText('家里台式机')).toBeOnTheScreen();
  expect(screen.getByText('控制中')).toBeOnTheScreen();

  await fireEvent.press(screen.getByText('回到会话'));
  expect(onPress).toHaveBeenCalledTimes(1);
});

it('says when the folded-away session dropped', async () => {
  await renderWithProviders(
    <LiveSessionBar
      bottom={0}
      onPress={jest.fn()}
      session={{ ...adapter, adapter: { ...adapter.adapter, phase: 'ended' } }}
    />,
  );
  expect(screen.getByText('已断开')).toBeOnTheScreen();
});
