import { render, screen, waitFor } from '@testing-library/react-native';
import DevHarness from './DevHarness';
import { harnessLog } from './log';

const mockNativeLog = jest.fn();

jest.mock('../specs/NativeRsDevLog', () => ({
  __esModule: true,
  default: { log: (...args: unknown[]) => mockNativeLog(...args) },
}));

beforeEach(() => {
  mockNativeLog.mockClear();
});

it('logs harness readiness before the ping harness result', async () => {
  await render(<DevHarness name="ping" arg="x" />);

  await waitFor(() => {
    expect(mockNativeLog).toHaveBeenNthCalledWith(
      1,
      'RS-HARNESS {"name":"ping","ready":true}',
    );
    expect(mockNativeLog).toHaveBeenNthCalledWith(
      2,
      'RS-PING {"ok":true,"arg":"x"}',
    );
  });
});

it('reports unknown harnesses', async () => {
  await render(<DevHarness name="nope" />);

  expect(await screen.findByText('未知的 harness：nope')).toBeOnTheScreen();
  expect(mockNativeLog).toHaveBeenCalledWith(
    'RS-HARNESS {"name":"nope","error":"unknown"}',
  );
});

it('normalizes invalid tags and tolerates a missing native module', () => {
  harnessLog('bad tag', {});
  expect(mockNativeLog).toHaveBeenCalledWith('RS-INVALID {}');
});
