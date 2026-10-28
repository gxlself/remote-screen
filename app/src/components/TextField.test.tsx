import { fireEvent, render, screen } from '@testing-library/react-native';
import { createRef } from 'react';
import {
  AccessibilityInfo,
  PixelRatio,
  Platform,
  StyleSheet,
} from 'react-native';
import { renderWithProviders } from '../test/render';
import { TextField, type TextFieldHandle } from './TextField';

function inputStyle(testID = 'input') {
  return StyleSheet.flatten(screen.getByTestId(testID).props.style);
}

describe('TextField', () => {
  it('reports typed text through onChangeText', async () => {
    const onChangeText = jest.fn();
    await render(
      <TextField
        value=""
        onChangeText={onChangeText}
        placeholder="10.30.1.20"
        testID="input"
      />,
    );
    await fireEvent.changeText(screen.getByTestId('input'), '10.30.1.20');
    expect(onChangeText).toHaveBeenCalledWith('10.30.1.20');
  });

  it('shows the label above the field and uses it as the a11y label', async () => {
    await render(
      <TextField
        label="地址"
        value=""
        onChangeText={() => {}}
        testID="input"
      />,
    );
    expect(screen.getByText('地址')).toBeOnTheScreen();
    expect(
      StyleSheet.flatten(screen.getByText('地址').props.style),
    ).toMatchObject({ color: '#A9B4BB', fontSize: 13 });
    expect(screen.getByLabelText('地址')).toBeOnTheScreen();
  });

  it('lets callers override the accessibility label', async () => {
    await render(
      <TextField
        label="地址"
        value=""
        onChangeText={() => {}}
        accessibilityLabel="服务器地址"
        testID="input"
      />,
    );
    expect(screen.getByLabelText('服务器地址')).toBeOnTheScreen();
  });

  it('draws the AddServer canvas geometry and colors', async () => {
    await render(
      <TextField
        label="地址"
        value=""
        onChangeText={() => {}}
        testID="input"
      />,
    );
    expect(inputStyle()).toMatchObject({
      height: 46,
      borderRadius: 12,
      paddingHorizontal: 12,
      backgroundColor: '#151A1E',
      // tokens.inputBorder: at least 3:1 on the card and the canvas.
      borderColor: '#64707A',
      borderWidth: 1,
      color: '#ECF0F2',
      fontSize: 15,
      fontFamily: 'Geist-Regular',
      textAlign: 'left',
    });
  });

  it('thickens the border in the accent color while focused', async () => {
    await render(<TextField value="" onChangeText={() => {}} testID="input" />);
    await fireEvent(screen.getByTestId('input'), 'focus');
    expect(inputStyle()).toMatchObject({
      borderColor: '#7DA8FF',
      borderWidth: 2,
    });

    await fireEvent(screen.getByTestId('input'), 'blur');
    expect(inputStyle()).toMatchObject({
      borderColor: '#64707A',
      borderWidth: 1,
    });
  });

  it('shows the error text, a bad border and the error hint', async () => {
    await render(
      <TextField
        label="地址"
        value="x"
        onChangeText={() => {}}
        error="格式不对"
        testID="input"
      />,
    );
    expect(screen.getByText('格式不对')).toBeOnTheScreen();
    expect(
      StyleSheet.flatten(screen.getByText('格式不对').props.style),
    ).toMatchObject({ color: '#FF8873', fontSize: 12, lineHeight: 17 });
    expect(inputStyle()).toMatchObject({
      borderColor: '#FF8873',
      borderWidth: 1,
    });
    expect(screen.getByLabelText('地址')).toHaveProp(
      'accessibilityHint',
      '格式不对',
    );
  });

  it('shows the hint when there is no error', async () => {
    await render(
      <TextField
        value=""
        onChangeText={() => {}}
        hint="支持域名或 IP"
        testID="input"
      />,
    );
    expect(screen.getByText('支持域名或 IP')).toBeOnTheScreen();
    expect(
      StyleSheet.flatten(screen.getByText('支持域名或 IP').props.style),
    ).toMatchObject({ color: '#86929A', fontSize: 12 });
  });

  it('hides the hint while an error is present', async () => {
    await render(
      <TextField
        value=""
        onChangeText={() => {}}
        hint="支持域名或 IP"
        error="格式不对"
        testID="input"
      />,
    );
    expect(screen.queryByText('支持域名或 IP')).toBeNull();
    expect(screen.getByText('格式不对')).toBeOnTheScreen();
  });

  it('switches to the mono family at 14 px', async () => {
    await render(
      <TextField value="" onChangeText={() => {}} mono testID="input" />,
    );
    expect(inputStyle()).toMatchObject({
      fontFamily: 'GeistMono-Regular',
      fontSize: 14,
    });
  });

  it('applies secure, keyboard and alignment options', async () => {
    await render(
      <TextField
        value=""
        onChangeText={() => {}}
        secure
        keyboardType="number-pad"
        align="center"
        testID="input"
      />,
    );
    const input = screen.getByTestId('input');
    expect(input).toHaveProp('secureTextEntry', true);
    expect(input).toHaveProp('keyboardType', 'number-pad');
    expect(inputStyle().textAlign).toBe('center');
  });

  it('defaults to no autocapitalize and no autocorrect', async () => {
    await render(<TextField value="" onChangeText={() => {}} testID="input" />);
    const input = screen.getByTestId('input');
    expect(input).toHaveProp('autoCapitalize', 'none');
    expect(input).toHaveProp('autoCorrect', false);
  });

  it('honours explicit autocapitalize and autocorrect overrides', async () => {
    await render(
      <TextField
        value=""
        onChangeText={() => {}}
        autoCapitalize="words"
        autoCorrect
        testID="input"
      />,
    );
    const input = screen.getByTestId('input');
    expect(input).toHaveProp('autoCapitalize', 'words');
    expect(input).toHaveProp('autoCorrect', true);
  });

  it('marks the field non editable when asked', async () => {
    await render(
      <TextField
        value="10.30.1.20"
        onChangeText={() => {}}
        editable={false}
        testID="input"
      />,
    );
    expect(screen.getByTestId('input')).toHaveProp('editable', false);
  });
});

describe('TextField platform and font scaling', () => {
  afterEach(() => jest.restoreAllMocks());

  it('removes Android underline and vertical padding and supplies theme colors', async () => {
    jest.replaceProperty(Platform, 'OS', 'android');
    await renderWithProviders(
      <TextField
        label="地址"
        value=""
        onChangeText={() => {}}
        testID="input"
      />,
    );
    const input = screen.getByTestId('input');
    expect(input).toHaveProp('underlineColorAndroid', 'transparent');
    expect(input).toHaveProp('cursorColor', '#7DA8FF');
    expect(input).toHaveProp('selectionColor', 'rgba(125, 168, 255, 0.16)');
    expect(inputStyle()).toMatchObject({
      height: 48,
      paddingVertical: 0,
      textAlignVertical: 'center',
    });
  });

  it('leaves Android props and styles unset on iOS', async () => {
    jest.replaceProperty(Platform, 'OS', 'ios');
    await renderWithProviders(
      <TextField
        label="地址"
        value=""
        onChangeText={() => {}}
        testID="input"
      />,
    );
    const input = screen.getByTestId('input');
    expect(input.props.underlineColorAndroid).toBeUndefined();
    expect(input.props.cursorColor).toBeUndefined();
    expect(input.props.selectionColor).toBeUndefined();
    expect(inputStyle().paddingVertical).toBeUndefined();
    expect(inputStyle().textAlignVertical).toBeUndefined();
    expect(inputStyle().height).toBe(46);
  });

  it.each([
    ['ios', false, 46],
    ['ios', true, 46],
    ['android', false, 48],
    ['android', true, 48],
  ] as const)(
    'allows %s input text to grow above scale 2 (mono=%s)',
    async (os, mono, minHeight) => {
      jest.replaceProperty(Platform, 'OS', os);
      const getFontScale = jest
        .spyOn(PixelRatio, 'getFontScale')
        .mockReturnValue(3);
      const { rerender } = await renderWithProviders(
        <TextField
          label="地址"
          value="hs.example.com"
          onChangeText={() => {}}
          mono={mono}
          testID="input"
        />,
      );
      expect(inputStyle().minHeight).toBe(minHeight);
      expect(inputStyle()).not.toHaveProperty('height');
      expect(screen.getByTestId('input').props.numberOfLines).toBeUndefined();

      getFontScale.mockReturnValue(2);
      await rerender(
        <TextField
          label="地址"
          value="hs.example.com"
          onChangeText={() => {}}
          mono={mono}
          testID="input"
        />,
      );
      expect(inputStyle().height).toBe(minHeight);
      expect(inputStyle()).not.toHaveProperty('minHeight');
    },
  );
});

describe('TextField keyboard, focus and secure options', () => {
  afterEach(() => jest.restoreAllMocks());

  it('forwards its ref to the input so callers can focus it', async () => {
    const ref = createRef<TextFieldHandle>();
    await render(
      <TextField ref={ref} value="" onChangeText={() => {}} testID="input" />,
    );
    const handle = ref.current;
    if (handle == null) {
      throw new Error('TextField did not attach its ref');
    }
    // The preset's TextInput mock gives the instance jest.fn focus/blur.
    const focus = jest.mocked(handle.focus);
    focus.mockClear();
    handle.focus();
    expect(focus).toHaveBeenCalledTimes(1);
  });

  it('passes the return key, submit and autofill options to the input', async () => {
    const onSubmitEditing = jest.fn();
    await render(
      <TextField
        autoComplete="current-password"
        blurOnSubmit={false}
        maxLength={64}
        onChangeText={() => {}}
        onSubmitEditing={onSubmitEditing}
        returnKeyType="next"
        testID="input"
        textContentType="password"
        value=""
      />,
    );
    const input = screen.getByTestId('input');
    expect(input).toHaveProp('returnKeyType', 'next');
    expect(input).toHaveProp('blurOnSubmit', false);
    expect(input).toHaveProp('textContentType', 'password');
    expect(input).toHaveProp('autoComplete', 'current-password');
    expect(input).toHaveProp('maxLength', 64);
    await fireEvent(input, 'submitEditing', { nativeEvent: { text: 'x' } });
    expect(onSubmitEditing).toHaveBeenCalledTimes(1);
  });

  it('leaves the new options unset by default', async () => {
    await render(<TextField value="" onChangeText={() => {}} testID="input" />);
    const input = screen.getByTestId('input');
    expect(input.props.returnKeyType).toBeUndefined();
    expect(input.props.textContentType).toBeUndefined();
    expect(input.props.maxLength).toBeUndefined();
    expect(input).toHaveProp('multiline', false);
    expect(screen.queryByRole('button')).toBeNull();
    // No wrapper around a plain input: screens walk up from it to their rows.
    expect(input.parent).toHaveStyle({ alignSelf: 'stretch' });
  });

  it('grows a multiline field from a minimum height with top-aligned text', async () => {
    const view = await render(
      <TextField multiline value="" onChangeText={() => {}} testID="input" />,
    );
    expect(screen.getByTestId('input')).toHaveProp('multiline', true);
    expect(inputStyle()).toMatchObject({
      minHeight: 96,
      textAlignVertical: 'top',
      paddingTop: 10,
      paddingBottom: 10,
    });
    expect(inputStyle()).not.toHaveProperty('height');

    await view.rerender(
      <TextField
        minHeight={140}
        multiline
        value=""
        onChangeText={() => {}}
        testID="input"
      />,
    );
    expect(inputStyle().minHeight).toBe(140);
  });

  it('keeps the multiline text in place while the border thickens', async () => {
    await render(
      <TextField multiline value="" onChangeText={() => {}} testID="input" />,
    );
    const idle = inputStyle();
    await fireEvent(screen.getByTestId('input'), 'focus');
    const focused = inputStyle();
    expect(focused.borderWidth).toBe(2);
    expect(focused.paddingTop + focused.borderWidth).toBe(
      idle.paddingTop + idle.borderWidth,
    );
    expect(focused.paddingBottom + focused.borderWidth).toBe(
      idle.paddingBottom + idle.borderWidth,
    );
  });

  it.each(['ios', 'android'] as const)(
    'does not shift the text sideways when focused on %s',
    async os => {
      jest.replaceProperty(Platform, 'OS', os);
      await renderWithProviders(
        <TextField value="" onChangeText={() => {}} testID="input" />,
      );
      const idle = inputStyle();
      await fireEvent(screen.getByTestId('input'), 'focus');
      const focused = inputStyle();
      expect(idle.borderWidth).toBe(1);
      expect(focused.borderWidth).toBe(2);
      expect(focused.paddingLeft + focused.borderWidth).toBe(
        idle.paddingLeft + idle.borderWidth,
      );
      expect(focused.paddingRight + focused.borderWidth).toBe(
        idle.paddingRight + idle.borderWidth,
      );
      expect(focused.height).toBe(idle.height);
    },
  );

  it('shows and hides a secure value with a labelled toggle', async () => {
    await render(
      <TextField
        label="密码"
        secure
        value="hunter2"
        onChangeText={() => {}}
        testID="input"
      />,
    );
    const input = () => screen.getByTestId('input');
    expect(input()).toHaveProp('secureTextEntry', true);
    // The toggle sits inside the field; the text stops before it.
    expect(inputStyle().paddingRight).toBe(44);

    await fireEvent.press(screen.getByRole('button', { name: '显示密码' }));
    expect(input()).toHaveProp('secureTextEntry', false);

    await fireEvent.press(screen.getByRole('button', { name: '隐藏密码' }));
    expect(input()).toHaveProp('secureTextEntry', true);
    expect(screen.getByTestId('input-reveal')).toBeOnTheScreen();
  });

  it('gives the Android toggle a 48 dp target', async () => {
    jest.replaceProperty(Platform, 'OS', 'android');
    await renderWithProviders(
      <TextField secure value="" onChangeText={() => {}} testID="input" />,
    );
    expect(
      StyleSheet.flatten(screen.getByTestId('input-reveal').props.style),
    ).toMatchObject({ width: 48, top: 0, bottom: 0 });
  });

  it('announces an error on iOS when it appears or changes', async () => {
    jest.replaceProperty(Platform, 'OS', 'ios');
    const announce = jest.mocked(AccessibilityInfo.announceForAccessibility);
    announce.mockClear();
    const field = (error?: string) => (
      <TextField
        error={error}
        label="地址"
        value="x"
        onChangeText={() => {}}
        testID="input"
      />
    );
    const view = await render(field());
    expect(announce).not.toHaveBeenCalled();

    await view.rerender(field('格式不对'));
    expect(announce).toHaveBeenCalledWith('格式不对');
    await view.rerender(field('格式不对'));
    expect(announce).toHaveBeenCalledTimes(1);

    await view.rerender(field('端口要在 1–65535 之间'));
    expect(announce).toHaveBeenLastCalledWith('端口要在 1–65535 之间');
    expect(announce).toHaveBeenCalledTimes(2);
  });

  it('leaves Android error announcements to the live region', async () => {
    jest.replaceProperty(Platform, 'OS', 'android');
    const announce = jest.mocked(AccessibilityInfo.announceForAccessibility);
    announce.mockClear();
    await renderWithProviders(
      <TextField
        error="格式不对"
        value="x"
        onChangeText={() => {}}
        testID="input"
      />,
    );
    expect(screen.getByText('格式不对')).toHaveProp(
      'accessibilityLiveRegion',
      'polite',
    );
    expect(announce).not.toHaveBeenCalled();
  });
});
