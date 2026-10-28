import type { RefObject } from 'react';
import { type HostInstance, StyleSheet, View } from 'react-native';
import AppText from '../../../components/AppText';
import { Button } from '../../../components/Button';
import { IconButton } from '../../../components/IconButton';
import { StatusChip } from '../../../components/StatusChip';
import { Icon } from '../../../components/icons/Icon';
import type { NativeCardModel } from '../../../features/directory/cardModel';
import { common } from '../../../i18n/zh/common';
import { ipadHome } from '../../../i18n/zh/ipadHome';
import { native } from '../../../i18n/zh/native';
import { tokens } from '../../../theme/tokens';
import type { HomeActions } from '../useHomeModel';

export type NativeDetailPaneProps = {
  card: NativeCardModel;
  titleRef?: RefObject<HostInstance | null>;
  actions: Pick<HomeActions, 'longPressCard' | 'openCard'>;
  /** This computer's connection is starting. */
  busy?: boolean;
  /** Another computer is connecting. */
  disabled?: boolean;
};

const PREVIEW_ASPECT = 16 / 10;

/** Wide detail for a computer without Host: identity, reach and Connect. */
export function NativeDetailPane({
  actions,
  busy = false,
  card,
  disabled = false,
  titleRef,
}: NativeDetailPaneProps) {
  const offline = card.look === 'offline';
  const spokenReach =
    card.reach === 'unknown' ? native.a11y.reachUnknown : card.reachText;

  return (
    <View style={styles.pane} testID="wide-native-pane">
      <View style={styles.header}>
        <View style={styles.headerCopy}>
          <View style={styles.titleRow}>
            <View
              accessibilityLabel={card.name}
              accessibilityRole="header"
              accessible
              ref={titleRef}
            >
              <AppText style={styles.title} variant="title">
                {card.name}
              </AppText>
            </View>
            <StatusChip label={card.badge} tone="accent" />
          </View>
          <AppText color="text3" variant="footnote">
            {card.subtitle}
          </AppText>
          {card.address === '' ? null : (
            <AppText color="text3" mono variant="monoSmall">
              {card.address}
            </AppText>
          )}
        </View>
        <IconButton
          accessibilityLabel={ipadHome.a11y.more}
          icon="more"
          onPress={() => actions.longPressCard(card)}
          size={44}
          variant="raised"
        />
      </View>

      <View style={styles.preview}>
        <View style={styles.previewCanvas}>
          <View style={styles.previewIcon}>
            <Icon
              color="muted"
              name={offline ? 'computerOff' : 'computer'}
              size={48}
            />
          </View>
          <View style={styles.previewBar}>
            <View
              accessibilityLabel={spokenReach}
              accessible
              style={styles.previewBarCopy}
            >
              <AppText
                color={card.reach === 'online' ? 'good' : 'text3'}
                variant="caption"
              >
                {card.reachText}
              </AppText>
            </View>
            <Button
              disabled={disabled && !busy}
              loading={busy}
              onPress={() => actions.openCard(card)}
              testID="wide-native-connect"
              title={common.connect}
              variant="pill"
            />
          </View>
        </View>
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  pane: {
    gap: 16,
  },
  header: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    justifyContent: 'space-between',
    gap: 16,
  },
  headerCopy: {
    flex: 1,
    gap: 4,
  },
  titleRow: {
    flexDirection: 'row',
    alignItems: 'center',
    flexWrap: 'wrap',
    gap: 10,
  },
  title: {
    fontSize: 28,
    lineHeight: 34,
  },
  preview: {
    width: '100%',
    maxWidth: 810,
    alignSelf: 'center',
  },
  previewCanvas: {
    width: '100%',
    aspectRatio: PREVIEW_ASPECT,
    borderRadius: 18,
    borderWidth: 1,
    borderColor: tokens.cardLine,
    backgroundColor: tokens.thumbBg,
    overflow: 'hidden',
  },
  previewIcon: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
  },
  previewBar: {
    position: 'absolute',
    left: 0,
    right: 0,
    bottom: 0,
    minHeight: 64,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 12,
    backgroundColor: tokens.hud,
    paddingHorizontal: 16,
    paddingVertical: 10,
  },
  previewBarCopy: {
    flex: 1,
  },
});
