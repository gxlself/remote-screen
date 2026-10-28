import { act, fireEvent, screen, within } from '@testing-library/react-native';
import { adapters } from '../../i18n/zh/adapters';
import { renderWithProviders } from '../../test/render';
import { CERT_PROMPT_SECONDS, CertPrompt } from './CertPrompt';

it('shows_both_fps puts the old fingerprint before the new one', async () => {
  await renderWithProviders(
    <CertPrompt fp="aa:bb" oldFp="cc:dd" onAnswer={jest.fn()} />,
  );

  const old = screen.getByTestId('adapter-cert-old');
  const fresh = screen.getByTestId('adapter-cert-new');
  expect(within(old).getByText(adapters.cert.oldFp)).toBeOnTheScreen();
  expect(within(old).getByText('cc:dd')).toBeOnTheScreen();
  expect(within(fresh).getByText(adapters.cert.newFp)).toBeOnTheScreen();
  expect(within(fresh).getByText('aa:bb')).toBeOnTheScreen();
  const tree = JSON.stringify(screen.toJSON());
  expect(tree.indexOf('adapter-cert-old')).toBeLessThan(
    tree.indexOf('adapter-cert-new'),
  );
  expect(screen.getByText(adapters.cert.title)).toBeOnTheScreen();
  expect(screen.getByText(adapters.cert.message)).toBeOnTheScreen();
});

it('shows only the new fingerprint when there is no old one', async () => {
  await renderWithProviders(<CertPrompt fp="aa:bb" onAnswer={jest.fn()} />);

  expect(screen.getByTestId('adapter-cert-new')).toBeOnTheScreen();
  expect(screen.getByText('aa:bb')).toBeOnTheScreen();
  expect(screen.queryByTestId('adapter-cert-old')).toBeNull();
});

it('splits a long fingerprint into lines of eight bytes (V47)', async () => {
  const fp = Array.from({ length: 20 }, (_, index) =>
    index.toString(16).padStart(2, '0'),
  ).join(':');
  await renderWithProviders(<CertPrompt fp={fp} onAnswer={jest.fn()} />);

  const fresh = within(screen.getByTestId('adapter-cert-new'));
  expect(fresh.getByText('00:01:02:03:04:05:06:07')).toBeOnTheScreen();
  expect(fresh.getByText('08:09:0a:0b:0c:0d:0e:0f')).toBeOnTheScreen();
  expect(fresh.getByText('10:11:12:13')).toBeOnTheScreen();
  expect(screen.getByTestId('adapter-cert-new')).toHaveProp(
    'accessibilityLabel',
    `${adapters.cert.newFp} ${fp}`,
  );
});

it('counts down the 60 seconds the computer waits (V18)', async () => {
  jest.useFakeTimers();
  try {
    await renderWithProviders(<CertPrompt fp="aa:bb" onAnswer={jest.fn()} />);
    expect(screen.getByTestId('adapter-cert-countdown')).toHaveTextContent(
      adapters.cert.countdown(CERT_PROMPT_SECONDS),
    );

    await act(async () => {
      await jest.advanceTimersByTimeAsync(5000);
    });
    expect(screen.getByTestId('adapter-cert-countdown')).toHaveTextContent(
      adapters.cert.countdown(CERT_PROMPT_SECONDS - 5),
    );
  } finally {
    jest.useRealTimers();
  }
});

it('answers_once accepts the new certificate and ignores later answers', async () => {
  const onAnswer = jest.fn<void, [boolean]>();
  const rendered = await renderWithProviders(
    <CertPrompt fp="aa:bb" oldFp="cc:dd" onAnswer={onAnswer} />,
  );

  await fireEvent.press(screen.getByText(adapters.cert.trust));
  expect(onAnswer).toHaveBeenCalledTimes(1);
  expect(onAnswer).toHaveBeenLastCalledWith(true);

  await fireEvent.press(screen.getByText(adapters.cert.reject));
  expect(onAnswer).toHaveBeenCalledTimes(1);

  await rendered.unmount();
  await renderWithProviders(
    <CertPrompt fp="aa:bb" oldFp="cc:dd" onAnswer={onAnswer} />,
  );
  await fireEvent.press(screen.getByText(adapters.cert.reject));
  expect(onAnswer).toHaveBeenCalledTimes(2);
  expect(onAnswer).toHaveBeenLastCalledWith(false);
});

it('dismiss_rejects answers false from the Android back action', async () => {
  const onAnswer = jest.fn<void, [boolean]>();
  await renderWithProviders(<CertPrompt fp="aa:bb" onAnswer={onAnswer} />);

  await fireEvent(screen.getByTestId('adapter-cert'), 'requestClose');

  expect(onAnswer).toHaveBeenCalledTimes(1);
  expect(onAnswer).toHaveBeenCalledWith(false);
});
