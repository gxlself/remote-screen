import {
  PixelRatio,
  Platform,
  Pressable,
  type HostInstance,
  StyleSheet,
  View,
  useWindowDimensions,
} from 'react-native';
import { useState, type RefObject } from 'react';
import AppText from '../../../components/AppText';
import { Button } from '../../../components/Button';
import { IconButton } from '../../../components/IconButton';
import { PathLabel } from '../../../components/PathMark';
import { Icon } from '../../../components/icons/Icon';
import type { CardModel } from '../../../features/directory/cardModel';
import { useHostPath } from '../../../features/directory/pathStore';
import { addressLine } from '../../../features/directory/wideRows';
import { useHistory } from '../../../features/sessions/history';
import type { Computer } from '../../../native/rscore/types';
import type { HomeActions } from '../useHomeModel';
import { ipadHome } from '../../../i18n/zh/ipadHome';
import { tokens } from '../../../theme/tokens';
import { detailInfoCells, InfoGrid } from './InfoGrid';

export type DetailPaneProps = {
  card: CardModel;
  computer: Computer;
  titleRef?: RefObject<HostInstance | null>;
  actions: Pick<HomeActions, 'longPressCard' | 'openCard' | 'openTerminal'>;
  /** This computer's connection is starting. */
  busy?: boolean;
  /** Another computer is connecting. */
  disabled?: boolean;
};

const PREVIEW_ASPECT = 16 / 10;
const INFO_TWO_COLUMN_WIDTH = 900;
/** Below this pane width the title and its buttons stack (iPad mini, 2/3 split). */
const HEADER_ROW_WIDTH = 560;

function isLiveCard(card: CardModel): boolean {
  return card.group === 0;
}

export function DetailPane({
  actions,
  busy = false,
  card,
  computer,
  disabled = false,
  titleRef,
}: DetailPaneProps) {
  const nowUs = Date.now() * 1000;
  const { width } = useWindowDimensions();
  const [paneWidth, setPaneWidth] = useState<number | null>(null);
  const largeFont = PixelRatio.getFontScale() > 2;
  const stacked =
    largeFont || (paneWidth !== null && paneWidth < HEADER_ROW_WIDTH);
  const hostPath = useHostPath(card.hostId);
  const history = useHistory();
  const live = isLiveCard(card);
  const online = computer.online && computer.hostRunning;
  const path = card.path;
  const primaryDisplay = computer.displays.find(display => display.primary);
  const displayIndex = primaryDisplay?.id ?? 1;
  const cells = detailInfoCells({
    computer,
    hostPath,
    history,
    nowUs,
  });

  const statusText = online
    ? ipadHome.detail.online
    : card.statusText ?? ipadHome.detail.offline;
  // Tapping the picture does what its button says, so it only works where
  // there is a button: never for an offline computer.
  const previewTappable = live || computer.online;
  const connect = () => actions.openCard(card);

  return (
    <View
      onLayout={event => setPaneWidth(event.nativeEvent.layout.width)}
      style={styles.pane}
      testID="wide-detail-pane"
    >
      <View style={[styles.header, stacked ? styles.headerLarge : null]}>
        <View style={styles.headerCopy}>
          <View style={styles.titleRow}>
            <View
              accessibilityLabel={computer.name}
              accessibilityRole="header"
              accessible
              ref={titleRef}
            >
              <AppText style={styles.title} variant="title">
                {computer.name}
              </AppText>
            </View>
            {path === undefined ? null : (
              <PathLabel kind={path} latencyMs={card.latencyMs} />
            )}
          </View>
          <AppText color="text3" mono variant="monoSmall">
            {addressLine(computer)}
          </AppText>
        </View>
        <View
          style={[
            styles.headerActions,
            stacked ? styles.headerActionsLarge : null,
          ]}
          testID="wide-detail-actions"
        >
          <Button
            icon="terminal"
            onPress={() => actions.openTerminal(card.hostId)}
            title={ipadHome.detail.terminal}
            variant="secondary"
          />
          <IconButton
            accessibilityLabel={ipadHome.a11y.more}
            icon="more"
            onPress={() => actions.longPressCard(card)}
            size={44}
            variant="raised"
          />
        </View>
      </View>

      <Pressable
        // 同 ripple.neutral
        android_ripple={
          Platform.OS === 'android'
            ? { color: 'rgba(255, 255, 255, 0.12)' }
            : undefined
        }
        accessible={false}
        accessibilityLabel={ipadHome.detail.previewLabel(
          computer.name,
          displayIndex,
        )}
        accessibilityRole="button"
        disabled={!previewTappable || busy || disabled}
        onPress={connect}
        style={styles.preview}
        testID="wide-preview"
      >
        <View style={styles.previewCanvas}>
          <View style={styles.previewIcon}>
            <Icon color="muted" name="computer" size={48} />
          </View>
          <View style={styles.displayPill}>
            <View
              style={[
                styles.dot,
                { backgroundColor: online ? tokens.good : tokens.muted },
              ]}
            />
            <AppText variant="caption" weight={500}>
              {ipadHome.detail.displayPill(displayIndex)}
            </AppText>
          </View>
          <View style={styles.previewBar}>
            {live ? (
              <>
                <View style={styles.previewBarCopy} />
                <Button
                  onPress={connect}
                  title={ipadHome.detail.backToSession}
                  variant="pill"
                />
              </>
            ) : online ? (
              <>
                <View style={styles.previewBarCopy} />
                <Button
                  disabled={disabled && !busy}
                  loading={busy}
                  onPress={connect}
                  testID="wide-control"
                  title={ipadHome.detail.control}
                  variant="pill"
                />
              </>
            ) : computer.online ? (
              // Host is not running; Screen Sharing / Remote Desktop still is.
              <>
                <View style={styles.previewBarCopy}>
                  <AppText color="text3" variant="caption">
                    {statusText}
                  </AppText>
                </View>
                <Button
                  disabled={disabled && !busy}
                  loading={busy}
                  onPress={connect}
                  testID="wide-connect"
                  title={ipadHome.detail.connect}
                  variant="pill"
                />
              </>
            ) : (
              <AppText color="text3" variant="caption">
                {statusText}
              </AppText>
            )}
          </View>
        </View>
      </Pressable>

      <InfoGrid
        cells={cells}
        narrow={largeFont || width < INFO_TWO_COLUMN_WIDTH}
      />
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
  headerLarge: {
    flexDirection: 'column',
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
  headerActions: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
  },
  headerActionsLarge: {
    alignSelf: 'stretch',
    flexWrap: 'wrap',
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
    alignItems: 'center',
    justifyContent: 'center',
  },
  previewIcon: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
  },
  displayPill: {
    position: 'absolute',
    top: 14,
    left: 14,
    minHeight: 28,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    borderRadius: 14,
    backgroundColor: tokens.hud,
    paddingHorizontal: 12,
  },
  dot: {
    width: 6,
    height: 6,
    borderRadius: 3,
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
    gap: 2,
  },
});
