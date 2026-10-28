import type { ReactNode } from 'react';
import { KeyboardAvoidingView, ScrollView, View } from 'react-native';
import type { ScrollViewProps, StyleProp, ViewStyle } from 'react-native';
import { isAndroid } from '../theme/platform';

export type FormScrollProps = {
  children: ReactNode;
  /** Pinned above the scroll area, e.g. a close button row. */
  header?: ReactNode;
  /** Rendered after the scroll area: a pinned footer, sheets and dialogs. */
  footer?: ReactNode;
  style?: StyleProp<ViewStyle>;
  contentContainerStyle?: StyleProp<ViewStyle>;
  importantForAccessibility?: ScrollViewProps['importantForAccessibility'];
  /** The scroll view gets `${testID}-scroll`. */
  testID?: string;
};

/**
 * Full-page form layout. On iOS the page stays put when the keyboard opens:
 * only the scroll view insets its bottom by the keyboard, and RN scrolls the
 * focused input above it. Android keeps the padding KeyboardAvoidingView.
 */
export function FormScroll({
  children,
  header,
  footer,
  style,
  contentContainerStyle,
  importantForAccessibility,
  testID,
}: FormScrollProps) {
  const android = isAndroid();
  const scroll = (
    <ScrollView
      {...(android
        ? {}
        : {
            automaticallyAdjustKeyboardInsets: true,
            keyboardDismissMode: 'interactive' as const,
          })}
      contentContainerStyle={contentContainerStyle}
      importantForAccessibility={importantForAccessibility}
      keyboardShouldPersistTaps="handled"
      showsVerticalScrollIndicator={false}
      testID={testID === undefined ? undefined : `${testID}-scroll`}
    >
      {children}
    </ScrollView>
  );

  if (android) {
    return (
      <KeyboardAvoidingView behavior="padding" style={style} testID={testID}>
        {header}
        {scroll}
        {footer}
      </KeyboardAvoidingView>
    );
  }
  return (
    <View style={style} testID={testID}>
      {header}
      {scroll}
      {footer}
    </View>
  );
}

export default FormScroll;
