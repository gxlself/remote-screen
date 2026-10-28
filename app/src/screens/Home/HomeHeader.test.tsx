import { act, cleanup, screen } from '@testing-library/react-native';
import { MemoryKv } from '../../features/storage/kv';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { leaveHubUrl } from '../../test/localDirectory';
import { renderWithProviders } from '../../test/render';
import { HomeHeader } from './HomeHeader';

async function renderHeader(leftoverHubUrl: boolean) {
  const core = createFakeCore();
  const kv = new MemoryKv();
  if (leftoverHubUrl) {
    // A Hub address left in MMKV by an older version must not matter.
    leaveHubUrl(kv);
  }
  await renderWithProviders(
    <HomeHeader
      onAdd={jest.fn()}
      onSettings={jest.fn()}
      status={{ tone: 'good', text: '已连接' }}
    />,
    { core, kv },
  );
  await act(async () => {
    await Promise.resolve();
  });
  return core;
}

afterEach(async () => {
  await cleanup();
});

it.each([
  ['no saved Hub address', false],
  ['a leftover Hub address', true],
])('shows the plain avatar with %s', async (_label, leftoverHubUrl) => {
  const core = await renderHeader(leftoverHubUrl);

  expect(screen.getByTestId('home-settings')).toBeOnTheScreen();
  expect(screen.getByTestId('home-add')).toBeOnTheScreen();
  expect(screen.getByText('已连接')).toBeOnTheScreen();
  // The account initials came from the Hub.
  expect(screen.queryByText('XI')).toBeNull();
  expect(core.calls.filter(call => call.method.startsWith('hub.'))).toEqual([]);
});
