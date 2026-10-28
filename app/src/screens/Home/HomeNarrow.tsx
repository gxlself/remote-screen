import { useEffect, useMemo, useRef } from 'react';
import {
  ActivityIndicator,
  AccessibilityInfo,
  PixelRatio,
  Platform,
  RefreshControl,
  ScrollView,
  StyleSheet,
  View,
  useWindowDimensions,
} from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import AppText from '../../components/AppText';
import { DeviceCard } from '../../components/DeviceCard';
import { EmptyState } from '../../components/EmptyState';
import { ListGroup, SectionHeader } from '../../components/List';
import { useHostPath } from '../../features/directory/pathStore';
import {
  cardKey,
  type CardModel,
  type HomeCard,
} from '../../features/directory/cardModel';
import { common } from '../../i18n/zh/common';
import { home } from '../../i18n/zh/home';
import { ipadHome } from '../../i18n/zh/ipadHome';
import { isAndroid } from '../../theme/platform';
import { tokens } from '../../theme/tokens';
import { HomeBanners } from './HomeBanners';
import { HomeEmpty } from './HomeEmpty';
import { HomeHeader } from './HomeHeader';
import { LiveSessionBar } from './LiveSessionBar';
import { NativeCard } from './NativeCard';
import { SearchField } from './SearchField';
import { ServerRow } from './ServerRow';
import type { HomeScreenViewProps } from './useHomeModel';

// The tab bar sits below this screen, so the floating bar only clears the
// screen edge.
const LIVE_BAR_BOTTOM = 12;
const LIVE_BAR_SPACE = 84;
const CARD_GAP = 12;

type CardCellProps = {
  card: CardModel;
  liveSessionHostId?: string;
  width: number;
  busy: boolean;
  disabled: boolean;
  onPress(): void;
  onLongPress(): void;
};

function CardCell({
  card,
  liveSessionHostId,
  width,
  busy,
  disabled,
  onPress,
  onLongPress,
}: CardCellProps) {
  const hostPath = useHostPath(card.hostId);
  const name =
    card.path === undefined && liveSessionHostId === card.hostId
      ? ipadHome.a11y.liveCard(card.name)
      : card.name;

  return (
    <View style={{ width }}>
      <DeviceCard
        a11yPath={card.a11yPath}
        busy={busy}
        chip={card.chip}
        disabled={disabled}
        displays={card.displays}
        latencyMs={hostPath?.latencyMs ?? card.latencyMs}
        live={card.live}
        look={card.look}
        name={name}
        onLongPress={onLongPress}
        onPress={onPress}
        path={hostPath?.path ?? card.path}
        statusText={card.statusText}
        statusTone={card.statusTone}
        tcp={hostPath?.transport === 'tcp' || card.tcp}
        testID={`card-${card.hostId}`}
      />
    </View>
  );
}

export function HomeNarrow(props: HomeScreenViewProps) {
  const {
    actions,
    banners,
    busyKey = null,
    cards,
    emptyReason,
    headerStatus,
    loadError,
    isEmpty,
    isLoading,
    liveSession,
    onRefresh,
    query,
    refreshing,
    scrollRef,
    servers,
    setQuery,
  } = props;
  const insets = useSafeAreaInsets();
  const bottomSpace =
    liveSession === null ? 24 : LIVE_BAR_BOTTOM + LIVE_BAR_SPACE;
  const window = useWindowDimensions();
  const fontScale = PixelRatio.getFontScale();
  const singleColumn = fontScale > 1.3;
  const contentWidth = window.width - 40;
  const cardWidth = singleColumn ? contentWidth : (contentWidth - CARD_GAP) / 2;
  const rows = useMemo(() => {
    const next: HomeCard[][] = [];
    const perRow = singleColumn ? 1 : 2;
    for (let index = 0; index < cards.length; index += perRow) {
      next.push(cards.slice(index, index + perRow));
    }
    return next;
  }, [cards, singleColumn]);
  const previousCardCount = useRef(cards.length);

  useEffect(() => {
    const becameEmpty =
      previousCardCount.current > 0 &&
      cards.length === 0 &&
      query.trim() !== '';
    if (becameEmpty && Platform.OS === 'ios') {
      AccessibilityInfo.announceForAccessibility(home.computer.noMatch);
    }
    previousCardCount.current = cards.length;
  }, [cards.length, query]);

  const empty = (
    <HomeEmpty
      onAdd={actions.add}
      onInstallHost={actions.installHost}
      onOpenTailscale={actions.openTailscale}
      reason={emptyReason}
    />
  );

  return (
    <View style={styles.screen}>
      <ScrollView
        contentContainerStyle={[
          styles.content,
          { paddingTop: insets.top + 8, paddingBottom: bottomSpace },
        ]}
        keyboardDismissMode="on-drag"
        keyboardShouldPersistTaps="handled"
        ref={scrollRef}
        refreshControl={
          <RefreshControl
            colors={isAndroid() ? [tokens.accent] : undefined}
            onRefresh={onRefresh}
            progressBackgroundColor={isAndroid() ? tokens.raised : undefined}
            refreshing={refreshing}
            tintColor={tokens.text2}
          />
        }
        showsVerticalScrollIndicator={false}
        testID="home-scroll"
      >
        <HomeHeader
          onAdd={actions.add}
          onSettings={actions.openSettings}
          status={headerStatus}
        />
        <SearchField query={query} setQuery={setQuery} />

        <HomeBanners banners={banners} />

        {isLoading ? (
          <View style={styles.loading}>
            <ActivityIndicator
              accessibilityLabel={common.loading}
              color={tokens.text2}
            />
          </View>
        ) : isEmpty ? (
          empty
        ) : loadError !== null && cards.length === 0 ? (
          <EmptyState
            action={{ label: common.retry, onPress: onRefresh }}
            icon="alert"
            title={home.empty.loadFailedTitle}
          />
        ) : (
          <>
            <View style={styles.section}>
              <View
                accessibilityActions={[
                  { name: 'refresh', label: ipadHome.a11y.refresh },
                ]}
                accessibilityLabel={home.sections.computers}
                accessibilityRole="header"
                accessible
                onAccessibilityAction={event => {
                  if (event.nativeEvent.actionName === 'refresh') {
                    onRefresh();
                  }
                }}
              >
                <SectionHeader
                  right={
                    cards.length === 0 ? undefined : home.sections.longPress
                  }
                  title={home.sections.computers}
                />
              </View>
              {cards.length === 0 && query.trim() === '' ? (
                empty
              ) : cards.length === 0 ? (
                <AppText
                  accessibilityLiveRegion={
                    Platform.OS === 'android' ? 'polite' : undefined
                  }
                  color="text3"
                  variant="body"
                >
                  {home.computer.noMatch}
                </AppText>
              ) : (
                <View style={styles.grid}>
                  {rows.map(row => (
                    <View
                      key={row[0] === undefined ? 'row' : cardKey(row[0])}
                      style={styles.gridRow}
                    >
                      {row.map(card =>
                        card.kind === 'native' ? (
                          <View key={card.key} style={{ width: cardWidth }}>
                            <NativeCard
                              busy={busyKey === card.key}
                              card={card}
                              disabled={busyKey !== null}
                              onLongPress={() => actions.longPressCard(card)}
                              onPress={() => actions.openCard(card)}
                              testID={`card-${card.key}`}
                            />
                          </View>
                        ) : (
                          <CardCell
                            busy={busyKey === card.hostId}
                            card={card}
                            disabled={busyKey !== null}
                            key={card.hostId}
                            onLongPress={() => actions.longPressCard(card)}
                            onPress={() => actions.openCard(card)}
                            liveSessionHostId={liveSession?.hostId}
                            width={cardWidth}
                          />
                        ),
                      )}
                    </View>
                  ))}
                </View>
              )}
            </View>

            {servers.length === 0 ? null : (
              <View style={styles.section}>
                <SectionHeader
                  right={home.sections.serversRight(servers.length)}
                  title={home.sections.servers}
                />
                <ListGroup>
                  {servers.map(server => (
                    <ServerRow
                      key={server.serverId}
                      onPress={() => actions.openServer(server)}
                      server={server}
                    />
                  ))}
                </ListGroup>
              </View>
            )}
          </>
        )}
      </ScrollView>

      {liveSession === null ? null : (
        <LiveSessionBar
          bottom={LIVE_BAR_BOTTOM}
          onPress={actions.backToSession}
          session={liveSession}
        />
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    backgroundColor: tokens.bg,
  },
  content: {
    paddingHorizontal: 20,
    gap: 16,
  },
  loading: {
    minHeight: 240,
    alignItems: 'center',
    justifyContent: 'center',
  },
  section: {
    gap: 12,
  },
  grid: {
    gap: CARD_GAP,
  },
  gridRow: {
    flexDirection: 'row',
    gap: CARD_GAP,
  },
});
