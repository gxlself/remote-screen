import { act, cleanup, screen } from '@testing-library/react-native';
import { MemoryKv } from '../../features/storage/kv';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { FakePlatform } from '../../native/platform/fakePlatform';
import { renderWithProviders } from '../../test/render';
import AccountHeader from './AccountHeader';

async function renderHeader(kv = new MemoryKv()) {
  const core = createFakeCore();
  const platform = new FakePlatform();
  platform.info.model = 'iPad Pro';
  await renderWithProviders(<AccountHeader />, { core, platform, kv });
  await act(async () => {
    await Promise.resolve();
  });
  return core;
}

afterEach(async () => {
  await cleanup();
});

it('names this device', async () => {
  const core = await renderHeader();

  expect(screen.getByText('我的 iPad Pro')).toBeOnTheScreen();
  expect(screen.getByLabelText('我的 iPad Pro 的账号')).toBeOnTheScreen();
  expect(core.calls.filter(call => call.method.startsWith('hub.'))).toEqual([]);
});

it('ignores a Hub address left in storage', async () => {
  const kv = new MemoryKv();
  kv.setJSON('settings.hub.url', 'http://rs-hub:7300');
  const core = await renderHeader(kv);

  expect(screen.getByText('我的 iPad Pro')).toBeOnTheScreen();
  expect(screen.queryByText('alice')).toBeNull();
  expect(screen.queryByText('XL')).toBeNull();
  expect(core.calls.filter(call => call.method.startsWith('hub.'))).toEqual([]);
});
