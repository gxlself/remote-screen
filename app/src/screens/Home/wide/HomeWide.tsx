import { useEffect, useMemo, useRef, useState } from 'react';
import {
  AccessibilityInfo,
  ActivityIndicator,
  ScrollView,
  StyleSheet,
  View,
  type HostInstance,
} from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { EmptyState } from '../../../components/EmptyState';
import { cardKey, type HomeCard } from '../../../features/directory/cardModel';
import { useDirectory } from '../../../features/data/queries';
import { ipadHome } from '../../../i18n/zh/ipadHome';
import { common } from '../../../i18n/zh/common';
import { tokens } from '../../../theme/tokens';
import { HomeBanners } from '../HomeBanners';
import { HomeEmpty } from '../HomeEmpty';
import { LiveSessionBar } from '../LiveSessionBar';
import type { HomeScreenViewProps } from '../useHomeModel';
import { DetailPane } from './DetailPane';
import { NativeDetailPane } from './NativeDetailPane';
import { Sidebar } from './Sidebar';

const CONTENT_TOP = 20;
const LIVE_BAR_BOTTOM = 20;
const LIVE_BAR_SPACE = 84;

/** Same choice as `pickDefault`, over Host and native cards. */
function defaultSelection(
  cards: HomeCard[],
  liveHostId: string | null,
): string | null {
  const live =
    liveHostId === null
      ? undefined
      : cards.find(
          card => card.kind !== 'native' && card.hostId === liveHostId,
        );
  const card =
    live ?? cards.find(candidate => candidate.look !== 'offline') ?? cards[0];
  return card === undefined ? null : cardKey(card);
}

export function HomeWide(props: HomeScreenViewProps) {
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
  // Keeps the last info row above the floating session bar.
  const bottomSpace =
    liveSession === null ? 24 : LIVE_BAR_BOTTOM + LIVE_BAR_SPACE;
  const directory = useDirectory();
  const [selectedId, setSelectedId] = useState<string | null>(() =>
    defaultSelection(cards, liveSession?.hostId ?? null),
  );
  const titleRef = useRef<HostInstance | null>(null);
  const selectedByUser = useRef(false);
  // Searched the same way as the phone layout (`entrySearchFields`).
  const wideCards = useMemo<HomeCard[]>(() => {
    const livePath = liveSession?.path ?? null;
    if (livePath === null) {
      return cards;
    }
    return cards.map(card =>
      card.kind !== 'native' &&
      card.hostId === liveSession?.hostId &&
      card.path === undefined
        ? { ...card, path: livePath }
        : card,
    );
  }, [cards, liveSession]);

  useEffect(() => {
    if (wideCards.some(card => cardKey(card) === selectedId)) {
      return;
    }
    setSelectedId(wideCards[0] === undefined ? null : cardKey(wideCards[0]));
  }, [selectedId, wideCards]);

  const selectCard = (key: string) => {
    if (key === selectedId) {
      return;
    }
    selectedByUser.current = true;
    setSelectedId(key);
  };

  useEffect(() => {
    if (!selectedByUser.current || selectedId === null) {
      return;
    }
    selectedByUser.current = false;
    const timer = setTimeout(() => {
      const target = titleRef.current;
      if (target === null) {
        return;
      }
      AccessibilityInfo.sendAccessibilityEvent(target, 'focus');
    }, 0);
    return () => clearTimeout(timer);
  }, [selectedId]);

  const selected = useMemo(
    () => wideCards.find(card => cardKey(card) === selectedId) ?? null,
    [selectedId, wideCards],
  );
  const selectedHostId =
    selected === null || selected.kind === 'native' ? null : selected.hostId;
  const computer = useMemo(
    () =>
      directory.directory?.computers.find(
        candidate => candidate.hostId === selectedHostId,
      ) ?? null,
    [directory.directory, selectedHostId],
  );

  return (
    <View style={styles.screen} testID="home-wide">
      <Sidebar
        actions={actions}
        cards={wideCards}
        headerStatus={headerStatus}
        onRefresh={onRefresh}
        onSelect={selectCard}
        query={query}
        refreshing={refreshing}
        scrollRef={scrollRef}
        selectedId={selectedId}
        servers={servers}
        setQuery={setQuery}
      />
      <View style={styles.mainWrap}>
        <ScrollView
          contentContainerStyle={[
            styles.mainContent,
            {
              paddingTop: insets.top + CONTENT_TOP,
              paddingBottom: bottomSpace,
            },
          ]}
          keyboardShouldPersistTaps="handled"
          showsVerticalScrollIndicator={false}
          style={styles.main}
          testID="wide-main"
        >
          <HomeBanners banners={banners} />
          {isLoading && cards.length === 0 ? (
            <View style={styles.loading}>
              <ActivityIndicator
                accessibilityLabel={common.loading}
                color={tokens.text2}
              />
            </View>
          ) : isEmpty ? (
            <HomeEmpty
              onAdd={actions.add}
              onInstallHost={actions.installHost}
              onOpenTailscale={actions.openTailscale}
              reason={emptyReason}
            />
          ) : loadError !== null && cards.length === 0 ? (
            <EmptyState
              action={{ label: common.retry, onPress: onRefresh }}
              icon="alert"
              title={ipadHome.empty.loadFailedTitle}
            />
          ) : cards.length === 0 && query.trim() === '' ? (
            <HomeEmpty
              onAdd={actions.add}
              onInstallHost={actions.installHost}
            />
          ) : selected?.kind === 'native' ? (
            <NativeDetailPane
              actions={actions}
              busy={busyKey === selected.key}
              card={selected}
              disabled={busyKey !== null}
              titleRef={titleRef}
            />
          ) : selected === null || computer === null ? (
            <EmptyState icon="computer" title={ipadHome.empty.noMatch} />
          ) : (
            <DetailPane
              actions={actions}
              busy={busyKey === selected.hostId}
              card={selected}
              computer={computer}
              disabled={busyKey !== null}
              titleRef={titleRef}
            />
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
    </View>
  );
}

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    flexDirection: 'row',
    backgroundColor: tokens.bg,
  },
  mainWrap: {
    flex: 1,
  },
  main: {
    flex: 1,
  },
  mainContent: {
    paddingHorizontal: 32,
    gap: 16,
  },
  loading: {
    minHeight: 240,
    alignItems: 'center',
    justifyContent: 'center',
  },
});
