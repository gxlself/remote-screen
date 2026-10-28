import type { KvStore } from '../features/storage/kv';
import { createMmkvKv } from '../features/storage/mmkvKv';

const ONBOARDING_DONE_KEY = 'settings.onboarding.done';
const TAILNET_ENABLED_KEY = 'settings.tailnet.enabled';

export function getInitialRouteName(
  kv: KvStore = createMmkvKv(),
): 'OnboardTailnet' | 'OnboardDiscover' | 'MainTabs' {
  try {
    if (kv.getJSON<unknown>(ONBOARDING_DONE_KEY) === true) {
      return 'MainTabs';
    }
    // Tailscale is opt-in: without it, onboarding starts at finding computers.
    return kv.getJSON<unknown>(TAILNET_ENABLED_KEY) === true
      ? 'OnboardTailnet'
      : 'OnboardDiscover';
  } catch {
    return 'MainTabs';
  }
}
