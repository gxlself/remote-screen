import { act, fireEvent, screen, waitFor } from '@testing-library/react-native';
import type { QueryClient } from '@tanstack/react-query';
import { AccessibilityInfo, Platform } from 'react-native';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { MemoryKv } from '../../features/storage/kv';
import { failLocalSync } from '../../test/localDirectory';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import { hit } from '../../theme/tokens';
import NetworksScreen from './index';

const queryClients: QueryClient[] = [];

async function renderScreen(platform: 'ios' | 'android' = 'ios') {
  jest.replaceProperty(Platform, 'OS', platform);
  const core = createFakeCore();
  const navigation = mockNavigation<'Networks'>();
  const result = await renderWithProviders(
    <NetworksScreen navigation={navigation} route={mockRoute('Networks')} />,
    { core, kv: new MemoryKv(), localDirectory: {} },
  );
  queryClients.push(result.queryClient);
  await waitFor(() =>
    expect(screen.getByText('还没有登记网络')).toBeOnTheScreen(),
  );
  await act(() => Promise.resolve());
  return { ...result, core, navigation };
}

afterEach(async () => {
  await act(() => Promise.resolve());
  jest.restoreAllMocks();
  for (const client of queryClients.splice(0)) {
    client.clear();
  }
});

beforeEach(() => {
  jest.clearAllMocks();
});

it.each(['ios', 'android'] as const)(
  'gives the %s back button the platform hit target',
  async platform => {
    await renderScreen(platform);
    expect(screen.getByLabelText('返回设置')).toHaveStyle({
      minHeight: hit.min,
    });
  },
);

it.each(['ios', 'android'] as const)(
  'announces the first validation error on %s',
  async platform => {
    await renderScreen(platform);
    await fireEvent.press(screen.getByLabelText('添加其他网络'));
    const announce = jest.spyOn(AccessibilityInfo, 'announceForAccessibility');

    await fireEvent.press(screen.getByLabelText('保存'));
    expect(announce).toHaveBeenCalledWith('名称 必填');
  },
);

it('announces a save failure on iOS', async () => {
  const { kv } = await renderScreen('ios');
  failLocalSync(kv, 'networks', new Error('网络断了'));
  await fireEvent.press(screen.getByLabelText('添加其他网络'));
  await fireEvent.changeText(screen.getByTestId('network-name'), '公司网络');
  await fireEvent.changeText(
    screen.getByTestId('network-cidrs'),
    '10.30.0.0/16',
  );
  const announce = jest.spyOn(AccessibilityInfo, 'announceForAccessibility');

  await fireEvent.press(screen.getByLabelText('保存'));
  await waitFor(() =>
    expect(screen.getByText('没保存上：网络断了')).toBeOnTheScreen(),
  );
  expect(announce).toHaveBeenCalledWith('没保存上：网络断了');
});
