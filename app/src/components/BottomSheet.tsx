import type { ComponentRef, ReactNode } from 'react';
import { useEffect, useMemo, useRef, useState } from 'react';
import {
  AccessibilityInfo,
  Animated,
  BackHandler,
  Easing,
  PanResponder,
  Platform,
  Pressable,
  ScrollView,
  StyleSheet,
  View,
  useWindowDimensions,
} from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { common } from '../i18n/zh/common';
import { hit, radius, space, tokens } from '../theme/tokens';
import AppText from './AppText';
import { Icon } from './icons/Icon';
import { useKeyboardOverlap } from './useKeyboardOverlap';

// Below this much room for the content the header and footer stop being
// pinned and the whole sheet scrolls instead (a text field with its label).
const MIN_PINNED_BODY = 88;
// Wide windows (iPad, landscape) keep the sheet to a readable column.
export const SHEET_MAX_WIDTH = 540;
const ENTER_MS = 250;
const EXIT_MS = 200;
// A pull on the handle or header closes the sheet once it passes this
// distance, or on a quick flick (pt per ms).
export const DRAG_DISMISS_DISTANCE = 96;
export const DRAG_DISMISS_VELOCITY = 0.8;
const DRAG_SLOP = 6;

export type BottomSheetProps = {
  onClose: () => void;
  title: string;
  subtitle?: string;
  leading?: ReactNode;
  children: ReactNode;
  scroll?: boolean;
  footer?: ReactNode;
  // false: the backdrop, Android back and pulling the handle leave it open;
  // only the close button and the caller's own buttons call onClose.
  dismissible?: boolean;
  testID?: string;
};

export function BottomSheet({
  onClose,
  title,
  subtitle,
  leading,
  children,
  scroll = false,
  footer,
  dismissible = true,
  testID,
}: BottomSheetProps) {
  const titleRef = useRef<ComponentRef<typeof View>>(null);
  const isAndroid = Platform.OS === 'android';
  const closeSize = isAndroid ? 48 : hit.min; // 同 07 · Android：可点区域至少 48dp。
  const insets = useSafeAreaInsets();
  const { height } = useWindowDimensions();
  const keyboard = useKeyboardOverlap();
  const [headerHeight, setHeaderHeight] = useState(0);
  const [footerHeight, setFooterHeight] = useState(0);
  const enter = useRef(new Animated.Value(0)).current;
  const drag = useRef(new Animated.Value(0)).current;
  const latest = useRef({ onClose, dismissible, height });
  latest.current = { onClose, dismissible, height };

  useEffect(() => {
    const timer = setTimeout(() => {
      if (titleRef.current != null) {
        AccessibilityInfo.sendAccessibilityEvent(titleRef.current, 'focus');
      }
    }, 0);
    return () => clearTimeout(timer);
  }, []);

  // The backdrop fades in while the panel slides up, whatever animation the
  // caller's Modal uses.
  useEffect(() => {
    const animation = Animated.timing(enter, {
      toValue: 1,
      duration: ENTER_MS,
      easing: Easing.out(Easing.cubic),
      useNativeDriver: true,
    });
    animation.start();
    return () => animation.stop();
  }, [enter]);

  useEffect(() => {
    if (!isAndroid) {
      return;
    }
    const subscription = BackHandler.addEventListener(
      'hardwareBackPress',
      () => {
        // Swallowed either way, so back never leaves the screen under a sheet
        // that is not dismissible.
        if (dismissible) {
          onClose();
        }
        return true;
      },
    );
    return () => subscription.remove();
  }, [dismissible, isAndroid, onClose]);

  const pan = useMemo(
    () =>
      PanResponder.create({
        onMoveShouldSetPanResponder: (_event, gesture) =>
          latest.current.dismissible &&
          gesture.dy > DRAG_SLOP &&
          Math.abs(gesture.dy) > Math.abs(gesture.dx),
        onPanResponderMove: (_event, gesture) => {
          drag.setValue(Math.max(0, gesture.dy));
        },
        onPanResponderRelease: (_event, gesture) => {
          if (
            gesture.dy > DRAG_DISMISS_DISTANCE ||
            (gesture.dy > DRAG_SLOP && gesture.vy > DRAG_DISMISS_VELOCITY)
          ) {
            Animated.timing(drag, {
              toValue: latest.current.height,
              duration: EXIT_MS,
              easing: Easing.in(Easing.cubic),
              useNativeDriver: true,
            }).start(({ finished }) => {
              if (finished) {
                latest.current.onClose();
              }
            });
            return;
          }
          Animated.spring(drag, {
            toValue: 0,
            bounciness: 0,
            useNativeDriver: true,
          }).start();
        },
        onPanResponderTerminate: () => {
          Animated.spring(drag, {
            toValue: 0,
            bounciness: 0,
            useNativeDriver: true,
          }).start();
        },
      }),
    [drag],
  );

  const translateY = useMemo(
    () =>
      Animated.add(
        enter.interpolate({ inputRange: [0, 1], outputRange: [height, 0] }),
        drag,
      ),
    [drag, enter, height],
  );
  const backdropOpacity = useMemo(
    () =>
      Animated.multiply(
        enter,
        drag.interpolate({
          inputRange: [0, Math.max(1, height)],
          outputRange: [1, 0],
          extrapolate: 'clamp',
        }),
      ),
    [drag, enter, height],
  );

  // The keyboard already covers the home indicator.
  const paddingBottom =
    keyboard > 0 ? space.lg : Math.max(34, insets.bottom + 8);
  const available = Math.max(0, height - keyboard - insets.top - space.md);
  const maxHeight = scroll ? Math.min(height * 0.85, available) : available;
  // The frame holds the handle, header, body and footer. When the header and
  // footer leave the body enough room they stay pinned and only the body
  // scrolls; otherwise (landscape with the keyboard up) everything scrolls.
  // Either way the element tree is the same, so a focused field never remounts.
  const frameHeight = maxHeight - styles.surface.paddingTop - paddingBottom;
  const handleHeight = isAndroid
    ? styles.androidHandle.height
    : styles.handle.height;
  const chrome =
    handleHeight +
    headerHeight +
    (footer == null ? 0 : footerHeight) +
    styles.frame.gap * (footer == null ? 2 : 3);
  const pinned = chrome + MIN_PINNED_BODY <= frameHeight;
  const backdropTestID = testID ? `${testID}-backdrop` : 'sheet-backdrop';
  const closeTestID = testID ? `${testID}-close` : 'sheet-close';
  const closeCircleTestID = testID
    ? `${testID}-close-circle`
    : 'sheet-close-circle';
  const handleTestID = testID ? `${testID}-handle` : 'sheet-handle';
  const grabTestID = testID ? `${testID}-grab` : 'sheet-grab';
  const panelTestID = testID ? `${testID}-panel` : 'bottom-sheet-panel';
  const surfaceTestID = testID ? `${testID}-surface` : 'bottom-sheet-surface';
  const frameTestID = testID ? `${testID}-frame` : 'bottom-sheet-frame';
  const headerTestID = testID ? `${testID}-header` : 'sheet-header';
  const footerTestID = testID ? `${testID}-footer` : 'sheet-footer';
  const scrollTestID = testID ? `${testID}-scroll` : 'bottom-sheet-scroll';
  const content = (
    <>
      {leading == null ? null : <View>{leading}</View>}
      <View style={styles.titleBlock}>
        <View
          accessible
          accessibilityRole="header"
          accessibilityLabel={title}
          ref={titleRef}
        >
          <AppText
            color="text"
            style={styles.title}
            variant="headline"
            weight={600}
          >
            {title}
          </AppText>
        </View>
        {subtitle == null ? null : (
          <AppText color="text3" style={styles.subtitle} variant="footnote">
            {subtitle}
          </AppText>
        )}
      </View>
    </>
  );

  return (
    <View
      accessibilityViewIsModal
      onAccessibilityEscape={dismissible ? onClose : undefined}
      style={[styles.root, { paddingBottom: keyboard }]}
      testID={testID}
    >
      <Animated.View
        pointerEvents="box-none"
        style={[styles.fill, { opacity: backdropOpacity }]}
      >
        <Pressable
          accessible={false}
          accessibilityLabel={dismissible ? common.a11y.close : undefined}
          accessibilityRole={dismissible ? 'button' : undefined}
          disabled={!dismissible}
          importantForAccessibility="no"
          onPress={dismissible ? onClose : undefined}
          style={styles.backdrop}
          testID={backdropTestID}
        />
      </Animated.View>
      <Animated.View
        style={[styles.panel, { transform: [{ translateY }] }]}
        testID={panelTestID}
      >
        <View
          style={[styles.surface, { paddingBottom, maxHeight }]}
          testID={surfaceTestID}
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
            <View {...pan.panHandlers} style={styles.grab} testID={grabTestID}>
              <View
                accessibilityElementsHidden
                accessible={false}
                importantForAccessibility="no-hide-descendants"
                style={[styles.handle, isAndroid ? styles.androidHandle : null]}
                testID={handleTestID}
              />
              <View
                onLayout={event =>
                  setHeaderHeight(event.nativeEvent.layout.height)
                }
                style={styles.header}
                testID={headerTestID}
              >
                {content}
                <Pressable
                  accessibilityLabel={common.a11y.close}
                  accessibilityRole="button"
                  onPress={onClose}
                  style={[
                    styles.closeButton,
                    { width: closeSize, height: closeSize },
                  ]}
                  testID={closeTestID}
                >
                  <View style={styles.closeCircle} testID={closeCircleTestID}>
                    <Icon color="text2" name="close" size={16} />
                  </View>
                </Pressable>
              </View>
            </View>
            <ScrollView
              alwaysBounceVertical={false}
              keyboardShouldPersistTaps="handled"
              nestedScrollEnabled
              showsVerticalScrollIndicator={false}
              style={styles.scroll}
              testID={scrollTestID}
            >
              {children}
            </ScrollView>
            {footer == null ? null : (
              <View
                onLayout={event =>
                  setFooterHeight(event.nativeEvent.layout.height)
                }
                testID={footerTestID}
              >
                {footer}
              </View>
            )}
          </ScrollView>
        </View>
      </Animated.View>
    </View>
  );
}

const styles = StyleSheet.create({
  root: {
    position: 'absolute',
    top: 0,
    right: 0,
    bottom: 0,
    left: 0,
    justifyContent: 'flex-end',
  },
  fill: {
    position: 'absolute',
    top: 0,
    right: 0,
    bottom: 0,
    left: 0,
  },
  backdrop: {
    position: 'absolute',
    top: 0,
    right: 0,
    bottom: 0,
    left: 0,
    backgroundColor: 'rgba(0, 0, 0, 0.5)',
  },
  panel: {
    width: '100%',
    alignItems: 'center',
  },
  surface: {
    width: '100%',
    maxWidth: SHEET_MAX_WIDTH,
    paddingTop: 8,
    paddingHorizontal: space.gutter,
    borderTopLeftRadius: radius.toolbar,
    borderTopRightRadius: radius.toolbar,
    backgroundColor: tokens.card,
    shadowColor: tokens.thumbBg,
    shadowOffset: { width: 0, height: -8 },
    shadowOpacity: 0.35,
    shadowRadius: 24,
    elevation: 24,
  },
  frameScroll: {
    flexGrow: 0,
    flexShrink: 1,
  },
  frame: {
    gap: space.lg,
  },
  // Handle and header: the area a downward pull closes the sheet from.
  grab: {
    gap: space.lg,
  },
  handle: {
    width: 36,
    height: 5,
    borderRadius: 3,
    alignSelf: 'center',
    backgroundColor: tokens.switchOff,
  },
  // 同 07 · Android：Material 3 bottom sheet 拖动条。
  androidHandle: {
    width: 32,
    height: 4,
    borderRadius: 2,
  },
  header: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: space.md,
  },
  titleBlock: {
    flex: 1,
    minWidth: 0,
  },
  title: {
    fontSize: 21,
    lineHeight: 26,
  },
  subtitle: {
    marginTop: 2,
  },
  closeButton: {
    alignItems: 'center',
    justifyContent: 'center',
  },
  closeCircle: {
    width: 30,
    height: 30,
    borderRadius: 15,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: tokens.track,
  },
  scroll: {
    flexShrink: 1,
  },
});

export default BottomSheet;
