import { useEffect, useState } from 'react';
import { StyleSheet, View } from 'react-native';
import AppText from '../../../components/AppText';
import { Icon } from '../../../components/icons/Icon';
import {
  recordDenied,
  remainingRequests,
} from '../../../features/errors/deniedQuota';
import { formatHm } from '../../../features/format/time';
import { defaultDeviceName } from '../../../features/onboarding/discover';
import { common } from '../../../i18n/zh/common';
import { errBoards1 } from '../../../i18n/zh/errBoards1';
import { usePlatform } from '../../../native/platform/PlatformContext';
import { tokens } from '../../../theme/tokens';
import type { BoardProps } from '../boardTypes';
import { ErrorFrame } from '../ErrorFrame';
import { MockWindow, summaryText } from '../parts';

function parseRetries(value: string | undefined): number | null {
  if (value === undefined || !/^\d+$/.test(value)) {
    return null;
  }
  return Number(value);
}

export default function DeniedBoard({
  params,
  code,
  hostName,
  os,
  actions,
}: BoardProps) {
  const copy = errBoards1.denied;
  const mac = os === 'macos';
  const platform = usePlatform();
  const [recordedNowMs, setRecordedNowMs] = useState<number | null>(null);
  const hostId = params.hostId;
  const timeout = params.detail?.reason === 'timeout';

  useEffect(() => {
    const now = Date.now();
    recordDenied(hostId, now);
    setRecordedNowMs(now);
  }, [hostId]);

  // The local name the app registers this phone with; there is no account.
  const deviceName = defaultDeviceName(null, platform.deviceInfo().model);
  const explicitRetries = parseRetries(params.detail?.retries_left);
  const nowMs = recordedNowMs ?? Date.now();
  const remaining =
    explicitRetries ??
    (recordedNowMs === null
      ? Math.max(0, remainingRequests(hostId, nowMs) - 1)
      : remainingRequests(hostId, nowMs));
  const time = formatHm(nowMs * 1000);
  const blocked = remaining === 0;
  const retryDisabled = blocked;
  const illustrationSummary = summaryText(
    [{ label: copy.request(deviceName, mac) }],
    [timeout ? copy.timeoutAt(time) : copy.rejectedAt(time)],
  );

  return (
    <ErrorFrame
      back={{ label: common.back, onPress: actions.back }}
      code={code}
      desc={timeout ? copy.timeoutDesc : copy.rejectedDesc(mac)}
      illustrationSummary={illustrationSummary}
      illustration={
        <MockWindow title={copy.windowTitle(hostName)}>
          <AppText style={styles.request} weight={600}>
            {copy.request(deviceName, mac)}
          </AppText>
          <View style={styles.buttons}>
            <View style={styles.fakeButton}>
              <AppText color="text2" style={styles.fakeButtonText}>
                {common.deny}
              </AppText>
              {timeout ? null : (
                <View style={styles.marker}>
                  <Icon color="bad" name="arrowRight" size={13} />
                  <AppText color="bad" style={styles.markerText}>
                    {copy.marker}
                  </AppText>
                </View>
              )}
            </View>
            <View style={styles.fakeButton}>
              <AppText color="text2" style={styles.fakeButtonText}>
                {common.allow}
              </AppText>
            </View>
          </View>
          <AppText color="text3" style={styles.time}>
            {timeout ? copy.timeoutAt(time) : copy.rejectedAt(time)}
          </AppText>
        </MockWindow>
      }
      kicker={timeout ? copy.timeoutKicker : copy.rejectedKicker}
      meta={blocked ? copy.blocked : copy.remaining(remaining)}
      onCopyCode={actions.copyCode}
      primary={{
        label: copy.again,
        disabled: retryDisabled,
        onPress: () => actions.retry(),
      }}
      secondary={{
        label: copy.viewOnly,
        disabled: retryDisabled,
        onPress: () => actions.retry({ viewOnly: true }),
      }}
      title={
        timeout ? copy.timeoutTitle(hostName) : copy.rejectedTitle(hostName)
      }
    />
  );
}

const styles = StyleSheet.create({
  request: { fontSize: 13, lineHeight: 18 },
  buttons: { flexDirection: 'row', gap: 8, marginTop: 2 },
  fakeButton: {
    flex: 1,
    minHeight: 34,
    alignItems: 'center',
    justifyContent: 'center',
    borderRadius: 8,
    backgroundColor: tokens.raised,
    paddingHorizontal: 6,
    paddingVertical: 4,
  },
  fakeButtonText: { fontSize: 11.5, lineHeight: 16 },
  marker: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 2,
    marginTop: 2,
  },
  markerText: { fontSize: 9.5, lineHeight: 12 },
  time: { fontSize: 10.5, lineHeight: 14, marginTop: 2 },
});
