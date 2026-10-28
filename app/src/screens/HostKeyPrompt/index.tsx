import { useEffect, useRef, useState } from 'react';
import { AccessibilityInfo, Platform, StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import BottomSheet from '../../components/BottomSheet';
import { Button } from '../../components/Button';
import { Dialog } from '../../components/Dialog';
import { Icon } from '../../components/icons/Icon';
import { useCoreEvent } from '../../features/events/EventsProvider';
import { useSshServers } from '../../features/data/queries';
import { formatFp, spokenFp, sshServerName } from '../../features/ssh/prompts';
import { sshPrompt } from '../../i18n/zh/sshPrompt';
import { common } from '../../i18n/zh/common';
import { usePlatform } from '../../native/platform/PlatformContext';
import { useCore } from '../../native/rscore/CoreContext';
import { space, tokens } from '../../theme/tokens';
import type { RootScreenProps } from '../../app/navigation/routes';

type PromptProps = RootScreenProps<'HostKeyPrompt'>;

function LeadingIcon({ changed }: { changed: boolean }) {
  return (
    <View style={[styles.leading, changed ? styles.leadingBad : null]}>
      <Icon
        color={changed ? 'bad' : 'accent'}
        name={changed ? 'warning' : 'shieldKey'}
        size={26}
      />
    </View>
  );
}

function FingerprintBlock({
  label,
  fp,
  keyType,
  tone = 'normal',
  testID,
}: {
  label?: string;
  fp: string;
  // Unknown for the old key: the prompt only carries the new key's type.
  keyType?: string;
  tone?: 'normal' | 'bad';
  testID?: string;
}) {
  const groups = formatFp(fp);
  const rows: string[][] = [];
  for (let index = 0; index < groups.length; index += 4) {
    rows.push(groups.slice(index, index + 4));
  }
  const accessibilityLabel = `${
    label === undefined ? '' : `${label}，`
  }${sshPrompt.hostKey.fingerprint(keyType ?? '', spokenFp(fp))}`;

  return (
    <View
      accessible
      accessibilityLabel={accessibilityLabel}
      style={[
        styles.fingerprint,
        tone === 'bad' ? styles.fingerprintBad : null,
      ]}
      testID={testID}
    >
      {label === undefined ? null : (
        <AppText color="text3" variant="caption">
          {label}
        </AppText>
      )}
      {keyType === undefined ? null : (
        <AppText color="text3" variant="caption">
          {keyType}
        </AppText>
      )}
      <AppText mono selectable style={styles.fpText}>
        {rows.map(row => row.join(' ')).join('\n')}
      </AppText>
    </View>
  );
}

export default function HostKeyPromptScreen({
  navigation,
  route,
}: PromptProps) {
  const { sshId, serverId, fp, keyType, kind, oldFp } = route.params;
  const core = useCore();
  const platform = usePlatform();
  const servers = useSshServers();
  const [trusting, setTrusting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmNoLock, setConfirmNoLock] = useState(false);
  const closedRef = useRef(false);

  useEffect(() => {
    if (error !== null && Platform.OS === 'ios') {
      AccessibilityInfo.announceForAccessibility(error);
    }
  }, [error]);

  const name = sshServerName(servers.data, serverId);
  const changed = kind === 'changed';
  const title = changed
    ? sshPrompt.hostKey.changedTitle(name)
    : sshPrompt.hostKey.firstTitle(name);
  const body = changed
    ? sshPrompt.hostKey.changedBody
    : sshPrompt.hostKey.firstBody;

  const finish = () => {
    if (closedRef.current) {
      return;
    }
    closedRef.current = true;
    navigation.goBack();
  };

  useCoreEvent('ssh.state', event => {
    if (
      event.sshId === sshId &&
      (event.state === 'closed' || (!changed && event.state === 'failed'))
    ) {
      finish();
    }
  });

  const close = () => {
    if (trusting) {
      return;
    }
    core.ssh.close(sshId);
    finish();
  };

  const save = async () => {
    if (closedRef.current) {
      return;
    }
    await core.ssh.trustHostKey(serverId, fp);
    finish();
  };

  const trust = async () => {
    if (trusting) {
      return;
    }
    setError(null);
    setTrusting(true);
    try {
      if (changed) {
        const result = await platform.authenticate(
          sshPrompt.hostKey.authReason,
        );
        if (result === 'failed') {
          setError(sshPrompt.hostKey.authFailed);
          return;
        }
        if (result === 'unavailable') {
          // T08: no passcode to check against; the user confirms in the app.
          setConfirmNoLock(true);
          return;
        }
        if (result !== 'ok') {
          return;
        }
      }
      await save();
    } catch (cause) {
      setError(
        sshPrompt.hostKey.saveFailed(
          cause instanceof Error ? cause.message : String(cause),
        ),
      );
    } finally {
      setTrusting(false);
    }
  };

  const trustWithoutLock = async () => {
    setConfirmNoLock(false);
    setError(null);
    setTrusting(true);
    try {
      await save();
    } catch (cause) {
      setError(
        sshPrompt.hostKey.saveFailed(
          cause instanceof Error ? cause.message : String(cause),
        ),
      );
    } finally {
      setTrusting(false);
    }
  };

  return (
    <>
      <BottomSheet
        footer={
          <View style={styles.actions}>
            {changed ? (
              <>
                <Button
                  disabled={trusting}
                  fullWidth
                  onPress={close}
                  title={sshPrompt.hostKey.disconnect}
                  variant="primary"
                />
                <Button
                  fullWidth
                  loading={trusting}
                  onPress={trust}
                  title={sshPrompt.hostKey.trustChanged}
                  variant="pillDanger"
                />
              </>
            ) : (
              <>
                <Button
                  fullWidth
                  loading={trusting}
                  onPress={trust}
                  title={sshPrompt.hostKey.trustAndConnect}
                  variant="primary"
                />
                <Button
                  disabled={trusting}
                  fullWidth
                  onPress={close}
                  title={common.cancel}
                  variant="text"
                />
              </>
            )}
          </View>
        }
        dismissible={!trusting}
        onClose={close}
        leading={<LeadingIcon changed={changed} />}
        scroll
        title={title}
        testID="host-key-prompt"
      >
        <View style={styles.content}>
          <AppText color="text2" variant="body">
            {body}
          </AppText>

          {changed ? (
            <View style={styles.fingerprints}>
              {oldFp === undefined || oldFp === '' ? (
                <View style={styles.fingerprint} testID="host-key-old">
                  <AppText color="text3" variant="caption">
                    {sshPrompt.hostKey.original}
                  </AppText>
                  <AppText color="text2" variant="footnote">
                    {sshPrompt.hostKey.originalMissing}
                  </AppText>
                </View>
              ) : (
                <FingerprintBlock
                  fp={oldFp}
                  label={sshPrompt.hostKey.original}
                  testID="host-key-old"
                />
              )}
              <FingerprintBlock
                fp={fp}
                keyType={keyType}
                label={sshPrompt.hostKey.current}
                testID="host-key-current"
                tone="bad"
              />
            </View>
          ) : (
            <FingerprintBlock
              fp={fp}
              keyType={keyType}
              testID="host-key-fingerprint"
            />
          )}

          {error === null ? null : (
            <AppText
              accessibilityLiveRegion="polite"
              color="bad"
              variant="caption"
            >
              {error}
            </AppText>
          )}
        </View>
      </BottomSheet>
      <Dialog
        actions={[
          {
            label: sshPrompt.hostKey.noLockDialog.confirm,
            onPress: () => {
              trustWithoutLock().catch(() => undefined);
            },
            testID: 'host-key-no-lock-confirm',
            variant: 'pillDanger',
          },
          {
            label: common.cancel,
            onPress: () => setConfirmNoLock(false),
            variant: 'secondary',
          },
        ]}
        message={sshPrompt.hostKey.noLockDialog.message}
        onDismiss={() => setConfirmNoLock(false)}
        testID="host-key-no-lock-dialog"
        title={sshPrompt.hostKey.noLockDialog.title}
        visible={confirmNoLock}
      />
    </>
  );
}

const styles = StyleSheet.create({
  leading: {
    width: 48,
    height: 48,
    borderRadius: 14,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: tokens.accentSoft,
  },
  leadingBad: {
    backgroundColor: tokens.badSoft,
  },
  content: {
    gap: space.lg,
  },
  fingerprints: {
    gap: space.md,
  },
  fingerprint: {
    gap: space.xs,
    borderRadius: 12,
    padding: 12,
    backgroundColor: tokens.raised,
  },
  fingerprintBad: {
    borderWidth: 1,
    borderColor: tokens.bad,
  },
  fpText: {
    fontSize: 15,
    lineHeight: 22,
    letterSpacing: 0.3,
  },
  actions: {
    gap: space.sm,
  },
});
