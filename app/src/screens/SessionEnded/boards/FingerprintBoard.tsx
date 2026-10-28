import { useState } from 'react';
import { StyleSheet, View } from 'react-native';
import { useQueryClient } from '@tanstack/react-query';
import AppText from '../../../components/AppText';
import Dialog from '../../../components/Dialog';
import { fpDisplay } from '../../../features/format/fingerprint';
import { usePairedHosts } from '../../../features/data/queries';
import { qk } from '../../../features/data/queryClient';
import { useTailnetEnabled } from '../../../features/tailnet/enabled';
import { tokens } from '../../../theme/tokens';
import { common } from '../../../i18n/zh/common';
import { errBoards2 } from '../../../i18n/zh/errBoards2';
import { useCore } from '../../../native/rscore/CoreContext';
import type { BoardProps } from '../boardTypes';
import { ErrorFrame } from '../ErrorFrame';
import { MockWindow, NoteBox, StatusRow, summaryText } from '../parts';

const US_PER_MS = 1000;
const MS_PER_MINUTE = 60 * 1000;

function formatDate(us: number): string {
  const date = new Date(
    Math.floor(us / US_PER_MS) - new Date().getTimezoneOffset() * MS_PER_MINUTE,
  );
  return `${date.getUTCFullYear()}-${String(date.getUTCMonth() + 1).padStart(
    2,
    '0',
  )}-${String(date.getUTCDate()).padStart(2, '0')}`;
}

function fingerprint(fp: string, prefix: string): string {
  return `${prefix}${fpDisplay(fp)}`;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export default function FingerprintBoard({
  params,
  code,
  host,
  hostName,
  os,
  actions,
}: BoardProps) {
  const copy = errBoards2.fingerprint;
  const core = useCore();
  const queryClient = useQueryClient();
  const pairedHosts = usePairedHosts();
  const tailnetEnabled = useTailnetEnabled();
  const [confirmVisible, setConfirmVisible] = useState(false);
  const [forgetting, setForgetting] = useState(false);
  const paired = pairedHosts.data?.find(item => item.hostId === params.hostId);
  const previousFp = paired?.fp ?? host?.fp ?? '';
  const currentFp = params.detail?.fp ?? '';
  const pairedAt =
    paired === undefined ? copy.unknownDate : formatDate(paired.pairedAt);
  // With Tailscale off its address is not the one shown, nor named.
  const addr =
    params.detail?.addr ??
    host?.addresses.find(address =>
      tailnetEnabled ? address.kind === 'tailnet' : address.kind !== 'tailnet',
    )?.addr ??
    host?.addresses[0]?.addr ??
    copy.unknownDate;
  const path =
    params.detail?.path ?? (tailnetEnabled ? copy.defaultPath : null);
  const addressValue = path === null ? addr : `${addr} · ${path}`;
  const hostVersion = params.detail?.host_version ?? copy.unknownVersion;
  const note = copy.note(os);
  const illustrationSummary = summaryText(
    [
      {
        label: copy.previousLabel(pairedAt),
        value: fingerprint(previousFp, copy.fingerprintPrefix),
      },
      {
        label: copy.currentLabel,
        value: fingerprint(currentFp, copy.fingerprintPrefix),
      },
      { label: copy.addressLabel, value: addressValue },
      {
        label: copy.hostVersionLabel,
        value: copy.versionArrow(
          host?.hostVersion ?? copy.unknownDate,
          hostVersion,
        ),
      },
    ],
    [note],
  );

  async function forgetAndPair(): Promise<void> {
    setForgetting(true);
    try {
      await core.hosts.forget(params.hostId);
      await queryClient.invalidateQueries({ queryKey: qk.pairedHosts });
      setConfirmVisible(false);
      actions.pair('scan');
    } catch (error) {
      actions.toast(copy.forgetFailed(errorMessage(error)));
    } finally {
      setForgetting(false);
    }
  }

  return (
    <>
      <ErrorFrame
        code={code}
        desc={copy.desc}
        illustrationSummary={illustrationSummary}
        illustration={
          <MockWindow title={copy.windowTitle(host?.node ?? hostName)}>
            <View style={styles.fingerprints}>
              <View style={styles.fingerprintBlock}>
                <AppText color="text2" style={styles.label}>
                  {copy.previousLabel(pairedAt)}
                </AppText>
                <AppText mono style={styles.fingerprint}>
                  {fingerprint(previousFp, copy.fingerprintPrefix)}
                </AppText>
              </View>
              <View style={styles.fingerprintBlock}>
                <AppText color="text2" style={styles.label}>
                  {copy.currentLabel}
                </AppText>
                <AppText color="bad" mono style={styles.fingerprint}>
                  {fingerprint(currentFp, copy.fingerprintPrefix)}
                </AppText>
              </View>
            </View>
            <View style={styles.rows}>
              <StatusRow
                label={copy.addressLabel}
                value={addressValue}
              />
              <StatusRow
                label={copy.hostVersionLabel}
                value={copy.versionArrow(
                  host?.hostVersion ?? copy.unknownDate,
                  hostVersion,
                )}
              />
            </View>
            <NoteBox text={note} tone="relay" />
          </MockWindow>
        }
        kicker={copy.kicker}
        meta={copy.meta}
        onCopyCode={actions.copyCode}
        primary={{ label: copy.cancel, onPress: actions.back }}
        secondary={{
          label: copy.rePair,
          onPress: () => setConfirmVisible(true),
        }}
        title={copy.title(hostName)}
      />
      <Dialog
        actions={[
          {
            label: copy.forgetAndPair,
            variant: 'pillDanger',
            loading: forgetting,
            testID: 'fingerprint-forget',
            onPress: () => {
              forgetAndPair().catch(() => undefined);
            },
          },
          {
            label: common.cancel,
            variant: 'secondary',
            onPress: () => setConfirmVisible(false),
          },
        ]}
        message={copy.confirmBody(hostName)}
        onDismiss={() => setConfirmVisible(false)}
        testID="fingerprint-dialog"
        title={copy.confirmTitle}
        visible={confirmVisible}
      />
    </>
  );
}

const styles = StyleSheet.create({
  fingerprints: { gap: 8 },
  fingerprintBlock: {
    gap: 3,
    borderRadius: 10,
    backgroundColor: tokens.raised,
    paddingHorizontal: 9,
    paddingVertical: 7,
  },
  label: { fontSize: 10.5, lineHeight: 14 },
  fingerprint: { fontSize: 11, lineHeight: 16 },
  rows: { gap: 2 },
});
