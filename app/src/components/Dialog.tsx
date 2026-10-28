import type { ComponentRef, ReactNode } from 'react';
import { useEffect, useRef, useState } from 'react';
import {
  AccessibilityInfo,
  Modal,
  Platform,
  Pressable,
  ScrollView,
  StyleSheet,
  View,
  useWindowDimensions,
} from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { common } from '../i18n/zh/common';
import { radius, space, tokens } from '../theme/tokens';
import AppText from './AppText';
import { Button, type ButtonVariant } from './Button';
import { useKeyboardOverlap } from './useKeyboardOverlap';

// Below this much room for the message and fields the title and actions stop
// being pinned and the whole card scrolls (a text field with its label).
const MIN_PINNED_BODY = 88;

// Android back while an action is loading: keep the dialog up.
function ignoreRequestClose() {}

export type DialogAction = {
  label: string;
  onPress: () => void;
  variant?: Extract<
    ButtonVariant,
    'primary' | 'secondary' | 'pillDanger' | 'text'
  >;
  // While any action is loading every button is held and the dialog cannot be
  // dismissed from the backdrop, the back button or the escape gesture.
  loading?: boolean;
  disabled?: boolean;
  testID?: string;
};

export type DialogProps = {
  visible: boolean;
  title: string;
  message?: string;
  children?: ReactNode;
  actions: DialogAction[];
  onDismiss?: () => void;
  testID?: string;
};

export function Dialog({
  visible,
  title,
  message,
  children,
  actions,
  onDismiss,
  testID,
}: DialogProps) {
  const titleRef = useRef<ComponentRef<typeof View>>(null);
  const isAndroid = Platform.OS === 'android';
  const { width, height } = useWindowDimensions();
  const insets = useSafeAreaInsets();
  const keyboard = useKeyboardOverlap(visible);
  const [titleHeight, setTitleHeight] = useState(0);
  const [actionsHeight, setActionsHeight] = useState(0);
  const cardWidth = Math.max(0, Math.min(400, width - 40));
  // Centred in the window above the keyboard, clear of the status bar.
  const cardMaxHeight = Math.max(
    0,
    height - keyboard - 2 * Math.max(insets.top, space.lg),
  );
  const cardPadding = isAndroid
    ? styles.androidCard.padding
    : styles.card.padding;
  const frameHeight =
    cardMaxHeight - 2 * (cardPadding + styles.card.borderWidth);
  const hasBody = message != null || (children != null && children !== false);
  const busy = actions.some(action => action.loading === true);
  const dismiss = busy ? undefined : onDismiss;
  // Title and actions stay pinned while the body keeps enough room to scroll;
  // otherwise the whole card scrolls. The element tree never changes, so a
  // focused field keeps its keyboard.
  const pinned =
    hasBody &&
    titleHeight + actionsHeight + 2 * styles.frame.gap + MIN_PINNED_BODY <=
      frameHeight;
  const backdropTestID = testID ? `${testID}-backdrop` : 'dialog-backdrop';
  const cardTestID = testID ? `${testID}-card` : 'dialog-card';
  const frameTestID = testID ? `${testID}-frame` : 'dialog-frame';
  const scrollTestID = testID ? `${testID}-scroll` : 'dialog-scroll';
  const actionsTestID = testID ? `${testID}-actions` : 'dialog-actions';

  useEffect(() => {
    if (!visible) {
      return;
    }
    const timer = setTimeout(() => {
      if (titleRef.current != null) {
        AccessibilityInfo.sendAccessibilityEvent(titleRef.current, 'focus');
      }
    }, 0);
    return () => clearTimeout(timer);
  }, [visible]);

  return (
    <Modal
      {...(isAndroid
        ? { statusBarTranslucent: true, navigationBarTranslucent: true }
        : {})}
      animationType="fade"
      onRequestClose={busy ? ignoreRequestClose : onDismiss}
      testID={testID}
      transparent
      visible={visible}
    >
      <View
        accessibilityViewIsModal
        onAccessibilityEscape={dismiss}
        style={[styles.overlay, { paddingBottom: keyboard }]}
        testID={testID ? `${testID}-overlay` : 'dialog-overlay'}
      >
        <Pressable
          accessibilityLabel={dismiss == null ? undefined : common.a11y.close}
          accessibilityRole={dismiss == null ? undefined : 'button'}
          disabled={dismiss == null}
          onPress={dismiss}
          style={styles.backdrop}
          testID={backdropTestID}
        />
        <View
          style={[
            styles.card,
            isAndroid ? styles.androidCard : null,
            { width: cardWidth, maxHeight: cardMaxHeight },
          ]}
          testID={cardTestID}
        >
          <ScrollView
            alwaysBounceVertical={false}
            contentContainerStyle={[
              styles.frame,
              pinned ? { maxHeight: frameHeight } : null,
            ]}
            keyboardShouldPersistTaps="handled"
            scrollEnabled={!pinned}
            showsVerticalScrollIndicator={false}
            style={styles.frameScroll}
            testID={frameTestID}
          >
            <View
              accessible
              accessibilityLabel={title}
              accessibilityRole="header"
              onLayout={event =>
                setTitleHeight(event.nativeEvent.layout.height)
              }
              ref={titleRef}
            >
              <AppText
                center
                style={styles.title}
                variant="headline"
                weight={600}
              >
                {title}
              </AppText>
            </View>
            {hasBody ? (
              <ScrollView
                alwaysBounceVertical={false}
                contentContainerStyle={styles.bodyContent}
                keyboardShouldPersistTaps="handled"
                nestedScrollEnabled
                showsVerticalScrollIndicator={false}
                style={styles.body}
                testID={scrollTestID}
              >
                {message == null ? null : (
                  <AppText center color="text2" variant="body">
                    {message}
                  </AppText>
                )}
                {children}
              </ScrollView>
            ) : null}
            <View
              onLayout={event =>
                setActionsHeight(event.nativeEvent.layout.height)
              }
              style={styles.actions}
              testID={actionsTestID}
            >
              {actions.map(action => (
                <Button
                  disabled={action.disabled || (busy && !action.loading)}
                  fullWidth
                  key={action.label}
                  loading={action.loading}
                  onPress={action.onPress}
                  testID={action.testID}
                  title={action.label}
                  variant={action.variant ?? 'primary'}
                />
              ))}
            </View>
          </ScrollView>
        </View>
      </View>
    </Modal>
  );
}

const styles = StyleSheet.create({
  overlay: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    paddingHorizontal: space.gutter,
  },
  backdrop: {
    position: 'absolute',
    top: 0,
    right: 0,
    bottom: 0,
    left: 0,
    backgroundColor: 'rgba(0, 0, 0, 0.5)',
  },
  card: {
    gap: space.md,
    padding: space.gutter,
    borderRadius: radius.panel,
    borderWidth: 1,
    borderColor: tokens.panelLine,
    backgroundColor: tokens.panel,
  },
  // 同 07 · Android：Material 3 对话框圆角与内边距。
  androidCard: {
    borderRadius: 28,
    padding: 24,
  },
  frameScroll: {
    flexGrow: 0,
    flexShrink: 1,
  },
  frame: {
    gap: space.md,
  },
  body: {
    flexShrink: 1,
  },
  bodyContent: {
    gap: space.md,
  },
  title: {
    alignSelf: 'stretch',
  },
  actions: {
    gap: space.sm,
  },
});

export default Dialog;
