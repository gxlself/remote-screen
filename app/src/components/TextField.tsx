import { useEffect, useState } from 'react';
import type { ComponentRef, Ref } from 'react';
import {
  AccessibilityInfo,
  Platform,
  Pressable,
  StyleSheet,
  TextInput,
  View,
} from 'react-native';
import type {
  AutoCapitalize,
  KeyboardTypeOptions,
  TextInputProps,
} from 'react-native';
import { common } from '../i18n/zh/common';
import { boxHeight } from '../theme/fontScale';
import { isAndroid } from '../theme/platform';
import { hit, tokens } from '../theme/tokens';
import { fontFor } from '../theme/typography';
import AppText from './AppText';
import { Icon } from './icons/Icon';

export type TextFieldHandle = ComponentRef<typeof TextInput>;

export type TextFieldProps = {
  ref?: Ref<TextFieldHandle>;
  label?: string;
  value: string;
  onChangeText: (text: string) => void;
  placeholder?: string;
  mono?: boolean;
  error?: string;
  hint?: string;
  secure?: boolean;
  keyboardType?: KeyboardTypeOptions;
  autoCapitalize?: AutoCapitalize;
  autoCorrect?: boolean;
  autoFocus?: boolean;
  editable?: boolean;
  align?: 'left' | 'center';
  returnKeyType?: TextInputProps['returnKeyType'];
  onSubmitEditing?: TextInputProps['onSubmitEditing'];
  blurOnSubmit?: boolean;
  textContentType?: TextInputProps['textContentType'];
  autoComplete?: TextInputProps['autoComplete'];
  multiline?: boolean;
  // Height of an empty multiline field; it grows with its text.
  minHeight?: number;
  maxLength?: number;
  testID?: string;
  accessibilityLabel?: string;
};

const MULTILINE_MIN_HEIGHT = 96;
// The focus border is 1 pt thicker; the padding gives that point back so the
// text does not move.
const PADDING = 12;
const MULTILINE_PADDING = 10;

export function TextField({
  ref,
  label,
  value,
  onChangeText,
  placeholder,
  mono = false,
  error,
  hint,
  secure = false,
  keyboardType,
  autoCapitalize = 'none',
  autoCorrect = false,
  autoFocus,
  editable = true,
  align = 'left',
  returnKeyType,
  onSubmitEditing,
  blurOnSubmit,
  textContentType,
  autoComplete,
  multiline = false,
  minHeight = MULTILINE_MIN_HEIGHT,
  maxLength,
  testID,
  accessibilityLabel,
}: TextFieldProps) {
  const android = isAndroid();
  const height = android ? Math.max(46, hit.min) : 46;
  const [focused, setFocused] = useState(false);
  const [revealed, setRevealed] = useState(false);
  const hasError = error != null && error !== '';
  const borderColor = hasError
    ? tokens.bad
    : focused
    ? tokens.accent
    : tokens.inputBorder;
  const inset = focused ? 1 : 0;
  const toggleSize = android ? hit.min : 44;

  // Read the error out when it appears or changes (Android's live region
  // on the message covers TalkBack).
  useEffect(() => {
    if (hasError && Platform.OS === 'ios') {
      AccessibilityInfo.announceForAccessibility(error);
    }
  }, [error, hasError]);

  const input = (
    <TextInput
      {...(android
        ? {
            underlineColorAndroid: 'transparent',
            cursorColor: tokens.accent,
            selectionColor: tokens.accentSoft,
          }
        : {})}
      accessibilityHint={hasError ? error : undefined}
      accessibilityLabel={accessibilityLabel ?? label}
      autoCapitalize={autoCapitalize}
      autoComplete={autoComplete}
      autoCorrect={autoCorrect}
      autoFocus={autoFocus}
      blurOnSubmit={blurOnSubmit}
      editable={editable}
      keyboardType={keyboardType}
      maxLength={maxLength}
      multiline={multiline}
      onBlur={() => setFocused(false)}
      onChangeText={onChangeText}
      onFocus={() => setFocused(true)}
      onSubmitEditing={onSubmitEditing}
      placeholder={placeholder}
      placeholderTextColor={tokens.text3}
      ref={ref}
      returnKeyType={returnKeyType}
      secureTextEntry={secure && !revealed}
      style={[
        styles.input,
        multiline ? { minHeight } : boxHeight(height),
        android ? styles.inputAndroid : null,
        multiline ? styles.inputMultiline : null,
        mono ? styles.inputMono : styles.inputSans,
        focused ? styles.inputFocused : styles.inputIdle,
        {
          borderColor,
          paddingLeft: PADDING - inset,
          paddingRight: (secure ? toggleSize : PADDING) - inset,
        },
        multiline
          ? {
              paddingTop: MULTILINE_PADDING - inset,
              paddingBottom: MULTILINE_PADDING - inset,
            }
          : null,
        align === 'center' ? styles.inputCentered : null,
      ]}
      testID={testID}
      textContentType={textContentType}
      value={value}
    />
  );

  return (
    <View style={styles.field}>
      {label == null ? null : (
        <AppText
          color="text2"
          variant="footnote"
          weight={500}
          style={styles.label}
        >
          {label}
        </AppText>
      )}
      {secure ? (
        // Only secure fields get the wrapper, so the input keeps its place
        // in the tree for callers that walk up from it.
        <View>
          {input}
          <Pressable
            accessibilityLabel={
              revealed ? common.a11y.hidePassword : common.a11y.showPassword
            }
            accessibilityRole="button"
            onPress={() => setRevealed(current => !current)}
            style={({ pressed }) => [
              styles.toggle,
              { width: toggleSize },
              pressed ? styles.togglePressed : null,
            ]}
            testID={testID == null ? undefined : `${testID}-reveal`}
          >
            <Icon color="text3" name={revealed ? 'eyeOff' : 'eye'} size={18} />
          </Pressable>
        </View>
      ) : (
        input
      )}
      {hasError ? (
        <AppText
          accessibilityLiveRegion="polite"
          color="bad"
          variant="caption"
          style={styles.message}
        >
          {error}
        </AppText>
      ) : hint == null ? null : (
        <AppText color="text3" variant="caption" style={styles.message}>
          {hint}
        </AppText>
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  field: {
    alignSelf: 'stretch',
  },
  label: {
    marginBottom: 6,
  },
  input: {
    borderRadius: 12,
    paddingHorizontal: PADDING,
    backgroundColor: tokens.card,
    color: tokens.text,
    textAlign: 'left',
  },
  inputAndroid: {
    paddingVertical: 0,
    textAlignVertical: 'center',
  },
  inputMultiline: {
    textAlignVertical: 'top',
  },
  inputSans: {
    fontFamily: fontFor(400),
    fontSize: 15,
  },
  inputMono: {
    fontFamily: fontFor(400, true),
    fontSize: 14,
  },
  inputIdle: {
    borderWidth: 1,
  },
  inputFocused: {
    borderWidth: 2,
  },
  inputCentered: {
    textAlign: 'center',
  },
  toggle: {
    position: 'absolute',
    top: 0,
    right: 0,
    bottom: 0,
    alignItems: 'center',
    justifyContent: 'center',
  },
  togglePressed: {
    opacity: 0.6,
  },
  message: {
    marginTop: 6,
  },
});
