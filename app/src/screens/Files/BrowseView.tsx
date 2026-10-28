import { useEffect, useMemo, useRef, useState } from 'react';
import type { JSX, RefObject } from 'react';
import {
  AccessibilityInfo,
  ActivityIndicator,
  StyleSheet,
  View,
} from 'react-native';
import type { ViewInstance } from 'react-native';
import { useQuery } from '@tanstack/react-query';
import AppText from '../../components/AppText';
import { EmptyState } from '../../components/EmptyState';
import { Icon } from '../../components/icons/Icon';
import { IconButton } from '../../components/IconButton';
import { ListGroup, ListRow } from '../../components/List';
import {
  crumbs,
  entryRows,
  fileSources,
  parentPath,
  type BrowseLocation,
  type EntryRowModel,
} from '../../features/files/browseView';
import { useFiles } from '../../features/files/FilesContext';
import { isUnsupported, type FileSource } from '../../features/files/filesApi';
import { useTransfers } from '../../features/files/useTransfers';
import { useDirectory, useSshServers } from '../../features/data/queries';
import { common } from '../../i18n/zh/common';
import { filesBrowse } from '../../i18n/zh/filesBrowse';
import { usePlatform } from '../../native/platform/PlatformContext';
import type { SshServer } from '../../native/rscore/types';
import { hit, space, tokens } from '../../theme/tokens';
import { SourceChips } from './SourceChips';

const EMPTY_SERVERS: SshServer[] = [];

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function BrowseView(p: {
  onLocationChange?: (loc: BrowseLocation | null) => void;
  onMessage?: (text: string) => void;
}): JSX.Element {
  const { files } = useFiles();
  const transfers = useTransfers();
  const directoryQuery = useDirectory();
  const sshQuery = useSshServers();
  const platform = usePlatform();
  const directory = directoryQuery.directory;
  const servers = sshQuery.data ?? EMPTY_SERVERS;
  const chips = useMemo(
    () => fileSources(directory, servers),
    [directory, servers],
  );
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [path, setPath] = useState<string | null>(null);
  const onLocationChangeRef = useRef(p.onLocationChange);
  const folderChangedRef = useRef(false);
  const currentFolderRef = useRef<ViewInstance>(null);
  onLocationChangeRef.current = p.onLocationChange;
  const selectedChip = chips.find(chip => chip.key === selectedKey);
  const source = selectedChip?.source ?? null;

  useEffect(() => {
    const next =
      selectedKey !== null && chips.some(chip => chip.key === selectedKey)
        ? selectedKey
        : chips[0]?.key ?? null;
    if (next !== selectedKey) {
      folderChangedRef.current = false;
      setSelectedKey(next);
      setPath(null);
    }
  }, [chips, selectedKey]);

  const listingQuery = useQuery({
    queryKey: ['files', 'list', selectedKey, path ?? '~'],
    queryFn: (): ReturnType<typeof files.list> => {
      if (source === null) {
        return Promise.reject(new Error(filesBrowse.states.noSourcesTitle));
      }
      return files.list(source, path);
    },
    enabled: source !== null,
  });
  const listing = listingQuery.data;
  const error = listingQuery.error;

  useEffect(() => {
    if (
      listing !== undefined &&
      source !== null &&
      selectedChip !== undefined
    ) {
      onLocationChangeRef.current?.({
        source,
        path: listing.path,
        label: selectedChip.label,
      });
      return;
    }
    if (source === null || error !== null) {
      onLocationChangeRef.current?.(null);
    }
  }, [listing, error, source, selectedChip]);

  useEffect(() => {
    if (listing === undefined || !folderChangedRef.current) {
      return;
    }
    const timer = setTimeout(() => {
      if (!folderChangedRef.current) {
        return;
      }
      if (currentFolderRef.current !== null) {
        AccessibilityInfo.sendAccessibilityEvent(
          currentFolderRef.current,
          'focus',
        );
      }
      folderChangedRef.current = false;
    }, 0);
    return () => clearTimeout(timer);
  }, [listing]);

  if (directoryQuery.isLoading || sshQuery.isLoading) {
    return (
      <View style={styles.loading}>
        <ActivityIndicator
          accessibilityLabel={common.loading}
          color={tokens.accent}
        />
      </View>
    );
  }

  if (chips.length === 0) {
    return (
      <EmptyState
        icon="computer"
        message={filesBrowse.states.noSourcesMessage}
        title={filesBrowse.states.noSourcesTitle}
      />
    );
  }

  const open = async (row: EntryRowModel) => {
    if (row.kind === 'dir') {
      folderChangedRef.current = true;
      setPath(row.fullPath);
      return;
    }
    if (row.badge?.tone === 'good') {
      if (row.localUri !== null) {
        try {
          await platform.share({ url: row.localUri, title: row.name });
        } catch {
          p.onMessage?.(filesBrowse.messages.shareFailed);
        }
      }
      return;
    }
    if (row.badge?.tone === 'accent') {
      p.onMessage?.(filesBrowse.messages.downloading(row.name));
      return;
    }
    if (source === null) {
      return;
    }
    try {
      await files.download(source, row.fullPath);
      p.onMessage?.(filesBrowse.messages.started(row.name));
    } catch (err) {
      p.onMessage?.(filesBrowse.messages.startFailed(errorMessage(err)));
    }
  };

  return (
    <View style={styles.root}>
      <SourceChips
        chips={chips}
        onSelect={key => {
          if (key !== selectedKey) {
            folderChangedRef.current = false;
            setSelectedKey(key);
            setPath(null);
          }
        }}
        selectedKey={selectedKey}
      />
      {listing === undefined ? (
        error === null ? (
          <View style={styles.loading}>
            <ActivityIndicator
              accessibilityLabel={common.loading}
              color={tokens.accent}
            />
          </View>
        ) : isUnsupported(error) ? (
          <EmptyState
            icon="folder"
            message={filesBrowse.states.unsupportedMessage}
            title={filesBrowse.states.unsupportedTitle}
          />
        ) : (
          <EmptyState
            action={{
              label: common.retry,
              onPress: () => {
                listingQuery.refetch().catch(() => undefined);
              },
            }}
            icon="alert"
            message={errorMessage(error)}
            title={filesBrowse.states.errorTitle}
          />
        )
      ) : (
        <>
          <PathRow
            canGoUp={crumbs(listing.path).canGoUp}
            count={listing.entries.length}
            currentFolderRef={currentFolderRef}
            netPath={listing.netPath}
            onUp={() => {
              folderChangedRef.current = true;
              setPath(parentPath(listing.path));
            }}
            path={listing.path}
          />
          {listing.entries.length === 0 ? (
            <AppText color="text3" variant="caption">
              {filesBrowse.emptyFolder}
            </AppText>
          ) : (
            <ListGroup>
              {entryRows(
                listing,
                source as FileSource,
                transfers,
                Date.now() * 1000,
              ).map(row => (
                <ListRow
                  accessibilityLabel={row.a11y}
                  chevron={row.kind === 'dir'}
                  key={row.key}
                  left={
                    <Icon
                      color={row.kind === 'dir' ? 'accent' : 'text2'}
                      name={row.icon}
                      size={22}
                    />
                  }
                  onPress={() => {
                    open(row).catch(() => undefined);
                  }}
                  right={
                    row.badge === null ? null : (
                      <AppText
                        color={row.badge.tone === 'good' ? 'good' : 'accent'}
                        mono={row.badge.tone === 'accent'}
                        variant="monoSmall"
                      >
                        {row.badge.text}
                      </AppText>
                    )
                  }
                  subtitle={row.subtitle}
                  title={row.name}
                />
              ))}
            </ListGroup>
          )}
          <AppText color="text3" style={styles.hint} variant="caption">
            {filesBrowse.hint}
          </AppText>
        </>
      )}
    </View>
  );
}

function PathRow({
  path,
  count,
  netPath,
  canGoUp,
  onUp,
  currentFolderRef,
}: {
  path: string;
  count: number;
  netPath:
    | 'tailnetDirect'
    | 'tailnetDerp'
    | 'intranet'
    | 'lan'
    | 'manual'
    | null;
  canGoUp: boolean;
  onUp: () => void;
  currentFolderRef: RefObject<ViewInstance | null>;
}) {
  const parts = crumbs(path);
  return (
    <View
      accessibilityLabel={filesBrowse.pathLabel}
      style={[styles.pathRow, { minHeight: hit.min }]}
    >
      <IconButton
        accessibilityLabel={filesBrowse.a11y.up}
        disabled={!canGoUp}
        icon="arrowUp"
        onPress={onUp}
      />
      <View style={styles.crumb}>
        {parts.parent === null ? null : (
          <>
            <AppText color="text3" variant="button" weight={400}>
              {parts.parent}
            </AppText>
            <AppText color="muted" variant="button">
              /
            </AppText>
          </>
        )}
        <View
          ref={currentFolderRef}
          accessible
          accessibilityLabel={parts.current}
          accessibilityRole="header"
        >
          <AppText variant="button" weight={600}>
            {parts.current}
          </AppText>
        </View>
      </View>
      <View style={styles.spacer} />
      <AppText color="text3" variant="caption">
        {filesBrowse.pathStatus(
          count,
          netPath === null ? null : common.path[netPath],
        )}
      </AppText>
    </View>
  );
}

const styles = StyleSheet.create({
  root: {
    gap: space.md,
  },
  loading: {
    minHeight: 180,
    alignItems: 'center',
    justifyContent: 'center',
  },
  pathRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
  },
  crumb: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    flexShrink: 1,
  },
  spacer: {
    flex: 1,
  },
  hint: {
    lineHeight: 18,
  },
});
