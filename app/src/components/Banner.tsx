import {
  AccessibilityInfo,
  PixelRatio,
  Platform,
  StyleSheet,
  View,
} from 'react-native';
import { useEffect } from 'react';
import { common } from '../i18n/zh/common';
import { tokens } from '../theme/tokens';
import AppText from './AppText';
import { Button } from './Button';
import { IconButton } from './IconButton';
import { Icon, type IconName } from './icons/Icon';

export type BannerTone = 'warn' | 'good' | 'info' | 'bad';

export type BannerAction = {
  label: string;
  onPress: () => void;
  disabled?: boolean;
  loading?: boolean;
};

export type BannerProps = {
  tone: BannerTone;
  title: string;
  message?: string;
  icon?: IconName;
  action?: BannerAction;
  // Adds a close button; the caller hides the banner (and remembers that).
  onDismiss?: () => void;
  announce?: boolean;
  testID?: string;
};

type ToneSpec = {
  backgroundColor: string;
  borderColor: string;
  titleColor: 'text' | 'good';
  iconBackgroundColor: string;
  iconColor: string;
};

const toneSpecs = {
  warn: {
    backgroundColor: 'rgba(244,178,78,0.1)',
    borderColor: 'rgba(244,178,78,0.3)',
    titleColor: 'text',
    iconBackgroundColor: tokens.relaySoft,
    iconColor: tokens.relay,
  },
  good: {
    backgroundColor: 'rgba(93,211,154,0.08)',
    borderColor: 'rgba(93,211,154,0.28)',
    titleColor: 'good',
    iconBackgroundColor: 'rgba(93,211,154,0.16)',
    iconColor: tokens.good,
  },
  info: {
    backgroundColor: tokens.accentTint,
    borderColor: 'rgba(125,168,255,0.3)',
    titleColor: 'text',
    iconBackgroundColor: tokens.accentSoft,
    iconColor: tokens.accent,
  },
  bad: {
    backgroundColor: 'rgba(255,136,115,0.1)',
    borderColor: 'rgba(255,136,115,0.3)',
    titleColor: 'text',
    iconBackgroundColor: tokens.badSoft,
    iconColor: tokens.bad,
  },
} as const satisfies Record<BannerTone, ToneSpec>;

export function Banner({
  tone,
  title,
  message,
  icon,
  action,
  onDismiss,
  announce = true,
  testID,
}: BannerProps) {
  const spec = toneSpecs[tone];
  const accessibilityLabel = message ? `${title}，${message}` : title;
  // With a button inside, the copy is its own stop instead of the whole card.
  const hasControls = action != null || onDismiss != null;
  const largeAction = action != null && PixelRatio.getFontScale() > 2;

  useEffect(() => {
    if (Platform.OS === 'ios' && announce) {
      AccessibilityInfo.announceForAccessibility(accessibilityLabel);
    }
  }, [accessibilityLabel, announce]);

  return (
    <View
      {...(hasControls
        ? {}
        : {
            accessible: true,
            accessibilityLabel,
            accessibilityLiveRegion: 'polite' as const,
            role: 'status' as const,
          })}
      style={[
        styles.container,
        {
          backgroundColor: spec.backgroundColor,
          borderColor: spec.borderColor,
        },
        largeAction ? styles.largeActionContainer : null,
      ]}
      testID={testID}
    >
      {icon == null ? null : (
        <View
          {...(hasControls
            ? {
                importantForAccessibility: 'no-hide-descendants' as const,
                accessibilityElementsHidden: true,
              }
            : {})}
          style={[
            styles.icon,
            largeAction ? styles.largeActionIcon : null,
            { backgroundColor: spec.iconBackgroundColor },
          ]}
          testID={testID == null ? undefined : `${testID}-icon-wrap`}
        >
          <Icon
            color={spec.iconColor}
            name={icon}
            size={18}
            testID={testID == null ? undefined : `${testID}-icon`}
          />
        </View>
      )}
      <View
        {...(hasControls
          ? {
              accessible: true,
              accessibilityLabel,
              accessibilityLiveRegion: 'polite' as const,
              role: 'status' as const,
            }
          : {})}
        {...(hasControls && testID ? { testID: `${testID}-copy` } : {})}
        style={styles.copy}
      >
        <AppText color={spec.titleColor} variant="button">
          {title}
        </AppText>
        {message == null ? null : (
          <AppText color="text2" variant="caption">
            {message}
          </AppText>
        )}
      </View>
      {action == null ? null : (
        <Button
          disabled={action.disabled}
          fullWidth={largeAction}
          loading={action.loading}
          onPress={action.onPress}
          title={action.label}
          variant="primary"
          testID={testID == null ? undefined : `${testID}-action`}
        />
      )}
      {onDismiss == null ? null : (
        <View style={largeAction ? styles.largeActionDismiss : null}>
          <IconButton
            accessibilityLabel={common.a11y.dismissBanner}
            icon="close"
            onPress={onDismiss}
            size={36}
            testID={testID == null ? undefined : `${testID}-dismiss`}
            variant="plain"
          />
        </View>
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    minHeight: 64,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 12,
    borderRadius: 16,
    borderWidth: 1,
    paddingTop: 12,
    paddingRight: 12,
    paddingBottom: 12,
    paddingLeft: 14,
  },
  icon: {
    width: 32,
    height: 32,
    borderRadius: 16,
    alignItems: 'center',
    justifyContent: 'center',
    flexShrink: 0,
  },
  largeActionContainer: {
    flexDirection: 'column',
    alignItems: 'stretch',
  },
  largeActionIcon: {
    alignSelf: 'flex-start',
  },
  largeActionDismiss: {
    alignSelf: 'flex-end',
  },
  copy: {
    flex: 1,
    gap: 2,
  },
});
