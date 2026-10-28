import { useState } from 'react';
import {
  ActivityIndicator,
  Pressable,
  ScrollView,
  StyleSheet,
  View,
} from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { useQueryClient } from '@tanstack/react-query';
import type {
  RecordingCapMb,
  RecordingRetentionDays,
} from '../../features/settings/schema';
import { useSetting } from '../../features/settings/store';
import {
  CAP_OPTIONS,
  RECORDINGS_QUERY_KEY,
  RETENTION_OPTIONS,
  capLabel,
  listErrorText,
  retentionLabel,
  useDeleteRecording,
  useRecordings,
  useRecordingsAutoRefresh,
} from '../../features/recording/library';
import { common } from '../../i18n/zh/common';
import { recordings } from '../../i18n/zh/recordings';
import { useCore } from '../../native/rscore/CoreContext';
import type { RsCoreError } from '../../native/rscore/RsCoreApi';
import type { RootScreenProps } from '../../app/navigation/routes';
import AppText from '../../components/AppText';
import { Banner } from '../../components/Banner';
import { Dialog } from '../../components/Dialog';
import { EmptyState } from '../../components/EmptyState';
import { Icon } from '../../components/icons/Icon';
import { ListGroup, ListRow } from '../../components/List';
import { OptionSheet } from '../Settings/OptionSheet';
import { tokens } from '../../theme/tokens';
import { StorageHeader } from './StorageHeader';
import { RecordingRow } from './RecordingRow';

function errorKind(error: unknown): RsCoreError['kind'] | undefined {
  if (error !== null && typeof error === 'object' && 'kind' in error) {
    const kind = (error as { kind: unknown }).kind;
    return typeof kind === 'string' ? (kind as RsCoreError['kind']) : undefined;
  }
  return undefined;
}

function isKeyUnavailable(error: unknown): boolean {
  if (error !== null && typeof error === 'object' && 'message' in error) {
    return String((error as { message: unknown }).message).includes(
      'recording.keyUnavailable',
    );
  }
  return false;
}

export default function RecordingsScreen({
  navigation,
}: RootScreenProps<'Recordings'>) {
  const insets = useSafeAreaInsets();
  const core = useCore();
  const queryClient = useQueryClient();
  const listQuery = useRecordings();
  useRecordingsAutoRefresh();
  const deleteMutation = useDeleteRecording();
  const [maxTotalMb, setMaxTotalMb] = useSetting('recording.maxTotalMb');
  const [retentionDays, setRetentionDays] = useSetting(
    'recording.retentionDays',
  );
  const [capSheetOpen, setCapSheetOpen] = useState(false);
  const [retentionSheetOpen, setRetentionSheetOpen] = useState(false);
  const [deleteId, setDeleteId] = useState<string | null>(null);

  const error = listQuery.error;
  const unsupported =
    core.recording === undefined || errorKind(error) === 'unsupported';
  const keyUnavailable = isKeyUnavailable(error);
  const loading =
    !unsupported && listQuery.isPending && listQuery.fetchStatus === 'fetching';
  const data = listQuery.data;
  const nearFull =
    data?.storage.capBytes !== undefined &&
    data.storage.capBytes > 0 &&
    data.storage.usedBytes / data.storage.capBytes > 0.9;

  const invalidateRecordings = () => {
    queryClient.invalidateQueries({ queryKey: RECORDINGS_QUERY_KEY });
  };

  const updateCap = (value: RecordingCapMb) => {
    setMaxTotalMb(value);
    invalidateRecordings();
  };

  const updateRetention = (value: number) => {
    const next: RecordingRetentionDays =
      value === 0 ? null : (value as 7 | 30 | 90);
    setRetentionDays(next);
    invalidateRecordings();
  };

  const confirmDelete = async () => {
    if (deleteId === null) {
      return;
    }
    const id = deleteId;
    setDeleteId(null);
    try {
      await deleteMutation.mutateAsync(id);
    } catch {
      // The mutation error is rendered below in the page banner.
    }
  };

  return (
    <View style={[styles.screen, { paddingTop: insets.top + 8 }]}>
      <View style={styles.topBar}>
        <Pressable
          accessibilityLabel={common.back}
          accessibilityRole="button"
          onPress={() => navigation.goBack()}
          style={styles.back}
        >
          <Icon color="accent" name="chevronLeft" size={18} />
          <AppText color="accent" variant="body">
            {common.settings}
          </AppText>
        </Pressable>
      </View>
      <AppText accessibilityRole="header" style={styles.title} variant="title">
        {recordings.title}
      </AppText>

      {loading ? (
        <View style={styles.center}>
          <ActivityIndicator
            accessibilityLabel={common.loading}
            color={tokens.accent}
            size="small"
          />
        </View>
      ) : unsupported ? (
        <View style={styles.center}>
          <EmptyState icon="alert" title={recordings.unsupported} />
        </View>
      ) : (
        <ScrollView
          contentContainerStyle={[
            styles.content,
            { paddingBottom: insets.bottom + 24 },
          ]}
          showsVerticalScrollIndicator={false}
          style={styles.list}
          testID="recordings-scroll"
        >
          {keyUnavailable ? (
            <Banner
              action={{
                label: recordings.retry,
                onPress: () => listQuery.refetch(),
              }}
              icon="lock"
              testID="recordings-key-error"
              title={recordings.keyUnavailable}
              tone="bad"
            />
          ) : error !== null && error !== undefined ? (
            <Banner
              action={{
                label: recordings.retry,
                onPress: () => listQuery.refetch(),
              }}
              icon="alert"
              title={listErrorText(error)}
              tone="bad"
            />
          ) : null}

          {data?.storage === undefined ? null : (
            <>
              <StorageHeader storage={data.storage} />
              {nearFull ? (
                <Banner
                  icon="database"
                  testID="recordings-near-full"
                  title={recordings.storageNearFull}
                  tone="warn"
                />
              ) : null}
            </>
          )}

          {data === undefined ? null : data.recordings.length === 0 ? (
            <EmptyState
              icon="record"
              message={recordings.emptyBody}
              title={recordings.emptyTitle}
            />
          ) : (
            <ListGroup>
              {data.recordings.map(recording => (
                <RecordingRow
                  key={recording.recordingId}
                  nowMs={Date.now()}
                  onDelete={() => setDeleteId(recording.recordingId)}
                  onPress={() =>
                    navigation.navigate('RecordingPlayer', {
                      recordingId: recording.recordingId,
                    })
                  }
                  recording={recording}
                  deleting={deleteMutation.isPending}
                />
              ))}
            </ListGroup>
          )}

          {deleteMutation.error === null ||
          deleteMutation.error === undefined ? null : (
            <Banner
              icon="alert"
              title={listErrorText(deleteMutation.error)}
              tone="bad"
            />
          )}

          <View style={styles.options}>
            <ListGroup>
              <ListRow
                chevron
                onPress={() => setCapSheetOpen(true)}
                testID="recording-cap-row"
                title={recordings.capRow}
                value={capLabel(maxTotalMb)}
              />
              <ListRow
                chevron
                onPress={() => setRetentionSheetOpen(true)}
                testID="recording-retention-row"
                title={recordings.retentionRow}
                value={retentionLabel(retentionDays)}
              />
            </ListGroup>
            <AppText color="text3" variant="caption">
              {recordings.retentionNote}
            </AppText>
          </View>
        </ScrollView>
      )}

      {capSheetOpen ? (
        <OptionSheet
          onClose={() => setCapSheetOpen(false)}
          onSelect={updateCap}
          options={CAP_OPTIONS.map(value => ({
            label: capLabel(value),
            value,
          }))}
          title={recordings.capRow}
          value={maxTotalMb}
        />
      ) : null}
      {retentionSheetOpen ? (
        <OptionSheet
          onClose={() => setRetentionSheetOpen(false)}
          onSelect={updateRetention}
          options={RETENTION_OPTIONS.map(value => ({
            label: retentionLabel(value),
            value: value ?? 0,
          }))}
          title={recordings.retentionRow}
          value={retentionDays ?? 0}
        />
      ) : null}
      <Dialog
        actions={[
          {
            label: recordings.cancel,
            onPress: () => setDeleteId(null),
            variant: 'secondary',
          },
          {
            label: recordings.delete,
            onPress: confirmDelete,
            variant: 'pillDanger',
          },
        ]}
        message={recordings.deleteBody}
        onDismiss={() => setDeleteId(null)}
        title={recordings.deleteTitle}
        visible={deleteId !== null}
      />
    </View>
  );
}

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    backgroundColor: tokens.bg,
    paddingHorizontal: 20,
  },
  topBar: {
    minHeight: 44,
    flexDirection: 'row',
    alignItems: 'center',
  },
  back: {
    minHeight: 44,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
  },
  title: {
    marginTop: 8,
    marginBottom: 12,
  },
  list: {
    flex: 1,
  },
  content: {
    gap: 12,
  },
  options: {
    gap: 8,
    marginTop: 4,
  },
  center: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    paddingHorizontal: 24,
  },
});
