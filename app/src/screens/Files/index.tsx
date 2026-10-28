import { useEffect, useMemo, useState } from 'react';
import { ScrollView, StyleSheet, View } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { EmptyState } from '../../components/EmptyState';
import { Segmented } from '../../components/Segmented';
import { Toast, useToast } from '../../components/Toast';
import type { TabScreenProps } from '../../app/navigation/routes';
import {
  fileSources,
  sendTargets,
  type BrowseLocation,
} from '../../features/files/browseView';
import { incomingShare } from '../../features/files/incomingShare';
import { useFiles } from '../../features/files/FilesContext';
import { isUnsupported, type PickedFile } from '../../features/files/filesApi';
import { useDirectory, useSshServers } from '../../features/data/queries';
import { files as copy } from '../../i18n/zh/files';
import type { SshServer } from '../../native/rscore/types';
import { space, tokens } from '../../theme/tokens';
import { BrowseView } from './BrowseView';
import { SendSheet } from './SendSheet';
import { TransfersView } from './TransfersView';

type FilesView = 'transfers' | 'browse';

const EMPTY_SERVERS: SshServer[] = [];

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export default function FilesScreen(_props: TabScreenProps<'Files'>) {
  const insets = useSafeAreaInsets();
  const { files, picker, available } = useFiles();
  const directoryQuery = useDirectory();
  const sshQuery = useSshServers();
  const toast = useToast();
  const [view, setView] = useState<FilesView>('transfers');
  const [location, setLocation] = useState<BrowseLocation | null>(null);
  const [picked, setPicked] = useState<PickedFile[] | null>(null);
  const [busy, setBusy] = useState(false);
  const directory = directoryQuery.directory;
  const servers = sshQuery.data ?? EMPTY_SERVERS;
  const targets = useMemo(
    () => sendTargets(fileSources(directory, servers), location),
    [directory, location, servers],
  );

  useEffect(() => {
    const consume = () => {
      if (busy || incomingShare.getState().pending.length === 0) {
        return;
      }
      const incoming = incomingShare.take();
      setPicked(current => [...(current ?? []), ...incoming]);
    };
    const unsubscribe = incomingShare.subscribe(consume);
    consume();
    return unsubscribe;
  }, [busy]);

  const pick = async () => {
    try {
      const selected = await picker.pickFiles();
      if (selected.length > 0) {
        setPicked(selected);
      }
    } catch (error) {
      toast.show(
        isUnsupported(error)
          ? copy.messages.pickUnsupported
          : copy.messages.pickFailed(errorMessage(error)),
      );
    }
  };

  const send = async (target: (typeof targets)[number]) => {
    if (picked === null || busy) {
      return;
    }
    setBusy(true);
    try {
      for (const file of picked) {
        await files.upload(target.source, file.uri, target.dir);
      }
      setPicked(null);
      setView('transfers');
      toast.show(copy.messages.sendStarted(target.title));
    } catch (error) {
      toast.show(copy.messages.pickFailed(errorMessage(error)));
    } finally {
      setBusy(false);
    }
  };

  return (
    <View style={styles.root}>
      {available ? (
        <ScrollView
          contentContainerStyle={[
            styles.content,
            {
              paddingTop: insets.top + 11,
              paddingHorizontal: space.gutter,
            },
          ]}
          showsVerticalScrollIndicator={false}
        >
          <View style={styles.header}>
            <AppText
              accessibilityRole="header"
              style={styles.title}
              variant="title"
              weight={700}
            >
              {copy.title}
            </AppText>
            <Button
              accessibilityHint={copy.sendFileHint}
              icon="uploadTo"
              onPress={() => {
                pick().catch(() => undefined);
              }}
              title={copy.sendFile}
            />
          </View>
          <Segmented
            accessibilityLabel={copy.viewsLabel}
            onChange={setView}
            options={[
              { value: 'transfers', label: copy.views.transfers },
              { value: 'browse', label: copy.views.browse },
            ]}
            value={view}
          />
          {view === 'transfers' ? (
            <TransfersView
              onBrowse={() => setView('browse')}
              onMessage={toast.show}
            />
          ) : (
            <BrowseView onLocationChange={setLocation} onMessage={toast.show} />
          )}
        </ScrollView>
      ) : (
        <View
          style={{
            paddingTop: insets.top + 11,
            paddingHorizontal: space.gutter,
          }}
        >
          <EmptyState
            icon="folder"
            message={copy.unsupported.message}
            title={copy.unsupported.title}
          />
        </View>
      )}
      <SendSheet
        busy={busy}
        onClose={() => setPicked(null)}
        onPick={target => {
          send(target).catch(() => undefined);
        }}
        picked={picked ?? []}
        targets={targets}
        visible={picked !== null}
      />
      <Toast onHide={toast.hide} position="bottom" toast={toast.toast} />
    </View>
  );
}

const styles = StyleSheet.create({
  root: {
    flex: 1,
    backgroundColor: tokens.bg,
  },
  content: {
    gap: 14,
    paddingBottom: 100,
  },
  header: {
    minHeight: 44,
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    flexWrap: 'wrap',
    rowGap: space.sm,
  },
  title: {
    fontSize: 28,
    lineHeight: 34,
  },
});
