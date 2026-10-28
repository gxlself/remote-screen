import { useEffect, useRef, type RefObject } from 'react';
import {
  AccessibilityInfo,
  Platform,
  Pressable,
  RefreshControl,
  ScrollView,
  StyleSheet,
  View,
} from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import AppText from '../../../components/AppText';
import { IconButton } from '../../../components/IconButton';
import { SectionHeader } from '../../../components/List';
import { StatusChip } from '../../../components/StatusChip';
import { Icon } from '../../../components/icons/Icon';
import type { HomeActions, HomeScreenViewProps } from '../useHomeModel';
import type { ScrollViewHandle } from '../useTabScrollToTop';
import { cardKey, type HomeCard } from '../../../features/directory/cardModel';
import { wideRowSubtitle } from '../../../features/directory/wideRows';
import { ipadHome } from '../../../i18n/zh/ipadHome';
import { native } from '../../../i18n/zh/native';
import { isAndroid, press } from '../../../theme/platform';
import { tokens } from '../../../theme/tokens';
import { SearchField } from '../SearchField';
import { ServerRow } from '../ServerRow';

const CONTENT_TOP = 20;

export type SidebarProps = {
  cards: HomeCard[];
  headerStatus: HomeScreenViewProps['headerStatus'];
  query: string;
  selectedId: string | null;
  servers: HomeScreenViewProps['servers'];
  setQuery(q: string): void;
  actions: Pick<HomeActions, 'add' | 'longPressCard' | 'openServer'>;
  /** Called with `cardKey(card)`. */
  onSelect(key: string): void;
  onRefresh?(): void;
  refreshing?: boolean;
  scrollRef?: RefObject<ScrollViewHandle | null>;
};

type ComputerRowProps = {
  card: HomeCard;
  selected: boolean;
  onPress(): void;
  onLongPress(): void;
};

function ComputerRow({
  card,
  selected,
  onPress,
  onLongPress,
}: ComputerRowProps) {
  const subtitle =
    card.kind === 'native'
      ? `${card.subtitle}${ipadHome.separator}${card.reachText}`
      : wideRowSubtitle(card);
  const spokenSubtitle =
    card.kind === 'native'
      ? native.a11y.card([
          card.subtitle,
          card.reach === 'unknown' ? native.a11y.reachUnknown : card.reachText,
          card.badge,
        ])
      : subtitle;

  return (
    <Pressable
      accessibilityHint={ipadHome.sidebar.rowHint}
      accessibilityActions={[
        { name: 'longpress', label: ipadHome.a11y.details },
      ]}
      accessibilityLabel={ipadHome.sidebar.rowLabel(card.name, spokenSubtitle)}
      accessibilityRole="button"
      accessibilityState={{ selected }}
      // 同 ripple.neutral
      android_ripple={
        Platform.OS === 'android'
          ? { color: 'rgba(255, 255, 255, 0.12)' }
          : undefined
      }
      onAccessibilityAction={event => {
        if (event.nativeEvent.actionName === 'longpress') {
          onLongPress();
        }
      }}
      onLongPress={onLongPress}
      onPress={onPress}
      style={({ pressed }) => [
        styles.computerRow,
        selected ? styles.computerRowSelected : null,
        pressed && !selected && !isAndroid() ? styles.computerRowPressed : null,
      ]}
      testID={`wide-computer-${cardKey(card)}`}
    >
      <View style={styles.computerIcon}>
        <Icon
          color="text2"
          name={card.look === 'offline' ? 'computerOff' : 'computer'}
          size={20}
        />
      </View>
      <View style={styles.computerCopy}>
        <AppText variant="bodyStrong" weight={600}>
          {card.name}
        </AppText>
        {subtitle === '' ? null : (
          <AppText color="text3" variant="caption">
            {subtitle}
          </AppText>
        )}
      </View>
      {card.kind === 'native' ? (
        <StatusChip label={card.badge} size="sm" tone="accent" />
      ) : null}
    </Pressable>
  );
}

export function Sidebar({
  actions,
  cards,
  headerStatus,
  onRefresh,
  onSelect,
  query,
  refreshing = false,
  scrollRef,
  selectedId,
  servers,
  setQuery,
}: SidebarProps) {
  const insets = useSafeAreaInsets();
  const previousCardCount = useRef(cards.length);

  useEffect(() => {
    const becameEmpty =
      previousCardCount.current > 0 &&
      cards.length === 0 &&
      query.trim() !== '';
    if (becameEmpty && Platform.OS === 'ios') {
      AccessibilityInfo.announceForAccessibility(ipadHome.empty.noMatch);
    }
    previousCardCount.current = cards.length;
  }, [cards.length, query]);

  return (
    <View style={styles.sidebar} testID="wide-sidebar">
      <ScrollView
        contentContainerStyle={[
          styles.content,
          { paddingTop: insets.top + CONTENT_TOP },
        ]}
        keyboardDismissMode="on-drag"
        keyboardShouldPersistTaps="handled"
        ref={scrollRef}
        refreshControl={
          onRefresh === undefined ? undefined : (
            <RefreshControl
              colors={isAndroid() ? [tokens.accent] : undefined}
              onRefresh={onRefresh}
              progressBackgroundColor={isAndroid() ? tokens.raised : undefined}
              refreshing={refreshing}
              tintColor={tokens.text2}
            />
          )
        }
        showsVerticalScrollIndicator={false}
        testID="wide-sidebar-scroll"
      >
        <View style={styles.header}>
          <View style={styles.headerCopy}>
            <AppText
              accessibilityRole="header"
              style={styles.title}
              weight={700}
            >
              {ipadHome.title}
            </AppText>
            <AppText color="text3" variant="caption">
              {headerStatus.text}
            </AppText>
          </View>
          <IconButton
            accessibilityLabel={ipadHome.a11y.add}
            icon="plus"
            onPress={actions.add}
            size={36}
            variant="raised"
          />
        </View>

        <SearchField query={query} setQuery={setQuery} />

        <View style={styles.section}>
          <SectionHeader title={ipadHome.sidebar.computers} />
          {cards.length === 0 ? (
            <AppText
              accessibilityLiveRegion={
                Platform.OS === 'android' ? 'polite' : undefined
              }
              color="text3"
              variant="caption"
            >
              {query.trim() === ''
                ? native.home.emptyTitle
                : ipadHome.empty.noMatch}
            </AppText>
          ) : (
            <View style={styles.rows}>
              {cards.map(card => (
                <ComputerRow
                  card={card}
                  key={cardKey(card)}
                  onLongPress={() => actions.longPressCard(card)}
                  onPress={() => onSelect(cardKey(card))}
                  selected={cardKey(card) === selectedId}
                />
              ))}
            </View>
          )}
        </View>

        {servers.length === 0 ? null : (
          <View style={styles.section}>
            <SectionHeader
              right={ipadHome.sidebar.serverCount(servers.length)}
              title={ipadHome.sidebar.servers}
            />
            <View style={styles.serverList}>
              {servers.map(server => (
                <ServerRow
                  key={server.serverId}
                  onPress={() => actions.openServer(server)}
                  server={server}
                />
              ))}
            </View>
          </View>
        )}
      </ScrollView>
    </View>
  );
}

const styles = StyleSheet.create({
  sidebar: {
    width: 320,
    backgroundColor: tokens.card,
    borderRightWidth: 1,
    borderRightColor: tokens.cardLine,
  },
  content: {
    paddingHorizontal: 16,
    paddingBottom: 20,
    gap: 14,
  },
  header: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    justifyContent: 'space-between',
    gap: 12,
  },
  headerCopy: {
    flex: 1,
    gap: 2,
  },
  title: {
    fontSize: 22,
    lineHeight: 28,
  },
  section: {
    gap: 8,
  },
  rows: {
    gap: 4,
  },
  computerRow: {
    minHeight: 56,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    borderRadius: 12,
    paddingVertical: 10,
    paddingHorizontal: 12,
  },
  computerRowSelected: {
    backgroundColor: tokens.selected,
  },
  computerRowPressed: {
    backgroundColor: press.highlight,
  },
  computerIcon: {
    width: 36,
    height: 36,
    borderRadius: 10,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: tokens.raised,
  },
  computerCopy: {
    flex: 1,
    gap: 1,
  },
  serverList: {
    borderRadius: 12,
    overflow: 'hidden',
    backgroundColor: tokens.bg,
  },
});
