import { useEffect, useRef, useState } from 'react';
import {
  AccessibilityInfo,
  Platform,
  Pressable,
  StyleSheet,
  View,
} from 'react-native';
import { useWindowSize } from '../../features/session/WindowContext';
import AppText from '../../components/AppText';
import { Icon } from '../../components/icons/Icon';
import { useSetting } from '../../features/settings/store';
import type { NetworkEntry } from '../../features/settings/schema';
import {
  hudCaption,
  intranetNetworkAt,
  noticeFor,
} from '../../features/session/pathNotice';
import type { NoticeKind } from '../../features/session/pathNotice';
import { sharedRelayTcp } from '../../i18n/zh/sharedRelayTcp';
import { hit, radius, tokens } from '../../theme/tokens';
import type { SlotProps } from './slots';

const NOTICE_MS = 8000;

function nonEmpty(value: string | null | undefined): string | null {
  if (value === undefined || value === null || value.trim() === '') {
    return null;
  }
  return value;
}

function okAddress(rec: SlotProps['ctx']['rec']): string | null {
  return rec?.dial.find(attempt => attempt.state === 'ok')?.addr ?? null;
}

/**
 * 走中继或内网 TCP 时的一行说明。带底色，放在会话页顶部的提示栏里（R37、R38），
 * 不再直接压在画面上。
 */
export function PathCaption({ ctx }: SlotProps) {
  const [storedNetworks] = useSetting('networks');
  const networks: NetworkEntry[] = storedNetworks;
  const caption = hudCaption({
    relay: ctx.view.relay,
    tcp: ctx.view.tcp,
    path: ctx.rec?.path ?? null,
    addr: okAddress(ctx.rec),
    networks,
  });
  if (caption === null) {
    return null;
  }
  return (
    <View pointerEvents="none" style={styles.caption} testID="path-caption">
      <Icon color="relay" name="relay" size={12} />
      <AppText color="text2" variant="caption">
        {caption}
      </AppText>
    </View>
  );
}

export function PathNotice({ ctx }: SlotProps) {
  const { width } = useWindowSize();
  const [storedNetworks] = useSetting('networks');
  const networks: NetworkEntry[] = storedNetworks;
  const rec = ctx.rec;
  const relay = ctx.view.relay;
  const tcp = ctx.view.tcp;
  const addr = okAddress(rec);

  const seenRelay = useRef(false);
  const seenTcp = useRef(false);
  const [queue, setQueue] = useState<NoticeKind[]>([]);
  const notice = queue[0] ?? null;
  const noticeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const announcedNotice = useRef<NoticeKind | null>(null);

  const clearNotice = () => {
    if (noticeTimer.current !== null) {
      clearTimeout(noticeTimer.current);
      noticeTimer.current = null;
    }
  };

  const dequeue = () => {
    clearNotice();
    setQueue(current => current.slice(1));
  };

  useEffect(() => {
    if (!relay || seenRelay.current) {
      return;
    }
    seenRelay.current = true;
    setQueue(current =>
      current.includes('relay') ? current : [...current, 'relay'],
    );
  }, [relay]);

  useEffect(() => {
    if (!tcp || seenTcp.current) {
      return;
    }
    seenTcp.current = true;
    setQueue(current =>
      current.includes('tcp') ? current : [...current, 'tcp'],
    );
  }, [tcp]);

  useEffect(() => {
    if (notice === null) {
      return undefined;
    }
    clearNotice();
    noticeTimer.current = setTimeout(() => {
      noticeTimer.current = null;
      setQueue(current => current.slice(1));
    }, NOTICE_MS);
    return clearNotice;
  }, [notice]);

  const stats = rec?.stats ?? null;
  const region = nonEmpty(stats?.derpRegion ?? rec?.derpRegion);
  const matchedNetwork = intranetNetworkAt(addr, networks);
  const intranet = tcp && (rec?.path === 'intranet' || matchedNetwork !== null);
  const content =
    notice === null
      ? null
      : noticeFor(notice, {
          region,
          height: stats?.height ?? null,
          fps: stats?.fps ?? null,
          intranet,
        });

  useEffect(() => {
    if (notice === null || content === null) {
      announcedNotice.current = null;
      return;
    }
    if (Platform.OS !== 'ios' || announcedNotice.current === notice) {
      return;
    }
    announcedNotice.current = notice;
    AccessibilityInfo.announceForAccessibility(
      `${content.title}。${content.body}`,
    );
  }, [content, notice]);

  const noticeWidth = Math.max(0, Math.min(430, width - 2 * 59));

  return (
    <>
      {notice !== null && content !== null && (
        <View
          style={[styles.card, { width: noticeWidth }]}
          testID="path-notice"
        >
          <View
            accessible
            accessibilityLabel={`${content.title}。${content.body}`}
            accessibilityLiveRegion="polite"
            style={styles.cardText}
          >
            <Icon color="relay" name="relay" size={20} />
            <View style={styles.cardCopy}>
              <AppText weight={600} variant="footnote">
                {content.title}
              </AppText>
              <AppText color="text2" variant="caption">
                {content.body}
              </AppText>
            </View>
          </View>
          <Pressable
            accessibilityLabel={sharedRelayTcp.path.gotIt}
            accessibilityRole="button"
            onPress={dequeue}
            style={[styles.textAction, { minHeight: hit.min }]}
          >
            <AppText color="relay" variant="button" weight={500}>
              {sharedRelayTcp.path.gotIt}
            </AppText>
          </Pressable>
        </View>
      )}
    </>
  );
}

const styles = StyleSheet.create({
  caption: {
    minHeight: 24,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 5,
    paddingHorizontal: 10,
    borderRadius: 12,
    backgroundColor: tokens.hud,
  },
  card: {
    position: 'absolute',
    bottom: 78,
    alignSelf: 'center',
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    paddingTop: 12,
    paddingRight: 8,
    paddingBottom: 12,
    paddingLeft: 14,
    borderRadius: radius.card,
    borderWidth: 1,
    borderColor: 'rgba(244, 178, 78, 0.35)',
    backgroundColor: 'rgba(28, 24, 16, 0.95)',
  },
  cardText: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    flexShrink: 1,
  },
  cardCopy: {
    flexShrink: 1,
    gap: 2,
  },
  textAction: {
    justifyContent: 'center',
    paddingHorizontal: 8,
  },
});

export default PathNotice;
