import { getInitialRouteName } from './startup';
import { MemoryKv } from '../features/storage/kv';
import type { KvStore } from '../features/storage/kv';

describe('getInitialRouteName', () => {
  it('starts onboarding at finding computers when storage is empty', () => {
    expect(getInitialRouteName(new MemoryKv())).toBe('OnboardDiscover');
  });

  it('starts onboarding at Tailscale when it is switched on', () => {
    const kv = new MemoryKv({ 'settings.tailnet.enabled': 'true' });
    expect(getInitialRouteName(kv)).toBe('OnboardTailnet');
  });

  it('starts on the main tabs after onboarding is done', () => {
    const kv = new MemoryKv({ 'settings.onboarding.done': 'true' });
    expect(getInitialRouteName(kv)).toBe('MainTabs');
  });

  it('keeps onboarding when the persisted flag is false', () => {
    const kv = new MemoryKv({ 'settings.onboarding.done': 'false' });
    expect(getInitialRouteName(kv)).toBe('OnboardDiscover');
  });

  it('fails open to the main tabs when storage cannot be read', () => {
    const kv: KvStore = {
      getString: jest.fn(() => {
        throw new Error('storage unavailable');
      }),
      set: jest.fn(),
      delete: jest.fn(),
      getJSON: jest.fn(() => {
        throw new Error('storage unavailable');
      }),
      setJSON: jest.fn(),
      keys: jest.fn(() => []),
    };
    expect(getInitialRouteName(kv)).toBe('MainTabs');
  });
});
