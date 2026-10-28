import type { ComponentRef, ReactNode } from 'react';
import { useEffect, useRef } from 'react';
import {
  AccessibilityInfo,
  Pressable,
  StyleSheet,
  View,
  useWindowDimensions,
} from 'react-native';
import { common } from '../i18n/zh/common';
import { hit, radius, space, tokens } from '../theme/tokens';
import AppText from './AppText';
import { IconButton } from './IconButton';

export type SessionPanelKind = 'side' | 'popover';

export type SessionPanelProps = {
  kind: SessionPanelKind;
  title: string;
  subtitle?: string;
  onClose: () => void;
  caretX?: number;
  showTitle?: boolean;
  backdropLabel?: string;
  closeLabel?: string;
  children: ReactNode;
  testID?: string;
};

export function SessionPanel({
  kind,
  title,
  subtitle,
  onClose,
  caretX,
  showTitle,
  backdropLabel,
  closeLabel,
  children,
  testID,
}: SessionPanelProps) {
  const titleRef = useRef<ComponentRef<typeof View>>(null);
  const { width, height } = useWindowDimensions();
  const portrait = width < height;
  const right = portrait ? space.md : hit.landscapeSafe;
  const side = kind === 'side';
  const panelWidth = Math.max(
    0,
    Math.min(side ? 540 : 560, width - hit.landscapeSafe - space.md),
  );
  const titleVisible = showTitle ?? side;
  const panelTestID = testID ? `${testID}-panel` : `session-panel-${kind}`;
  const backdropTestID = testID
    ? `${testID}-backdrop`
    : 'session-panel-backdrop';
  const caretTestID = testID ? `${testID}-caret` : 'session-panel-caret';

  useEffect(() => {
    const timer = setTimeout(() => {
      if (titleRef.current != null) {
        AccessibilityInfo.sendAccessibilityEvent(titleRef.current, 'focus');
      }
    }, 0);
    return () => clearTimeout(timer);
  }, []);

  return (
    <View
      accessibilityViewIsModal
      onAccessibilityEscape={onClose}
      pointerEvents="box-none"
      style={styles.root}
      testID={testID}
    >
      {side ? null : (
        <Pressable
          accessibilityLabel={backdropLabel ?? common.a11y.close}
          accessibilityRole="button"
          onPress={onClose}
          style={styles.backdrop}
          testID={backdropTestID}
        />
      )}
      <View
        style={[
          styles.panel,
          side ? styles.sidePanel : styles.popoverPanel,
          side
            ? {
                width: panelWidth,
                right,
              }
            : { width: panelWidth, right },
        ]}
        testID={panelTestID}
      >
        <View style={styles.header}>
          {titleVisible ? (
            <View style={styles.titleRow}>
              <View
                accessible
                accessibilityLabel={title}
                accessibilityRole="header"
                ref={titleRef}
                style={styles.title}
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
                <AppText color="text3" variant="monoSmall">
                  {subtitle}
                </AppText>
              )}
            </View>
          ) : (
            <View
              accessible
              accessibilityLabel={title}
              accessibilityRole="header"
              ref={titleRef}
              style={styles.hiddenTitle}
            >
              <AppText color="text" variant="headline" weight={600}>
                {title}
              </AppText>
            </View>
          )}
          <IconButton
            accessibilityLabel={closeLabel ?? common.a11y.close}
            icon="close"
            onPress={onClose}
            variant="raised"
          />
        </View>
        <View>{children}</View>
        {side ? null : (
          <View
            accessibilityElementsHidden
            importantForAccessibility="no-hide-descendants"
            style={[styles.caret, { left: caretX ?? panelWidth / 2 - 7 }]}
            testID={caretTestID}
          />
        )}
      </View>
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
  },
  backdrop: {
    position: 'absolute',
    top: 0,
    right: 0,
    bottom: 0,
    left: 0,
    backgroundColor: 'rgba(0, 0, 0, 0.32)',
  },
  panel: {
    position: 'absolute',
    borderRadius: radius.panel,
    borderWidth: 1,
    borderColor: tokens.panelLine,
    shadowColor: tokens.thumbBg,
    shadowOffset: { width: 0, height: 10 },
    shadowOpacity: 0.35,
    shadowRadius: 24,
    elevation: 20,
  },
  sidePanel: {
    top: 10,
    bottom: 10,
    paddingTop: 12,
    paddingHorizontal: space.lg,
    paddingBottom: 14,
    gap: space.sm,
    backgroundColor: tokens.panel,
  },
  popoverPanel: {
    bottom: 74,
    paddingTop: 12,
    paddingHorizontal: 14,
    paddingBottom: 4,
    gap: 10,
    backgroundColor: tokens.card,
  },
  header: {
    minHeight: 44,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
  },
  titleRow: {
    flex: 1,
    minWidth: 0,
    flexDirection: 'row',
    alignItems: 'baseline',
    gap: 10,
  },
  title: {
    flexShrink: 1,
  },
  hiddenTitle: {
    width: 1,
    height: 1,
    marginLeft: 'auto',
    opacity: 0,
  },
  caret: {
    position: 'absolute',
    bottom: -7,
    width: 14,
    height: 14,
    borderRightWidth: 1,
    borderBottomWidth: 1,
    borderColor: tokens.panelLine,
    backgroundColor: tokens.card,
    transform: [{ rotate: '45deg' }],
  },
});

export default SessionPanel;
