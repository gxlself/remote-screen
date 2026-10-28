import { act } from '@testing-library/react-native';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { renderWithProviders } from '../../test/render';
import IdentityBootstrap from './IdentityBootstrap';

it('creates the device identity at launch', async () => {
  const core = createFakeCore();
  const providers = await renderWithProviders(<IdentityBootstrap />, { core });
  await act(async () => undefined);
  expect(
    core.calls.filter(call => call.method === 'identity.ensure'),
  ).toHaveLength(1);
  await providers.unmount();
  providers.queryClient.clear();
});
