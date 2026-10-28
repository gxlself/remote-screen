import { useImperativeHandle, useRef, useState } from 'react';
import type { ComponentRef, Ref } from 'react';
import {
  PixelRatio,
  Platform,
  Pressable,
  StyleSheet,
  TextInput,
  View,
} from 'react-native';
import { digitsOf, sanitizeCode } from '../../features/pairing/codeInput';
import { pairAdd } from '../../i18n/zh/pairAdd';
import { fontFor } from '../../theme/typography';
import { hit, tokens } from '../../theme/tokens';
import AppText from '../../components/AppText';

export type CodeBoxesHandle = { focus: () => void };

export type CodeBoxesProps = {
  ref?: Ref<CodeBoxesHandle>;
  value: string;
  onChange: (code: string) => void;
  editable?: boolean;
  autoFocus?: boolean;
  testID?: string;
};

export function CodeBoxes({
  ref,
  value,
  onChange,
  editable = true,
  autoFocus,
  testID = 'pair-code',
}: CodeBoxesProps) {
  const inputRef = useRef<ComponentRef<typeof TextInput>>(null);
  const [focused, setFocused] = useState(false);
  const digits = digitsOf(value);
  // Only the box the next digit goes into is lit, and only while typing.
  const activeIndex = focused ? Math.min(digits.join('').length, 5) : -1;
  const fontScale = PixelRatio.getFontScale();
  const horizontalHitSlop = Math.max(0, (hit.min - 46) / 2);
  const verticalHitSlop = Math.max(0, (hit.min - 56) / 2);

  const focusInput = () => {
    if (editable) {
      inputRef.current?.focus();
    }
  };

  useImperativeHandle(ref, () => ({ focus: focusInput }));

  return (
    <View style={styles.container}>
      <TextInput
        ref={inputRef}
        accessibilityLabel={pairAdd.a11y.codeInput}
        autoComplete="one-time-code"
        autoFocus={autoFocus}
        editable={editable}
        importantForAccessibility={Platform.OS === 'android' ? 'no' : undefined}
        keyboardType="number-pad"
        maxLength={6}
        onBlur={() => setFocused(false)}
        onChangeText={text => onChange(sanitizeCode(text))}
        onFocus={() => setFocused(true)}
        style={styles.hiddenInput}
        testID={`${testID}-input`}
        textContentType="oneTimeCode"
        value={value}
      />
      <View style={[styles.boxes, fontScale > 2 ? styles.boxesLarge : null]}>
        {digits.map((digit, index) => {
          const position = index + 1;
          const label = digit === '' ? pairAdd.a11y.emptyDigit : digit;
          return (
            <Pressable
              key={position}
              accessibilityLabel={pairAdd.a11y.codeDigit(position)}
              accessibilityRole="button"
              accessibilityValue={{ text: label }}
              disabled={!editable}
              hitSlop={{
                bottom: verticalHitSlop,
                left: horizontalHitSlop,
                right: horizontalHitSlop,
                top: verticalHitSlop,
              }}
              onPress={focusInput}
              style={[
                styles.box,
                activeIndex === index ? styles.boxActive : styles.boxIdle,
              ]}
              testID={`${testID}-box-${position}`}
            >
              <AppText
                mono
                color={digit === '' ? 'text3' : 'text'}
                style={styles.digit}
                weight={600}
              >
                {digit}
              </AppText>
            </Pressable>
          );
        })}
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    alignSelf: 'stretch',
  },
  hiddenInput: {
    position: 'absolute',
    width: 1,
    height: 1,
    opacity: 0,
  },
  boxes: {
    flexDirection: 'row',
    justifyContent: 'center',
    gap: 7,
  },
  boxesLarge: {
    flexWrap: 'wrap',
    rowGap: 7,
  },
  box: {
    // Grows with large text instead of clipping the digit.
    minWidth: 46,
    paddingHorizontal: 4,
    minHeight: 56,
    borderRadius: 12,
    backgroundColor: tokens.card,
    alignItems: 'center',
    justifyContent: 'center',
  },
  boxIdle: {
    borderWidth: 1,
    borderColor: tokens.inputLine,
  },
  boxActive: {
    borderWidth: 2,
    borderColor: tokens.accent,
  },
  digit: {
    fontFamily: fontFor(600, true),
    fontSize: 24,
    lineHeight: 30,
  },
});

export default CodeBoxes;
