import { useRef, useState } from 'react';
import {
  Platform,
  Pressable,
  ScrollView,
  StyleSheet,
  View,
} from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
import type { TabScreenProps } from '../../app/navigation/routes';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import Dialog from '../../components/Dialog';
import { EmptyState } from '../../components/EmptyState';
import { SectionHeader } from '../../components/List';
import { Toast, useToast } from '../../components/Toast';
import { useDisconnectCollapsed } from '../../features/adapters/useAdapterLiveKeeper';
import { FallbackGuide } from '../../features/fallback/FallbackGuide';
import { useComputerEntries } from '../../features/native/useComputerEntries';
import { useConnect } from '../../features/native/useConnect';
import { clearHistory } from '../../features/sessions/history';
import { useKv } from '../../features/storage/StorageContext';
import { common } from '../../i18n/zh/common';
import { sessions } from '../../i18n/zh/sessions';
import { useCore } from '../../native/rscore/CoreContext';
import { press } from '../../theme/platform';
import { hit, space, tokens } from '../../theme/tokens';
import {
  useTabScrollToTop,
  type ScrollViewHandle,
} from '../Home/useTabScrollToTop';
import HistoryList from './HistoryList';
import LiveRemoteCard from './LiveRemoteCard';
import LiveSshList from './LiveSshList';
import { useSessionsModel } from './useSessionsModel';
import type {
  AdapterSessionItem,
  HistoryItem,
  RemoteSessionItem,
  SshSessionItem,
} from './useSessionsModel';

function isAdapterItem(
  item: RemoteSessionItem | AdapterSessionItem,
): item is AdapterSessionItem {
  return 'params' in item;
}

/** 最近 shows this many until the user asks for the rest. */
const HISTORY_PREVIEW = 10;

export default function SessionsScreen({
  navigation,
}: TabScreenProps<'Sessions'>) {
  const core = useCore();
  const kv = useKv();
  const model = useSessionsModel();
  const entries = useComputerEntries();
  const { connect, guide, closeGuide } = useConnect();
  const disconnectCollapsed = useDisconnectCollapsed();
  const { toast, show, hide } = useToast();
  const scrollRef = useRef<ScrollViewHandle>(null);
  const [disconnectAllOpen, setDisconnectAllOpen] = useState(false);
  const [disconnecting, setDisconnecting] = useState<
    RemoteSessionItem | AdapterSessionItem | null
  >(null);
  const [clearOpen, setClearOpen] = useState(false);
  const [showAll, setShowAll] = useState(false);
  const [openingId, setOpeningId] = useState<string | null>(null);
  useTabScrollToTop(navigation, scrollRef);

  const openRemote = (hostId: string, sessionId?: string) =>
    navigation.navigate('Session', {
      hostId,
      ...(sessionId === undefined ? {} : { sessionId }),
    });
  // V33: back to the folded-away session as it was, no new connection.
  const openAdapter = (item: AdapterSessionItem) =>
    navigation.navigate('AdapterSession', {
      ...item.params,
      sessionId: item.id,
    });
  const openSsh = (serverId: string, sshId?: string) =>
    navigation.navigate('Terminal', {
      serverId,
      ...(sshId === undefined ? {} : { sshId }),
    });
  // Same route as tapping the computer on Home: offline checks, Screen
  // Sharing / Remote Desktop first, one at a time.
  const openHistory = (item: HistoryItem) => {
    if (item.kind === 'ssh') {
      openSsh(item.targetId);
      return;
    }
    if (openingId !== null) {
      return;
    }
    const collapsed = model.adapters.find(
      adapter => adapter.targetId === item.targetId,
    );
    if (item.kind === 'adapter' && collapsed !== undefined) {
      openAdapter(collapsed);
      return;
    }
    const key =
      item.kind === 'remote' ? `host:${item.targetId}` : item.targetId;
    const entry = entries.find(candidate => candidate.key === key);
    if (entry === undefined) {
      show(sessions.recent.gone, { icon: 'alert' });
      return;
    }
    if (entry.kind === 'host') {
      const live = model.remote.find(
        remote => remote.hostId === entry.computer.hostId,
      );
      if (live !== undefined) {
        openRemote(live.hostId, live.id);
        return;
      }
    }
    setOpeningId(item.id);
    connect(entry)
      .catch(() => undefined)
      .finally(() => setOpeningId(null));
  };
  const openSshLive = (item: SshSessionItem) =>
    openSsh(item.serverId, item.sshId);

  const disconnectAll = () => {
    for (const item of model.remote) {
      core.sessions.close(item.id);
    }
    for (const item of model.adapters) {
      disconnectCollapsed(item.id);
    }
    for (const item of model.ssh) {
      core.ssh.close(item.sshId);
    }
    setDisconnectAllOpen(false);
  };

  const disconnectOne = () => {
    if (disconnecting !== null && isAdapterItem(disconnecting)) {
      disconnectCollapsed(disconnecting.id);
    } else if (disconnecting !== null) {
      core.sessions.close(disconnecting.id);
    }
    setDisconnecting(null);
  };

  const confirmClear = () => {
    clearHistory(kv);
    setShowAll(false);
    setClearOpen(false);
  };

  const empty = model.liveCount === 0 && model.history.length === 0;
  const history = showAll
    ? model.history
    : model.history.slice(0, HISTORY_PREVIEW);

  return (
    <SafeAreaView edges={['top']} style={styles.screen}>
      <ScrollView
        contentContainerStyle={styles.content}
        ref={scrollRef}
        showsVerticalScrollIndicator={false}
        testID="sessions-scroll"
      >
        <View style={[styles.header, { minHeight: hit.min }]}>
          <AppText accessibilityRole="header" variant="largeTitle">
            {sessions.title}
          </AppText>
          {model.liveCount === 0 ? null : (
            <Pressable
              accessibilityLabel={sessions.disconnectAll}
              accessibilityRole="button"
              onPress={() => setDisconnectAllOpen(true)}
              style={({ pressed }) => [
                styles.disconnectAll,
                { minHeight: hit.min },
                pressed ? styles.pressed : null,
              ]}
            >
              <AppText color="bad" variant="button" weight={600}>
                {sessions.disconnectAll}
              </AppText>
            </Pressable>
          )}
        </View>

        {empty ? (
          <View style={styles.empty}>
            <EmptyState
              action={{
                label: sessions.empty.action,
                onPress: () => navigation.navigate('Home'),
              }}
              icon="sessions"
              message={sessions.empty.message}
              title={sessions.empty.title}
            />
          </View>
        ) : (
          <>
            {model.liveCount === 0 ? null : (
              <View style={styles.section}>
                <View style={styles.sectionHead}>
                  <SectionHeader
                    title={sessions.live.section(model.liveCount)}
                  />
                  <AppText color="text3" variant="caption">
                    {Platform.OS === 'android'
                      ? sessions.live.backgroundNoteAndroid
                      : sessions.live.backgroundNote}
                  </AppText>
                </View>
                {model.remote.map(item => (
                  <LiveRemoteCard
                    item={item}
                    key={item.id}
                    onDisconnect={() => setDisconnecting(item)}
                    onReturn={() => openRemote(item.hostId, item.id)}
                  />
                ))}
                {model.adapters.map(item => (
                  <LiveRemoteCard
                    item={item}
                    key={item.id}
                    onDisconnect={() => setDisconnecting(item)}
                    onReturn={() => openAdapter(item)}
                  />
                ))}
                {model.ssh.length === 0 ? null : (
                  <LiveSshList items={model.ssh} onOpen={openSshLive} />
                )}
              </View>
            )}

            {model.history.length === 0 ? null : (
              <View style={styles.section}>
                <SectionHeader
                  right={
                    <Pressable
                      accessibilityLabel={sessions.recent.clear}
                      accessibilityRole="button"
                      hitSlop={12}
                      onPress={() => setClearOpen(true)}
                      style={({ pressed }) =>
                        pressed && Platform.OS !== 'android'
                          ? styles.textPressed
                          : null
                      }
                      testID="history-clear"
                    >
                      <AppText color="text3" variant="caption" weight={600}>
                        {sessions.recent.clear}
                      </AppText>
                    </Pressable>
                  }
                  title={sessions.recent.section}
                />
                <HistoryList
                  busyId={openingId}
                  items={history}
                  onOpen={openHistory}
                />
                {model.history.length <= HISTORY_PREVIEW ? null : (
                  <Button
                    align="center"
                    onPress={() => setShowAll(all => !all)}
                    testID="history-more"
                    title={
                      showAll
                        ? sessions.recent.less
                        : sessions.recent.more(model.history.length)
                    }
                    variant="text"
                  />
                )}
              </View>
            )}
          </>
        )}
      </ScrollView>

      <Dialog
        actions={[
          {
            label: sessions.disconnectDialog.confirm,
            onPress: disconnectAll,
            variant: 'pillDanger',
          },
          {
            label: common.cancel,
            onPress: () => setDisconnectAllOpen(false),
            variant: 'secondary',
          },
        ]}
        message={sessions.disconnectDialog.message}
        onDismiss={() => setDisconnectAllOpen(false)}
        testID="disconnect-all-dialog"
        title={sessions.disconnectDialog.title(model.liveCount)}
        visible={disconnectAllOpen}
      />
      <Dialog
        actions={[
          {
            label: sessions.disconnectOne.confirm,
            onPress: disconnectOne,
            testID: 'disconnect-one-confirm',
            variant: 'pillDanger',
          },
          {
            label: common.cancel,
            onPress: () => setDisconnecting(null),
            variant: 'secondary',
          },
        ]}
        message={sessions.disconnectOne.message}
        onDismiss={() => setDisconnecting(null)}
        testID="disconnect-one-dialog"
        title={sessions.disconnectOne.title(disconnecting?.name ?? '')}
        visible={disconnecting !== null}
      />
      <Dialog
        actions={[
          {
            label: sessions.recent.clearDialog.confirm,
            onPress: confirmClear,
            testID: 'history-clear-confirm',
            variant: 'pillDanger',
          },
          {
            label: common.cancel,
            onPress: () => setClearOpen(false),
            variant: 'secondary',
          },
        ]}
        message={sessions.recent.clearDialog.message}
        onDismiss={() => setClearOpen(false)}
        testID="history-clear-dialog"
        title={sessions.recent.clearDialog.title}
        visible={clearOpen}
      />
      <FallbackGuide guide={guide} onClose={closeGuide} />
      <Toast onHide={hide} position="bottom" toast={toast} />
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    backgroundColor: tokens.bg,
  },
  content: {
    gap: space.section,
    paddingHorizontal: space.gutter,
    paddingTop: space.md,
    paddingBottom: 32,
  },
  header: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    flexWrap: 'wrap',
  },
  disconnectAll: {
    justifyContent: 'center',
    paddingHorizontal: 14,
    borderRadius: 22,
    backgroundColor: 'rgba(255,136,115,0.12)',
  },
  pressed: {
    opacity: 0.8,
  },
  textPressed: {
    opacity: press.opacity,
  },
  section: {
    gap: space.sm,
  },
  sectionHead: {
    gap: 2,
  },
  empty: {
    paddingTop: 96,
  },
});
