import { act, cleanup, fireEvent, screen } from '@testing-library/react-native';
import type { ReactElement } from 'react';
import {
  AccessibilityInfo,
  PixelRatio,
  Platform,
  StyleSheet,
} from 'react-native';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import { hit } from '../../theme/tokens';
import SshKeysScreen from './index';

function element(): ReactElement {
  return (
    <SshKeysScreen
      navigation={mockNavigation<'SshKeys'>()}
      route={mockRoute('SshKeys')}
    />
  );
}

async function settle(ms = 1) {
  await act(async () => {
    await jest.advanceTimersByTimeAsync(ms);
  });
}

async function renderScreen() {
  const rendered = await renderWithProviders(element());
  await settle();
  return rendered;
}

beforeEach(() => {
  jest.useFakeTimers();
});

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
  jest.useRealTimers();
});

describe('iOS', () => {
  it('exposes the page title and public key as accessible elements', async () => {
    await renderScreen();

    expect(screen.getByText('SSH 密钥')).toHaveProp(
      'accessibilityRole',
      'header',
    );
    expect(screen.getByTestId('public-key')).toHaveProp('accessible', true);
    expect(screen.getByTestId('public-key')).toHaveProp(
      'accessibilityLabel',
      '公钥',
    );
    expect(screen.getByTestId('public-key')).toHaveProp(
      'accessibilityHint',
      '用下面的「复制公钥」或「分享」拿到全文',
    );
    const publicKey = screen.getByText(/^ecdsa-/);
    expect(publicKey.type).toBe('Text');
    expect(publicKey.props.numberOfLines).toBe(3);
  });

  it('announces a successful copy exactly once', async () => {
    const announce = jest
      .spyOn(AccessibilityInfo, 'announceForAccessibility')
      .mockImplementation(() => undefined);
    announce.mockClear();
    await renderScreen();
    expect(announce).not.toHaveBeenCalled();

    await fireEvent.press(screen.getByLabelText('复制公钥'));
    await settle();

    expect(announce).toHaveBeenCalledTimes(1);
    expect(announce).toHaveBeenCalledWith('已复制');
    expect(screen.getByText('已复制')).toBeOnTheScreen();
  });
});

describe('Android', () => {
  it('uses a live region for the copy result and does not announce', async () => {
    jest.replaceProperty(Platform, 'OS', 'android');
    const announce = jest
      .spyOn(AccessibilityInfo, 'announceForAccessibility')
      .mockImplementation(() => undefined);
    announce.mockClear();
    await renderScreen();

    const liveRegion = screen.getByTestId('key-card-copy-live');
    expect(liveRegion).toHaveProp('accessibilityLiveRegion', 'polite');

    await fireEvent.press(screen.getByLabelText('复制公钥'));
    await settle();

    expect(announce).not.toHaveBeenCalled();
    expect(screen.getByText('已复制')).toBeOnTheScreen();
  });

  it('sizes the back button for Android touch targets', async () => {
    jest.replaceProperty(Platform, 'OS', 'android');
    await renderScreen();

    expect(screen.getByLabelText('返回设置')).toHaveStyle({
      minHeight: hit.min,
    });
  });
});

it('wraps the key actions at the largest font size', async () => {
  jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
  await renderScreen();

  expect(
    StyleSheet.flatten(screen.getByTestId('key-card-actions').props.style),
  ).toMatchObject({ flexWrap: 'wrap' });
});

it('leaves room for the bottom safe-area inset', async () => {
  await renderScreen();

  expect(
    StyleSheet.flatten(
      screen.getByTestId('ssh-keys-scroll').props.contentContainerStyle,
    ),
  ).toMatchObject({ paddingBottom: 40 + 34 });
});
