import type { Participant } from '../../native/rscore/types';
import {
  FIXTURE_NOW,
  SELF_DEVICE_ID,
  liveSession,
} from '../../native/rscore/fixtures';
import type { SessionRecord } from './store';
import { sharedView, shortDeviceName } from './sharedView';

const MY_USER = 'alice@';
const IPHONE_ID = 'k3jq7x2m9p4tbw8a';

const ipad: Participant = {
  deviceId: SELF_DEVICE_ID,
  deviceName: '小明的 iPad',
  user: MY_USER,
  path: 'tailnetDirect',
  since: FIXTURE_NOW,
};

const otherUserIpad: Participant = {
  ...ipad,
  deviceId: 'other-device',
  user: 'friend@example.com',
};

function record(overrides: Partial<SessionRecord> = {}): SessionRecord {
  return {
    ...liveSession,
    controller: null,
    viewers: [],
    inputPausedLocal: false,
    stats: null,
    displays: [],
    dial: [],
    approvalTimeoutS: null,
    derpRegion: null,
    reconnectReason: null,
    idleWarning: null,
    controlRequest: null,
    lastError: null,
    clipboard: [],
    ...overrides,
  };
}

describe('sharedView', () => {
  it('shows the controller for a viewer under the same user', () => {
    expect(
      sharedView({
        rec: record({ role: 'viewer', controller: ipad }),
        myUser: MY_USER,
        myDeviceId: IPHONE_ID,
      }),
    ).toEqual({
      bar: {
        kind: 'view',
        controllerName: '小明的 iPad',
        sameUser: true,
      },
      watchers: null,
    });
  });

  it('requires consent when the controller belongs to another user', () => {
    expect(
      sharedView({
        rec: record({ role: 'viewer', controller: otherUserIpad }),
        myUser: MY_USER,
        myDeviceId: IPHONE_ID,
      }).bar,
    ).toEqual({
      kind: 'view',
      controllerName: '小明的 iPad',
      sameUser: false,
    });
  });

  it('shows view-only when a viewer has no controller', () => {
    expect(
      sharedView({
        rec: record({ role: 'viewer', controller: null }),
        myUser: MY_USER,
        myDeviceId: IPHONE_ID,
      }).bar,
    ).toEqual({ kind: 'viewOnly' });
  });

  it.each([
    [[ipad], '小明的 iPad 在观看'],
    [
      [ipad, { ...ipad, deviceId: 'pixel', deviceName: '小明的 Pixel 8' }],
      '小明的 iPad 等 2 台在观看',
    ],
  ])('describes a controller with viewers', (viewers, expected) => {
    expect(
      sharedView({
        rec: record({ role: 'controller', viewers }),
        myUser: MY_USER,
        myDeviceId: IPHONE_ID,
      }).watchers,
    ).toBe(expected);
  });

  it('returns null for a single participant', () => {
    expect(
      sharedView({
        rec: record({ role: 'controller', viewers: [] }),
        myUser: MY_USER,
        myDeviceId: IPHONE_ID,
      }),
    ).toEqual({ bar: null, watchers: null });
  });
});

describe('shortDeviceName', () => {
  it('keeps the model name after the possessive marker', () => {
    expect(shortDeviceName('小明的 iPad')).toBe('iPad');
  });

  it('leaves a name without the marker unchanged', () => {
    expect(shortDeviceName('iPad')).toBe('iPad');
  });
});
