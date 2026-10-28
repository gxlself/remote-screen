import { adapters } from './zh/adapters';
import { deviceRoutes } from './zh/deviceRoutes';
import { directory } from './zh/directory';
import { errBoards1 } from './zh/errBoards1';
import { errBoards3 } from './zh/errBoards3';
import { networks } from './zh/networks';
import { onboardDiscover } from './zh/onboardDiscover';
import { onboarding } from './zh/onboarding';
import { pairing } from './zh/pairing';
import { pairWaiting } from './zh/pairWaiting';
import { sessionEnded } from './zh/sessionEnded';
import { settings } from './zh/settings';

/** Copy shown instead while 外网访问 (Tailscale) is off. */
const LAN_KEY = /Lan$|^local$|^otherSubtitle$|^otherNetwork$/;
const ARGS = ['macos', 'windows', null, '10.30.12.48', '3 分钟前'];

function collect(value: unknown, path: string, lan: boolean, out: string[][]) {
  if (typeof value === 'string') {
    if (lan) {
      out.push([path, value]);
    }
  } else if (typeof value === 'function') {
    if (!lan) {
      return;
    }
    for (const arg of ARGS) {
      let result: unknown;
      try {
        result = (value as (...a: unknown[]) => unknown)(arg, '12 ms');
      } catch {
        continue;
      }
      collect(result, path, lan, out);
    }
  } else if (value !== null && typeof value === 'object') {
    for (const [key, item] of Object.entries(value)) {
      collect(item, `${path}.${key}`, lan || LAN_KEY.test(key), out);
    }
  }
}

it('never names Tailscale in the copy used while it is switched off', () => {
  const strings: string[][] = [];
  collect(
    {
      adapters,
      deviceRoutes,
      directory,
      errBoards1,
      errBoards3,
      networks,
      onboardDiscover,
      onboarding,
      pairing,
      pairWaiting,
      sessionEnded,
      settings,
    },
    'zh',
    false,
    strings,
  );
  expect(strings.length).toBeGreaterThan(15);
  expect(
    strings.filter(([, text]) => /Tailscale|tailnet|Headscale|DERP/i.test(text)),
  ).toEqual([]);
  // The onboarding body and empty line are LAN-first for everyone.
  expect(onboardDiscover.body).not.toMatch(/Tailscale/);
  expect(onboardDiscover.empty).not.toMatch(/Tailscale/);
});
