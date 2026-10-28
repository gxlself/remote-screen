import {
  cleanup,
  fireEvent,
  screen,
  waitFor,
} from '@testing-library/react-native';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { CoreEvent } from '../../native/rscore/types';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import DiagnosticsScreen from '.';

const HOST_ID = 'w3k8p2x7m4q9tb6c';
const EARLY_PROBE_ID = 'early-diagnostics-probe';

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('replays a final probe result emitted before probe resolves its id', async () => {
  const core = createFakeCore();
  jest.spyOn(core.diag, 'probe').mockImplementation(() => {
    return Promise.resolve().then(() => {
      core.emit({
        type: 'probe.result',
        probeId: EARLY_PROBE_ID,
        hostId: HOST_ID,
        path: 'intranet',
        addr: '10.30.12.48',
        port: 7400,
        transport: 'quic',
        outcome: 'ok',
        ms: 23,
        last: true,
      } satisfies CoreEvent);
      return EARLY_PROBE_ID;
    });
  });

  await renderWithProviders(
    <DiagnosticsScreen
      navigation={mockNavigation<'Diagnostics'>()}
      route={mockRoute('Diagnostics', { hostId: HOST_ID })}
    />,
    { core },
  );

  await waitFor(() => {
    expect(screen.getByText('23 ms')).toBeOnTheScreen();
  });
  expect(screen.getByText('重新测试')).toBeOnTheScreen();
  expect(screen.queryByText('正在测，约 10 秒')).toBeNull();
});

it('clears early events between probe runs', async () => {
  const core = createFakeCore();
  const reusedProbeId = 'reused-diagnostics-probe';
  let calls = 0;
  jest.spyOn(core.diag, 'probe').mockImplementation(() => {
    calls += 1;
    if (calls === 1) {
      return new Promise<string>(() => {
        queueMicrotask(() => {
          core.emit({
            type: 'probe.result',
            probeId: reusedProbeId,
            hostId: HOST_ID,
            path: 'intranet',
            addr: '10.30.12.48',
            port: 7400,
            transport: 'quic',
            outcome: 'ok',
            ms: 23,
            last: true,
          } satisfies CoreEvent);
        });
      });
    }
    return Promise.resolve(reusedProbeId);
  });

  await renderWithProviders(
    <DiagnosticsScreen
      navigation={mockNavigation<'Diagnostics'>()}
      route={mockRoute('Diagnostics', { hostId: HOST_ID })}
    />,
    { core },
  );
  await waitFor(() => {
    expect(screen.getByText('停止')).toBeOnTheScreen();
  });

  await fireEvent.press(screen.getByText('停止'));
  await waitFor(() => {
    expect(screen.getByText('重新测试')).toBeOnTheScreen();
  });
  await fireEvent.press(screen.getByText('重新测试'));

  await waitFor(() => {
    expect(screen.getByText('正在测，约 10 秒')).toBeOnTheScreen();
  });
  expect(screen.queryByText('23 ms')).toBeNull();
});
