import { useEffect, useState } from 'react';
import {
  AccessibilityInfo,
  ActivityIndicator,
  Modal,
  Platform,
  StyleSheet,
  View,
} from 'react-native';
import AppText from '../../components/AppText';
import { BottomSheet } from '../../components/BottomSheet';
import { Button } from '../../components/Button';
import Dialog from '../../components/Dialog';
import { EmptyState } from '../../components/EmptyState';
import { IconButton } from '../../components/IconButton';
import { ListGroup, ListRow } from '../../components/List';
import { Switch } from '../../components/Switch';
import { TextField } from '../../components/TextField';
import { Toast, useToast } from '../../components/Toast';
import {
  SNIPPET_CMD_MAX,
  SNIPPET_MAX,
  snippetLabel,
  validateSnippet,
} from '../../features/terminal/snippets';
import type { Snippet } from '../../features/terminal/snippets';
import { useSnippets } from '../../features/terminal/useSnippets';
import { common } from '../../i18n/zh/common';
import { snippets } from '../../i18n/zh/snippets';
import { space, tokens } from '../../theme/tokens';

type Mode = 'list' | 'edit';

type Draft = Pick<Snippet, 'label' | 'cmd' | 'run'>;

// A new snippet only types the command; running it is opt-in.
const EMPTY_DRAFT: Draft = { label: '', cmd: '', run: false };

function errorMessage(error: unknown): string {
  if (error instanceof Error && error.message !== '') {
    return error.message;
  }
  return String(error);
}

function commandPreview(cmd: string): string {
  const singleLine = cmd.replace(/\s+/g, ' ').trim();
  return singleLine.length > 48 ? `${singleLine.slice(0, 47)}…` : singleLine;
}

export function SnippetsEditor({ onClose }: { onClose: () => void }) {
  const { items, isLoading, isError, retry, add, update, remove } =
    useSnippets();
  const { toast, show, hide } = useToast();
  const [mode, setMode] = useState<Mode>('list');
  const [editingId, setEditingId] = useState<string | null>(null);
  const [original, setOriginal] = useState<Draft>(EMPTY_DRAFT);
  const [label, setLabel] = useState('');
  const [cmd, setCmd] = useState('');
  const [run, setRun] = useState(EMPTY_DRAFT.run);
  // Where to go once the user agrees to drop an unsaved draft.
  const [discard, setDiscard] = useState<'list' | 'close' | null>(null);
  const [cmdError, setCmdError] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<Snippet | null>(null);

  useEffect(() => {
    if (saveError !== null && Platform.OS === 'ios') {
      AccessibilityInfo.announceForAccessibility(saveError);
    }
  }, [saveError]);

  const resetDraft = () => {
    setEditingId(null);
    setOriginal(EMPTY_DRAFT);
    setLabel(EMPTY_DRAFT.label);
    setCmd(EMPTY_DRAFT.cmd);
    setRun(EMPTY_DRAFT.run);
    setCmdError(null);
    setSaveError(null);
  };

  const openAdd = () => {
    resetDraft();
    setMode('edit');
  };

  const openEdit = (snippet: Snippet) => {
    setEditingId(snippet.id);
    setOriginal({ label: snippet.label, cmd: snippet.cmd, run: snippet.run });
    setLabel(snippet.label);
    setCmd(snippet.cmd);
    setRun(snippet.run);
    setCmdError(null);
    setSaveError(null);
    setMode('edit');
  };

  const dirty =
    mode === 'edit' &&
    (label !== original.label || cmd !== original.cmd || run !== original.run);

  const backToList = () => {
    if (saving) {
      return;
    }
    resetDraft();
    setMode('list');
  };

  // T10: leaving the form with changes asks before the draft is lost.
  const leaveForm = (to: 'list' | 'close') => {
    if (saving) {
      return;
    }
    if (dirty) {
      setDiscard(to);
      return;
    }
    if (to === 'list') {
      backToList();
    } else {
      onClose();
    }
  };

  const confirmDiscard = () => {
    const to = discard;
    setDiscard(null);
    if (to === 'list') {
      backToList();
    } else if (to === 'close') {
      onClose();
    }
  };

  const changeCmd = (value: string) => {
    setCmd(value);
    setCmdError(null);
    setSaveError(null);
  };

  const save = async () => {
    if (saving) {
      return;
    }
    const problem = validateSnippet({ label, cmd });
    if (problem !== null) {
      setCmdError(
        problem === 'emptyCmd'
          ? snippets.editor.validation.emptyCmd
          : snippets.editor.validation.tooLong(SNIPPET_CMD_MAX),
      );
      return;
    }

    setSaving(true);
    setSaveError(null);
    try {
      const next = { label: label.trim(), cmd, run };
      if (editingId === null) {
        await add(next);
      } else {
        await update({ ...next, id: editingId });
      }
      resetDraft();
      setMode('list');
      show(snippets.editor.saved, { tone: 'good' });
    } catch (error) {
      setSaveError(snippets.editor.saveFailed(errorMessage(error)));
    } finally {
      setSaving(false);
    }
  };

  const confirmDelete = async () => {
    const target = deleteTarget;
    if (target === null) {
      return;
    }
    setDeleteTarget(null);
    try {
      await remove(target.id);
      resetDraft();
      setMode('list');
      show(snippets.editor.deleted, {
        action: {
          label: snippets.editor.undo,
          onPress: () => {
            add({
              label: target.label,
              cmd: target.cmd,
              run: target.run,
            }).catch(error =>
              show(snippets.editor.saveFailed(errorMessage(error)), {
                tone: 'bad',
              }),
            );
          },
        },
      });
    } catch (error) {
      setSaveError(snippets.editor.deleteFailed(errorMessage(error)));
    }
  };

  const atLimit = items.length >= SNIPPET_MAX;
  const editing = mode === 'edit';
  const title = !editing
    ? snippets.title
    : editingId === null
    ? snippets.editor.addTitle
    : snippets.editor.editTitle;

  const footer = !editing ? (
    isLoading || isError ? undefined : (
      <View style={styles.footer}>
        <Button
          disabled={atLimit}
          fullWidth
          onPress={openAdd}
          title={snippets.list.add}
          variant="primary"
        />
        {atLimit ? (
          <AppText color="text3" variant="caption">
            {snippets.list.limit(SNIPPET_MAX)}
          </AppText>
        ) : null}
      </View>
    )
  ) : (
    <View style={styles.footer}>
      {saveError === null ? null : (
        <AppText accessibilityLiveRegion="polite" color="bad" variant="caption">
          {saveError}
        </AppText>
      )}
      <Button
        disabled={saving}
        fullWidth
        loading={saving}
        onPress={() => {
          save().catch(() => undefined);
        }}
        title={common.save}
        variant="primary"
      />
    </View>
  );

  return (
    <>
      <Modal
        animationType="fade"
        onRequestClose={() => leaveForm(editing ? 'list' : 'close')}
        transparent
        visible
      >
        <BottomSheet
          footer={footer}
          leading={
            editing ? (
              <IconButton
                accessibilityLabel={snippets.a11y.back}
                icon="chevronLeft"
                onPress={() => leaveForm('list')}
                size={36}
                variant="plain"
              />
            ) : undefined
          }
          onClose={() => leaveForm('close')}
          scroll
          subtitle={snippets.subtitle}
          testID="snippets-editor"
          title={title}
        >
          {!editing ? (
            isLoading ? (
              <View style={styles.loading}>
                <ActivityIndicator
                  accessibilityLabel={common.loading}
                  color={tokens.accent}
                  size="small"
                  testID="snippets-loading"
                />
              </View>
            ) : isError ? (
              <EmptyState
                action={{
                  label: common.retry,
                  onPress: retry,
                  testID: 'snippets-retry',
                }}
                icon="alert"
                message={snippets.list.loadFailedMessage}
                title={snippets.list.loadFailed}
              />
            ) : items.length === 0 ? (
              <EmptyState
                icon="prompt"
                message={snippets.list.emptyMessage}
                title={snippets.list.emptyTitle}
              />
            ) : (
              <ListGroup>
                {items.map(snippet => (
                  <ListRow
                    accessibilityLabel={snippets.list.editA11y(
                      snippetLabel(snippet),
                    )}
                    accessibilityHint={
                      snippet.run ? snippets.list.run : snippets.list.typeOnly
                    }
                    chevron
                    key={snippet.id}
                    onPress={() => openEdit(snippet)}
                    right={
                      <AppText color="text3" variant="caption">
                        {snippet.run
                          ? snippets.list.run
                          : snippets.list.typeOnly}
                      </AppText>
                    }
                    subtitle={commandPreview(snippet.cmd)}
                    testID={`snippet-row-${snippet.id}`}
                    title={snippetLabel(snippet)}
                  />
                ))}
              </ListGroup>
            )
          ) : (
            <View style={styles.form}>
              <TextField
                label={snippets.editor.nameLabel}
                onChangeText={value => {
                  setLabel(value);
                  setSaveError(null);
                }}
                placeholder={snippets.editor.namePlaceholder}
                value={label}
              />
              <TextField
                autoCapitalize="none"
                autoCorrect={false}
                error={cmdError ?? undefined}
                label={snippets.editor.commandLabel}
                mono
                multiline
                onChangeText={changeCmd}
                testID="snippet-command"
                value={cmd}
              />
              <ListGroup>
                <ListRow
                  right={
                    <Switch
                      accessibilityLabel={snippets.editor.runTitle}
                      onValueChange={setRun}
                      testID="snippet-run-switch"
                      value={run}
                    />
                  }
                  subtitle={snippets.editor.runSubtitle}
                  title={snippets.editor.runTitle}
                />
                {editingId === null ? null : (
                  <ListRow
                    accessibilityLabel={snippets.editor.deleteA11y(
                      snippetLabel(
                        items.find(item => item.id === editingId) ?? {
                          id: editingId,
                          label,
                          cmd,
                          run,
                        },
                      ),
                    )}
                    onPress={() => {
                      const stored = items.find(item => item.id === editingId);
                      setDeleteTarget(
                        stored ?? { id: editingId, label, cmd, run },
                      );
                    }}
                    title={snippets.editor.delete}
                    tone="danger"
                  />
                )}
              </ListGroup>
            </View>
          )}
        </BottomSheet>
        <Toast onHide={hide} toast={toast} />
      </Modal>
      <Dialog
        actions={[
          {
            label: snippets.discardDialog.confirm,
            onPress: confirmDiscard,
            testID: 'snippet-discard-confirm',
            variant: 'pillDanger',
          },
          {
            label: snippets.discardDialog.keep,
            onPress: () => setDiscard(null),
            variant: 'secondary',
          },
        ]}
        onDismiss={() => setDiscard(null)}
        testID="snippet-discard-dialog"
        title={snippets.discardDialog.title}
        visible={discard !== null}
      />
      <Dialog
        actions={[
          {
            label: common.delete,
            onPress: () => {
              confirmDelete().catch(() => undefined);
            },
            variant: 'pillDanger',
          },
          {
            label: common.cancel,
            onPress: () => setDeleteTarget(null),
            variant: 'secondary',
          },
        ]}
        message={deleteTarget?.cmd}
        onDismiss={() => setDeleteTarget(null)}
        testID="snippet-delete-dialog"
        title={snippets.deleteDialog.title}
        visible={deleteTarget !== null}
      />
    </>
  );
}

const styles = StyleSheet.create({
  form: {
    gap: space.lg,
  },
  footer: {
    gap: space.sm,
    paddingTop: space.xs,
  },
  loading: {
    minHeight: 180,
    alignItems: 'center',
    justifyContent: 'center',
  },
});

export default SnippetsEditor;
