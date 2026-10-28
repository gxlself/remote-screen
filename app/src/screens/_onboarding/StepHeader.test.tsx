import { screen } from '@testing-library/react-native';
import { renderWithProviders } from '../../test/render';
import StepHeader, { onboardingSteps } from './StepHeader';

it('counts two steps while Tailscale is switched off (the default)', async () => {
  expect(onboardingSteps(false)).toEqual(['discover', 'protect']);
  await renderWithProviders(<StepHeader step="discover" />);
  expect(screen.getByText('1 / 2')).toBeOnTheScreen();
  expect(screen.getByLabelText('第 1 步，共 2 步')).toBeOnTheScreen();
});

it('ends the two steps at protect', async () => {
  await renderWithProviders(<StepHeader step="protect" />);
  expect(screen.getByText('2 / 2')).toBeOnTheScreen();
});

it('counts the Tailscale step while it is switched on', async () => {
  expect(onboardingSteps(true)).toEqual(['tailnet', 'discover', 'protect']);
  await renderWithProviders(<StepHeader step="discover" />, {
    settings: { 'tailnet.enabled': true },
  });
  expect(screen.getByText('2 / 3')).toBeOnTheScreen();
});
