import { cleanup, fireEvent, screen } from '@testing-library/react-native';
import type { QueryClient } from '@tanstack/react-query';
import type { RootScreenProps } from '../../app/navigation/routes';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import SessionEndedScreen from './index';

const STUDIO = 'h7v2m9q3k8x4tb1a';

type Params = RootScreenProps<'SessionEnded'>['route']['params'];

const queryClients: QueryClient[] = [];

afterEach(async () => {
  await cleanup();
  for (const client of queryClients.splice(0)) {
    client.clear();
  }
});

// V07: every board can be left; the full-screen page has no back gesture.
// The fingerprint board says 取消 (stop connecting) and FileVault says who
// goes to unlock it; both only go back.
const CASES: [string, Params, string][] = [
  ...[
    101, 110, 120, 130, 131, 132, 140, 141, 142, 160, 161, 170, 171, 180, 181,
    190, 199,
  ].map((code): [string, Params, string] => [
    `E${code}`,
    { hostId: STUDIO, code },
    '返回',
  ]),
  ['E150', { hostId: STUDIO, code: 150 }, '取消'],
  ['E191', { hostId: STUDIO, code: 191 }, '我去 Mac 前解锁'],
  ['idleTimeout', { hostId: STUDIO, reason: 'idleTimeout' }, '返回'],
  [
    'hostClosedByLocalUser',
    { hostId: STUDIO, reason: 'hostClosedByLocalUser' },
    '返回',
  ],
];

it.each(CASES)('%s has a way back', async (_name, params, label) => {
  const navigation = mockNavigation<'SessionEnded'>();
  const rendered = await renderWithProviders(
    <SessionEndedScreen
      navigation={navigation as never}
      route={mockRoute('SessionEnded', params) as never}
    />,
    { core: createFakeCore(), localDirectory: { peers: [] } },
  );
  queryClients.push(rendered.queryClient);

  await fireEvent.press(await screen.findByRole('button', { name: label }));

  expect(navigation.goBack).toHaveBeenCalledTimes(1);
  expect(navigation.navigate).not.toHaveBeenCalled();
});
