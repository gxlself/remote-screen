import {
  cleanup,
  fireEvent,
  screen,
  waitFor,
} from '@testing-library/react-native';
import { installFakeAdapters } from '../../features/adapters/testing/fakeAdapters';
import { adapters } from '../../i18n/zh/adapters';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { computers } from '../../native/rscore/fixtures';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import DeviceSheetScreen from './index';

const HOST_ID = 'h7v2m9q3k8x4tb1a';
const WINDOWS_HOST_ID = 'w3k8p2x7m4q9tb6c';

async function renderSheet(withAdapters: boolean, hostId = HOST_ID) {
  const core = createFakeCore();
  if (withAdapters) {
    installFakeAdapters(core);
  }
  const navigation = mockNavigation<'DeviceSheet'>();
  const rendered = await renderWithProviders(
    <DeviceSheetScreen
      navigation={navigation}
      route={mockRoute('DeviceSheet', { hostId })}
    />,
    { core, localDirectory: { peers: [] } },
  );
  return { ...rendered, navigation };
}

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
});

it('adds the built-in adapter row after quick actions', async () => {
  const rendered = await renderSheet(true);
  await screen.findByText('Studio Mac mini');
  const quickActions = await screen.findByTestId('quick-actions');
  const adapter = await screen.findByTestId('adapter-entry');
  expect(quickActions).toBeOnTheScreen();
  expect(adapter).toBeOnTheScreen();

  await fireEvent.press(adapter);
  expect(rendered.navigation.replace).toHaveBeenCalledWith('AdapterSession', {
    target: { protocol: 'vnc', hostId: HOST_ID, addr: '100.64.0.7' },
    name: 'Studio Mac mini',
    addrKind: 'tailnet',
    context: 'manual',
  });
});

it('does not add the row without adapters', async () => {
  const rendered = await renderSheet(false);
  await screen.findByText('Studio Mac mini');
  await waitFor(() => {
    expect(screen.queryByTestId('adapter-entry')).toBeNull();
  });
  expect(rendered.navigation.replace).not.toHaveBeenCalledWith(
    'AdapterSession',
    expect.anything(),
  );
});

it('adds the built-in remote desktop row for a Windows computer', async () => {
  const office = computers.find(
    computer => computer.hostId === WINDOWS_HOST_ID,
  );
  if (office === undefined) {
    throw new Error('fixture host missing');
  }
  const rendered = await renderSheet(true, WINDOWS_HOST_ID);
  await screen.findByText(office.name);
  const adapter = await screen.findByTestId('adapter-entry');
  expect(screen.getByText(adapters.entry.rdp)).toBeOnTheScreen();

  await fireEvent.press(adapter);
  expect(rendered.navigation.replace).toHaveBeenCalledWith('AdapterSession', {
    target: { protocol: 'rdp', hostId: WINDOWS_HOST_ID, addr: '100.64.0.12' },
    name: office.name,
    addrKind: 'tailnet',
    context: 'manual',
  });
});
