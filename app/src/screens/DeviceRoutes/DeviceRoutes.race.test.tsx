import { cleanup, screen, waitFor } from '@testing-library/react-native';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { CoreEvent } from '../../native/rscore/types';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import DeviceRoutesScreen from './index';

const HOST_ID = 'w3k8p2x7m4q9tb6c';
const EARLY_PROBE_ID = 'early-routes-probe';

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('finishes route testing when the final result precedes probe id resolution', async () => {
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
    <DeviceRoutesScreen
      navigation={mockNavigation<'DeviceRoutes'>()}
      route={mockRoute('DeviceRoutes', { hostId: HOST_ID })}
    />,
    { core, settings: { 'tailnet.enabled': true } },
  );

  await waitFor(() => {
    expect(screen.getByText('2 个 · 刚刚测过')).toBeOnTheScreen();
  });
  expect(screen.queryByText('2 个 · 测试中…')).toBeNull();
});
