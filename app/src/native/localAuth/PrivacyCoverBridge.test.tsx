import { render } from '@testing-library/react-native';
import { MemoryKv } from '../../features/storage/kv';
import { SettingsProvider, createSettingsStore } from '../../features/settings/store';
import PrivacyCoverBridge from './PrivacyCoverBridge';

const mockSetPrivacyCover = jest.fn<void, [boolean]>();

jest.mock('./localAuth', () => ({
  __esModule: true,
  setPrivacyCover: (enabled: boolean) => mockSetPrivacyCover(enabled),
}));

async function renderWithSettings(kv = new MemoryKv()) {
  const store = createSettingsStore(kv);
  return await render(
    <SettingsProvider store={store}>
      <PrivacyCoverBridge />
    </SettingsProvider>,
  );
}

beforeEach(() => {
  mockSetPrivacyCover.mockClear();
});

it('enables the cover when the default lock.onOpen setting is true', async () => {
  await renderWithSettings();
  expect(mockSetPrivacyCover).toHaveBeenCalledWith(true);
});

it('disables the cover when both lock settings are false', async () => {
  const kv = new MemoryKv();
  kv.setJSON('settings.lock.onOpen', false);
  kv.setJSON('settings.lock.beforeSession', false);
  await renderWithSettings(kv);
  expect(mockSetPrivacyCover).toHaveBeenCalledWith(false);
});

it('removes the cover when unmounted', async () => {
  const rendered = await renderWithSettings();
  mockSetPrivacyCover.mockClear();
  await rendered.unmount();
  expect(mockSetPrivacyCover).toHaveBeenCalledWith(false);
});
