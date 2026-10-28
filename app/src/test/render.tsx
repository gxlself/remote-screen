import type { ReactElement, ReactNode } from 'react';
import { render } from '@testing-library/react-native';
import type { RenderResult } from '@testing-library/react-native';
import { SafeAreaProvider } from 'react-native-safe-area-context';
import { createFakeCore } from '../native/rscore/fakeCore';
import type { FakeCore } from '../native/rscore/fakeCore';
import { FakePlatform } from '../native/platform/fakePlatform';
import { MemoryKv } from '../features/storage/kv';
import type { KvStore } from '../features/storage/kv';
import { createSettingsStore } from '../features/settings/store';
import type { SettingsKey, SettingValue } from '../features/settings/schema';
import { createQueryClient } from '../features/data/queryClient';
import { ServicesProviders } from '../features/data/ServicesProviders';
import EventsProvider from '../features/events/EventsProvider';
import { seedLocalDirectory } from './localDirectory';
import type { LocalDirectorySeed } from './localDirectory';

export async function renderWithProviders(
  ui: ReactElement,
  opts?: {
    core?: FakeCore;
    platform?: FakePlatform;
    kv?: KvStore;
    settings?: Partial<{ [K in SettingsKey]: SettingValue<K> }>;
    /** Computers for screens that read the directory; `{}` is the fixtures. */
    localDirectory?: LocalDirectorySeed;
  },
) {
  const core = opts?.core ?? createFakeCore();
  const platform = opts?.platform ?? new FakePlatform();
  const kv = opts?.kv ?? new MemoryKv();
  if (opts?.localDirectory !== undefined) {
    seedLocalDirectory(core, kv, opts.localDirectory);
  }
  for (const [key, value] of Object.entries(opts?.settings ?? {})) {
    if (value !== undefined) {
      kv.setJSON(`settings.${key}`, value);
    }
  }
  const settingsStore = createSettingsStore(kv, core);
  const queryClient = createQueryClient({ test: true });
  const services = { core, platform, kv, settingsStore, queryClient };
  function Wrapper({ children }: { children: ReactNode }) {
    return (
      <SafeAreaProvider
        initialMetrics={{
          frame: { x: 0, y: 0, width: 390, height: 844 },
          insets: { top: 47, bottom: 34, left: 0, right: 0 },
        }}
      >
        <ServicesProviders {...services}>
          <EventsProvider>{children}</EventsProvider>
        </ServicesProviders>
      </SafeAreaProvider>
    );
  }
  const result: RenderResult = await render(ui, { wrapper: Wrapper });
  return Object.assign(result, services, { wrapper: Wrapper });
}
