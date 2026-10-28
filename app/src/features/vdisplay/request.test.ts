import { vdisplay } from '../../i18n/zh/vdisplay';
import { HOST_FEATURE_VIRTUAL_DISPLAY } from '../../native/rscore/types';
import { canCreate, statusText, virtualDisplayOpts } from './request';

describe('virtualDisplayOpts', () => {
  it('uses the iPad long side as the virtual display width', () => {
    expect(
      virtualDisplayOpts({ width: 1024, height: 1366, pixelRatio: 2 }),
    ).toEqual({
      widthPx: 2732,
      heightPx: 2048,
      scale: 2,
      refreshHz: 60,
      select: true,
    });
  });

  it('uses the phone long side as the virtual display width', () => {
    expect(
      virtualDisplayOpts({ width: 393, height: 852, pixelRatio: 3 }),
    ).toEqual({
      widthPx: 2556,
      heightPx: 1179,
      scale: 2,
      refreshHz: 60,
      select: true,
    });
  });

  it('keeps scale one below a two times pixel ratio', () => {
    expect(
      virtualDisplayOpts({ width: 800, height: 1280, pixelRatio: 1.5 }),
    ).toEqual({
      widthPx: 1920,
      heightPx: 1200,
      scale: 1,
      refreshHz: 60,
      select: true,
    });
  });
});

describe('canCreate', () => {
  const valid = {
    features: HOST_FEATURE_VIRTUAL_DISPLAY,
    role: 'controller' as const,
    sessionId: 'session-1',
    ownedId: null,
  };

  it.each([
    ['missing feature', { features: 0 }],
    ['viewer', { role: 'viewer' as const }],
    ['missing session', { sessionId: null }],
    ['already owned', { ownedId: 7 }],
  ])('%s disables creation', (_name, override) => {
    expect(canCreate({ ...valid, ...override })).toBe(false);
  });

  it('accepts a controller with the feature and no owned display', () => {
    expect(canCreate(valid)).toBe(true);
    expect(canCreate({ ...valid, features: 2 ** 22 + 2 ** 19 })).toBe(true);
    expect(canCreate({ ...valid, features: 2 ** 19 })).toBe(false);
  });
});

describe('statusText', () => {
  it.each([
    ['ok', vdisplay.created],
    ['unsupported', vdisplay.error.unsupported],
    ['notAllowed', vdisplay.error.notAllowed],
    ['limit', vdisplay.error.limit],
    ['badRequest', vdisplay.error.badRequest],
    ['failed', vdisplay.error.failed],
    ['timeout', vdisplay.error.timeout],
  ] as const)('%s uses its translated message', (status, expected) => {
    expect(statusText(status)).toBe(expected);
  });
});
