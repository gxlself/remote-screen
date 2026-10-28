import { useEffect, useRef } from 'react';
import {
  AccessibilityInfo,
  ActivityIndicator,
  Platform,
  StyleSheet,
  View,
} from 'react-native';
import AppText from '../../components/AppText';
import { PathLabel } from '../../components/PathMark';
import type {
  AddrProbe,
  AddrVerdict,
  ComputerMatch,
} from '../../features/servers/addressProbe';
import { addByAddress } from '../../i18n/zh/addByAddress';
import { tokens } from '../../theme/tokens';

export type ProbeResultCardProps = {
  probe: AddrProbe;
  verdict: AddrVerdict;
  match: ComputerMatch | null;
  port: number;
  /**
   * What connects without a Host: Screen Sharing, Remote Desktop or SSH.
   * Leads the card when no Host answered.
   */
  reachable?: 'vnc' | 'rdp' | 'ssh' | null;
};

function osLabel(match: ComputerMatch): string {
  return (
    match.computer.osVersion ??
    (match.computer.os === 'macos' ? 'macOS' : 'Windows')
  );
}

function notFoundReason(probe: AddrProbe, port: number): string {
  if (probe.rs === null) {
    return addByAddress.probe.reasons.timeout(port);
  }
  switch (probe.rs?.outcome) {
    case 'timeout':
      return addByAddress.probe.reasons.timeout(port);
    case 'refused':
      return addByAddress.probe.reasons.refused(port);
    case 'noRoute':
      return addByAddress.probe.reasons.noRoute;
    case 'needsVpn':
      return addByAddress.probe.reasons.needsVpn;
    case 'skipped':
      return addByAddress.probe.reasons.skipped;
    default:
      return addByAddress.probe.reasons.other;
  }
}

export function ProbeResultCard({
  probe,
  verdict,
  match,
  port,
  reachable = null,
}: ProbeResultCardProps) {
  const found = verdict === 'known' || verdict === 'new';
  // No Host, but the computer's own sharing (or sshd) answers: that is the
  // main path, so it leads and the missing Host is only a side note.
  const lead = verdict === 'notFound' ? reachable : null;
  const noHostButReachable = lead !== null;
  const title =
    verdict === 'idle'
      ? ''
      : verdict === 'running'
      ? addByAddress.probe.running
      : lead !== null
      ? addByAddress.probe.reachable(lead)
      : verdict === 'notFound'
      ? addByAddress.probe.notFound
      : addByAddress.probe.found(probe.hello?.hostVersion);
  const note = noHostButReachable
    ? addByAddress.probe.noHost
    : verdict === 'notFound'
    ? notFoundReason(probe, port)
    : null;
  const computer = match?.computer ?? null;
  const subtitle =
    found && (computer !== null || probe.hello !== null)
      ? computer === null
        ? probe.hello?.name ?? null
        : addByAddress.probe.hostSubtitle(osLabel(match!), computer.node)
      : null;
  const announcement =
    note !== null
      ? `${title}，${note}`
      : subtitle === null
      ? title
      : `${title}，${subtitle}`;
  const previousTitle = useRef<string | null>(null);

  useEffect(() => {
    if (
      Platform.OS === 'ios' &&
      verdict !== 'idle' &&
      verdict !== 'running' &&
      title !== previousTitle.current
    ) {
      AccessibilityInfo.announceForAccessibility(announcement);
    }
    previousTitle.current = title;
  }, [announcement, title, verdict]);

  if (verdict === 'idle') {
    return null;
  }

  return (
    <View
      accessibilityLiveRegion="polite"
      style={styles.card}
      testID="probe-result-card"
    >
      <View style={styles.row}>
        <AppText variant="headline" weight={600} style={styles.title}>
          {title}
        </AppText>
        {verdict === 'running' ? (
          <ActivityIndicator color={tokens.accent} size="small" />
        ) : null}
      </View>

      {subtitle !== null ? (
        <View style={styles.row}>
          <AppText color="text2" variant="footnote" style={styles.subtitle}>
            {subtitle}
          </AppText>
          {probe.rs === null ? null : (
            <PathLabel
              kind={probe.rs.path}
              latencyMs={probe.rs.ms ?? undefined}
              size="sm"
            />
          )}
        </View>
      ) : null}

      {note === null ? null : (
        <AppText
          color={noHostButReachable ? 'text3' : 'text2'}
          variant="footnote"
        >
          {note}
        </AppText>
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  card: {
    borderRadius: 16,
    backgroundColor: tokens.card,
    padding: 14,
    gap: 10,
  },
  row: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    gap: 10,
  },
  title: {
    flex: 1,
  },
  subtitle: {
    flex: 1,
  },
});
