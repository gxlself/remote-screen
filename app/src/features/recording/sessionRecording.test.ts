import { RsCoreError } from '../../native/rscore/RsCoreApi';
import type { Computer } from '../../native/rscore/types';
import { recording } from '../../i18n/zh/recording';
import {
  applyRecordingEvent,
  confirmKind,
  formatElapsed,
  getRecordingUi,
  resetRecordingUiForTests,
  startErrorText,
  stopReasonText,
} from './sessionRecording';

// eslint-disable-next-line no-bitwise
const TEST_BIT = 1 << 30;

function host(features: number): Computer {
  return {
    hostId: 'h1',
    name: 'Host',
    node: 'host',
    os: 'macos',
    online: true,
    hostRunning: true,
    lastSeenAt: 0,
    lastHeartbeatAt: null,
    hostVersion: '1.0.0',
    features,
    fp: 'fp',
    addresses: [],
    displays: [],
    permsOk: true,
    sessions: [],
    paired: true,
    update: { channel: 'stable', available: null },
    thumbnailUrl: '',
    filevault: false,
    console: null,
  };
}

beforeEach(() => {
  resetRecordingUiForTests();
});

it('selects the privacy confirmation copy', () => {
  expect(
    confirmKind({
      introShown: true,
      host: host(0),
      bit: TEST_BIT,
    }),
  ).toBe('hostWontKnow');
  expect(
    confirmKind({
      introShown: false,
      host: host(TEST_BIT),
      bit: TEST_BIT,
    }),
  ).toBe('intro');
  expect(
    confirmKind({
      introShown: true,
      host: host(TEST_BIT),
      bit: TEST_BIT,
    }),
  ).toBeNull();
  expect(
    confirmKind({
      introShown: true,
      host: host(0),
      policy: 'B',
      bit: TEST_BIT,
    }),
  ).toBeNull();
  expect(
    confirmKind({ introShown: true, host: undefined, bit: TEST_BIT }),
  ).toBe('hostWontKnow');
  expect(confirmKind({ introShown: true, host: host(1), bit: null })).toBe(
    'hostWontKnow',
  );
});

it('formats elapsed time', () => {
  expect(formatElapsed(0)).toBe('0:00');
  expect(formatElapsed(61_000)).toBe('1:01');
  expect(formatElapsed(3_600_000)).toBe('1:00:00');
});

it('maps start errors and stop reasons to copy', () => {
  expect(
    startErrorText(new RsCoreError('http', 'recording.diskLow', 507)),
  ).toBe(recording.errDiskLow);
  expect(
    startErrorText(new RsCoreError('unsupported', 'recording.unsupported')),
  ).toBe(recording.errUnsupported);
  expect(startErrorText(new Error('x'))).toBe(recording.errGeneric);
  expect(stopReasonText('user')).toBeNull();
  expect(stopReasonText('maxDuration')).toBe(recording.stopMaxDuration);
  expect(stopReasonText('unknown')).toBe(recording.stopError);
});

it('applies recording state events to the shared state', () => {
  expect(
    applyRecordingEvent({
      type: 'recording.state',
      sessionId: 's1',
      recordingId: 'r1',
      state: 'recording',
      durationMs: 65_000,
      bytes: 42,
    }),
  ).toBeNull();
  expect(getRecordingUi()).toMatchObject({
    sessionId: 's1',
    state: 'recording',
    durationMs: 65_000,
    bytes: 42,
  });
  expect(
    applyRecordingEvent({
      type: 'recording.state',
      sessionId: 's1',
      recordingId: 'r1',
      state: 'stopped',
      durationMs: 65_000,
      bytes: 42,
      reason: 'user',
    }),
  ).toBe('stopped');
  expect(getRecordingUi().state).toBe('idle');
  expect(
    applyRecordingEvent({
      type: 'recording.state',
      sessionId: 's1',
      recordingId: 'r1',
      state: 'failed',
      durationMs: 1,
      bytes: 2,
    }),
  ).toBe('failed');
  expect(getRecordingUi().state).toBe('idle');
});
