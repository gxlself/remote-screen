import { StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { EmptyState } from '../../components/EmptyState';
import { Icon } from '../../components/icons/Icon';
import { ListGroup, ListRow, SectionHeader } from '../../components/List';
import { sourceName } from '../../features/files/browseView';
import { useFiles } from '../../features/files/FilesContext';
import type { Transfer } from '../../features/files/filesApi';
import {
  groupTransfers,
  phoneLabel,
  transferRow,
  type TransferRowModel,
} from '../../features/files/transferView';
import { useTransfers } from '../../features/files/useTransfers';
import { useDirectory, useSshServers } from '../../features/data/queries';
import { files } from '../../i18n/zh/files';
import { usePlatform } from '../../native/platform/PlatformContext';
import type { SshServer } from '../../native/rscore/types';
import { space, tokens } from '../../theme/tokens';
import { TransferCard } from './TransferCard';

const EMPTY_SERVERS: SshServer[] = [];

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function TransfersView({
  onBrowse,
  onMessage,
}: {
  onBrowse(): void;
  onMessage(text: string): void;
}) {
  const { files: api } = useFiles();
  const list = useTransfers();
  const directoryQuery = useDirectory();
  const sshQuery = useSshServers();
  const platform = usePlatform();
  const directory = directoryQuery.directory;
  const servers = sshQuery.data ?? EMPTY_SERVERS;
  const groups = groupTransfers(list, Date.now() * 1000);
  const names = {
    source: (source: Transfer['source']) =>
      sourceName(source, directory, servers),
    phone: phoneLabel(platform.deviceInfo()),
  };
  const rows = (items: Transfer[]): TransferRowModel[] =>
    items.map(transfer => transferRow(transfer, names));
  const activeRows = rows(groups.active);
  const stalledRows = rows(groups.stalled);

  const empty =
    groups.active.length === 0 &&
    groups.stalled.length === 0 &&
    groups.doneToday.length === 0 &&
    groups.doneEarlier.length === 0;

  if (empty) {
    return (
      <EmptyState
        action={{ label: files.transfers.emptyAction, onPress: onBrowse }}
        icon="download"
        message={files.transfers.emptyMessage}
        title={files.transfers.emptyTitle}
      />
    );
  }

  const action = (kind: 'pause' | 'resume' | 'retry', id: string) => {
    if (kind === 'pause') {
      api.pause(id);
      return;
    }
    api.resume(id).catch(error => {
      onMessage(files.messages.operationFailed(errorMessage(error)));
    });
  };

  const share = (row: TransferRowModel, localUri: string) => {
    platform.share({ url: localUri, title: row.title }).catch(error => {
      onMessage(files.messages.operationFailed(errorMessage(error)));
    });
  };

  const completed = (items: Transfer[]) => (
    <ListGroup>
      {items.map(transfer => {
        const row = transferRow(transfer, names);
        const localUri = transfer.localUri;
        return (
          <ListRow
            accessibilityLabel={row.a11y}
            key={row.key}
            left={
              <View style={styles.completedIcon}>
                <Icon color="text2" name={row.icon} size={16} />
              </View>
            }
            onPress={localUri === null ? undefined : () => share(row, localUri)}
            right={
              <AppText color="text3" variant="caption">
                {row.time}
              </AppText>
            }
            subtitle={row.doneText}
            title={row.title}
          />
        );
      })}
    </ListGroup>
  );

  return (
    <View style={styles.root}>
      {groups.active.length > 0 || groups.stalled.length > 0 ? (
        <View style={styles.section}>
          <AppText
            accessibilityRole="header"
            color="text2"
            style={styles.activeTitle}
            weight={600}
          >
            {files.transfers.activeTitle(groups.active.length)}
          </AppText>
          <View style={styles.cards}>
            {activeRows.map(row => (
              <TransferCard
                key={row.key}
                onAction={() => {
                  if (row.action !== null) {
                    action(row.action, row.key);
                  }
                }}
                row={row}
                variant="active"
              />
            ))}
            {stalledRows.map(row => (
              <TransferCard
                key={row.key}
                onAction={() => {
                  if (row.action !== null) {
                    action(row.action, row.key);
                  }
                }}
                row={row}
                variant="stalled"
              />
            ))}
          </View>
        </View>
      ) : null}

      {groups.doneToday.length > 0 ? (
        <View style={styles.section}>
          <SectionHeader title={files.transfers.doneToday} />
          {completed(groups.doneToday)}
        </View>
      ) : null}

      {groups.doneEarlier.length > 0 ? (
        <View style={styles.section}>
          <SectionHeader title={files.transfers.doneEarlier} />
          {completed(groups.doneEarlier)}
        </View>
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create({
  root: {
    gap: space.section,
  },
  section: {
    gap: space.md,
  },
  activeTitle: {
    fontSize: 13,
    lineHeight: 19,
    letterSpacing: 0.5,
  },
  cards: {
    gap: space.md,
  },
  completedIcon: {
    width: 32,
    height: 32,
    borderRadius: 7,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: tokens.raised,
  },
});

export default TransfersView;
