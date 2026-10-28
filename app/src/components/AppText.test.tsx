import { render, screen } from '@testing-library/react-native';
import { Platform, StyleSheet } from 'react-native';
import { renderWithProviders } from '../test/render';
import AppText from './AppText';

describe('AppText', () => {
  it('renders text with the default body style', async () => {
    await render(<AppText>设备</AppText>);
    expect(screen.getByText('设备')).toBeOnTheScreen();
  });

  it('uses a token color', async () => {
    await render(<AppText color="text2">设备</AppText>);
    expect(
      StyleSheet.flatten(screen.getByText('设备').props.style),
    ).toMatchObject({
      color: '#A9B4BB',
    });
  });

  it('leaves numberOfLines unset by default', async () => {
    await render(<AppText>设备</AppText>);
    expect(screen.getByText('设备').props.numberOfLines).toBeUndefined();
  });

  it('switches to the mono family', async () => {
    await render(<AppText mono>6 ms</AppText>);
    const style = StyleSheet.flatten(screen.getByText('6 ms').props.style);
    expect(style.fontFamily).toMatch(/^GeistMono-/);
  });

  it('applies caller styles last', async () => {
    await render(
      <AppText style={{ fontSize: 30, color: '#FFFFFF' }}>设备</AppText>,
    );
    expect(
      StyleSheet.flatten(screen.getByText('设备').props.style),
    ).toMatchObject({
      fontSize: 30,
      color: '#FFFFFF',
    });
  });

  it('forwards Text props and explicit truncation', async () => {
    await render(
      <AppText numberOfLines={1} allowFontScaling={false} testID="label">
        设备
      </AppText>,
    );
    const text = screen.getByTestId('label');
    expect(text.props.numberOfLines).toBe(1);
    expect(text.props.allowFontScaling).toBe(false);
  });
});

describe('AppText platform font padding', () => {
  afterEach(() => jest.restoreAllMocks());

  it('removes Android font padding after caller styles', async () => {
    jest.replaceProperty(Platform, 'OS', 'android');
    await renderWithProviders(
      <AppText style={{ includeFontPadding: true, fontSize: 30 }}>
        设备
      </AppText>,
    );
    expect(
      StyleSheet.flatten(screen.getByText('设备').props.style),
    ).toMatchObject({
      includeFontPadding: false,
      fontSize: 30,
    });
  });

  it('leaves iOS font padding unchanged and reads platform when rendering', async () => {
    jest.replaceProperty(Platform, 'OS', 'ios');
    const { rerender } = await renderWithProviders(<AppText>设备</AppText>);
    expect(
      StyleSheet.flatten(screen.getByText('设备').props.style)
        .includeFontPadding,
    ).toBeUndefined();

    jest.replaceProperty(Platform, 'OS', 'android');
    await rerender(<AppText>设备</AppText>);
    expect(
      StyleSheet.flatten(screen.getByText('设备').props.style)
        .includeFontPadding,
    ).toBe(false);
  });
});
