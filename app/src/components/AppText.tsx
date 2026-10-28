import type { ComponentProps, ReactNode } from 'react';
import { StyleSheet, Text } from 'react-native';
import type { ColorValue, StyleProp, TextStyle } from 'react-native';
import { isAndroid } from '../theme/platform';
import { tokens } from '../theme/tokens';
import { textStyle } from '../theme/typography';
import type { TypeVariantName, Weight } from '../theme/typography';

type TokenColor = keyof typeof tokens;
type TextProps = Omit<
  ComponentProps<typeof Text>,
  'children' | 'style' | 'selectable' | 'numberOfLines'
>;

export type AppTextProps = TextProps & {
  children?: ReactNode;
  variant?: TypeVariantName;
  color?: TokenColor;
  weight?: Weight;
  mono?: boolean;
  center?: boolean;
  selectable?: boolean;
  style?: StyleProp<TextStyle>;
  numberOfLines?: number;
};

export default function AppText({
  children,
  variant = 'body',
  color = 'text',
  weight,
  mono = false,
  center = false,
  selectable = false,
  allowFontScaling = true,
  style,
  ...rest
}: AppTextProps) {
  const typography = textStyle(variant, weight);
  const fontFamily = mono
    ? typography.fontFamily.replace(/^Geist-/, 'GeistMono-')
    : typography.fontFamily;

  return (
    <Text
      {...rest}
      allowFontScaling={allowFontScaling}
      selectable={selectable}
      style={[
        typography,
        { color: tokens[color] as ColorValue, fontFamily },
        center && styles.center,
        style,
        isAndroid() ? styles.android : null,
      ]}
    >
      {children}
    </Text>
  );
}

const styles = StyleSheet.create({
  center: { textAlign: 'center' },
  android: { includeFontPadding: false },
});
