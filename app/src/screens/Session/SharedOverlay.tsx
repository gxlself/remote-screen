import { useCallback, useEffect, useRef, useState } from 'react';
import {
  AccessibilityInfo,
  PixelRatio,
  Platform,
  Pressable,
  StyleSheet,
  View,
} from 'react-native';
import { useWindowSize } from '../../features/session/WindowContext';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { Countdown, useCountdown } from '../../components/Countdown';
import { Dialog } from '../../components/Dialog';
import { Icon } from '../../components/icons/Icon';
import { useSessionsActions } from '../../features/session/store';
import { sharedView, shortDeviceName } from '../../features/session/sharedView';
import { sharedRelayTcp } from '../../i18n/zh/sharedRelayTcp';
import { useCore } from '../../native/rscore/CoreContext';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import { hit, radius, tokens } from '../../theme/tokens';
import type { SlotProps } from './slots';

const HANDOFF_MS = 8000;
/** Host 等对方答复 30 秒；再多等一会儿还没结果，就当没收到答复。 */
export const REQUEST_WAIT_MS = 35_000;

type LastError = NonNullable<NonNullable<SlotProps['ctx']['rec']>['lastError']>;

export function SharedOverlay({ ctx }: SlotProps) {
  const core = useCore();
  const { clearControlRequest } = useSessionsActions();
  const { width } = useWindowSize();
  const rec = ctx.rec;
  const sessionId = ctx.sessionId;
  // The Host lets the same tailnet login take over without asking, and lists
  // this phone among the viewers with the login it saw for us.
  const myUser =
    rec?.viewers.find(viewer => viewer.deviceId === ctx.myDeviceId)?.user ?? '';
  const model = sharedView({
    rec,
    myUser: myUser === '' ? null : myUser,
    myDeviceId: ctx.myDeviceId,
  });

  const [handoff, setHandoff] = useState<string | null>(null);
  const previousRole = useRef(rec?.role ?? null);
  const previousController = useRef<string | null>(null);
  const handoffTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const announcedHandoff = useRef<string | null>(null);
  const announcedWatchers = useRef<string | null>(null);
  const role = rec?.role ?? null;
  const watchingController =
    role === 'viewer' &&
    rec?.controller !== null &&
    rec?.controller !== undefined &&
    rec.controller.deviceId !== ctx.myDeviceId
      ? rec.controller.deviceName
      : null;

  useEffect(() => {
    const before = previousRole.current;
    const from = previousController.current;
    previousRole.current = role;
    previousController.current = watchingController;
    if (before !== 'viewer' || role !== 'controller' || from === null) {
      return;
    }

    setHandoff(shortDeviceName(from));
    if (handoffTimer.current !== null) {
      clearTimeout(handoffTimer.current);
    }
    handoffTimer.current = setTimeout(() => {
      handoffTimer.current = null;
      setHandoff(null);
    }, HANDOFF_MS);
  }, [role, watchingController]);

  useEffect(
    () => () => {
      if (handoffTimer.current !== null) {
        clearTimeout(handoffTimer.current);
        handoffTimer.current = null;
      }
    },
    [],
  );

  useEffect(() => {
    if (handoff === null) {
      announcedHandoff.current = null;
      return;
    }
    if (Platform.OS !== 'ios' || announcedHandoff.current === handoff) {
      return;
    }
    announcedHandoff.current = handoff;
    AccessibilityInfo.announceForAccessibility(
      `${sharedRelayTcp.shared.handoffTitle(
        handoff,
      )}。${sharedRelayTcp.shared.handoffBody(handoff)}`,
    );
  }, [handoff]);

  useEffect(() => {
    if (model.watchers === null || model.watchers === '') {
      announcedWatchers.current = null;
      return;
    }
    if (Platform.OS === 'ios' && announcedWatchers.current !== model.watchers) {
      announcedWatchers.current = model.watchers;
      AccessibilityInfo.announceForAccessibility(model.watchers);
    }
  }, [model.watchers]);

  // R18：请求发出后等结果，期间按钮转圈、不能再点。
  const [waiting, setWaiting] = useState(false);
  const waitTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const stopWaiting = useCallback(() => {
    if (waitTimer.current !== null) {
      clearTimeout(waitTimer.current);
      waitTimer.current = null;
    }
    setWaiting(false);
  }, []);
  useEffect(() => stopWaiting, [stopWaiting]);
  // 接管成功（角色变成控制者）或不再是观看者时，等待结束。
  useEffect(() => {
    if (role !== 'viewer') {
      stopWaiting();
    }
  }, [role, stopWaiting]);

  const handledError = useRef<LastError | null>(null);
  const lastError = rec?.lastError ?? null;
  useEffect(() => {
    if (
      lastError === null ||
      lastError.code !== 141 ||
      handledError.current === lastError
    ) {
      return;
    }
    handledError.current = lastError;
    stopWaiting();
    const reason = lastError.detail?.reason;
    ctx.showToast(
      (reason === undefined
        ? undefined
        : sharedRelayTcp.shared.rejectedBecause[reason]) ??
        sharedRelayTcp.shared.rejected,
      { tone: 'bad' },
    );
  }, [ctx, lastError, stopWaiting]);

  const request = rec?.controlRequest ?? null;
  const visibleRequest = role === 'controller' ? request : null;
  const answeredRequest = useRef<string | null>(null);
  const requestKey =
    visibleRequest === null
      ? null
      : `${visibleRequest.requester.deviceId}:${visibleRequest.timeoutS}:${visibleRequest.atUs}`;

  useEffect(() => {
    if (role !== 'controller') {
      answeredRequest.current = null;
    }
  }, [role]);

  const answer = (allow: boolean) => {
    if (
      sessionId === null ||
      visibleRequest === null ||
      requestKey === null ||
      answeredRequest.current === requestKey
    ) {
      return;
    }
    answeredRequest.current = requestKey;
    try {
      core.sessions.respondControl(
        sessionId,
        visibleRequest.requester.deviceId,
        allow,
      );
    } catch (error) {
      if (error instanceof RsCoreError && error.kind === 'unsupported') {
        ctx.showToast(sharedRelayTcp.shared.unsupported);
      }
    } finally {
      clearControlRequest(sessionId);
    }
  };

  const remaining = useCountdown(request?.timeoutS ?? 0, {
    running: visibleRequest !== null,
    key: request?.atUs,
    onDone: () => {
      if (sessionId !== null) {
        clearControlRequest(sessionId);
      }
    },
  });

  const takeOver = () => {
    if (sessionId === null || waiting) {
      return;
    }
    try {
      core.sessions.requestControl(sessionId);
    } catch (error) {
      if (error instanceof RsCoreError && error.kind === 'unsupported') {
        ctx.showToast(sharedRelayTcp.shared.unsupported);
      } else {
        ctx.showToast(sharedRelayTcp.shared.requestFailed, { tone: 'bad' });
      }
      return;
    }
    setWaiting(true);
    waitTimer.current = setTimeout(() => {
      waitTimer.current = null;
      setWaiting(false);
      ctx.showToast(sharedRelayTcp.shared.noAnswer, { tone: 'bad' });
    }, REQUEST_WAIT_MS);
  };

  const dismissHandoff = () => {
    if (handoffTimer.current !== null) {
      clearTimeout(handoffTimer.current);
      handoffTimer.current = null;
    }
    setHandoff(null);
  };

  const noticeWidth = Math.max(0, Math.min(404, width - 2 * 59));
  const bar = model.bar;
  const largeText = PixelRatio.getFontScale() > 2;

  return (
    <>
      {bar !== null && handoff === null && (
        <View
          accessible={bar.kind === 'viewOnly'}
          accessibilityLabel={
            bar.kind === 'viewOnly' ? sharedRelayTcp.shared.viewOnly : undefined
          }
          style={[
            styles.bar,
            { width: noticeWidth },
            largeText ? styles.barLarge : null,
            bar.kind === 'viewOnly' ? styles.viewOnlyBar : null,
          ]}
          testID="shared-bar"
        >
          {bar.kind === 'view' ? (
            <>
              <View style={styles.barText}>
                <AppText weight={600} variant="footnote">
                  {sharedRelayTcp.shared.controlling(bar.controllerName)}
                </AppText>
                <AppText
                  accessibilityLiveRegion="polite"
                  color="text2"
                  testID="shared-bar-status"
                  variant="caption"
                >
                  {waiting
                    ? bar.sameUser
                      ? sharedRelayTcp.shared.takingOver
                      : sharedRelayTcp.shared.waitingFor(bar.controllerName)
                    : bar.sameUser
                    ? sharedRelayTcp.shared.watchingSameUser
                    : sharedRelayTcp.shared.watchingOtherUser}
                </AppText>
              </View>
              <Button
                accessibilityLabel={
                  bar.sameUser
                    ? sharedRelayTcp.shared.takeOver
                    : sharedRelayTcp.shared.requestControl
                }
                disabled={waiting}
                loading={waiting}
                onPress={takeOver}
                testID="shared-take-over"
                title={
                  bar.sameUser
                    ? sharedRelayTcp.shared.takeOver
                    : sharedRelayTcp.shared.requestControl
                }
                variant="primary"
              />
            </>
          ) : (
            <AppText center color="text2" variant="caption">
              {sharedRelayTcp.shared.viewOnly}
            </AppText>
          )}
        </View>
      )}

      {handoff !== null && (
        <View
          style={[
            styles.bar,
            styles.handoffBar,
            largeText ? styles.barLarge : null,
            { width: noticeWidth },
          ]}
          testID="shared-handoff"
        >
          <View
            accessible
            accessibilityLabel={`${sharedRelayTcp.shared.handoffTitle(
              handoff,
            )}。${sharedRelayTcp.shared.handoffBody(handoff)}`}
            accessibilityLiveRegion="polite"
            style={styles.barText}
          >
            <AppText weight={600} variant="footnote">
              {sharedRelayTcp.shared.handoffTitle(handoff)}
            </AppText>
            <AppText color="text2" variant="caption">
              {sharedRelayTcp.shared.handoffBody(handoff)}
            </AppText>
          </View>
          <TextAction
            color="good"
            label={sharedRelayTcp.shared.gotIt}
            onPress={dismissHandoff}
          />
        </View>
      )}

      {model.watchers !== null && (
        <View
          accessible
          accessibilityLabel={model.watchers}
          accessibilityLiveRegion="polite"
          style={styles.watchers}
          testID="shared-watchers"
        >
          <Icon color="text2" name="eye" size={14} />
          <AppText color="text2" variant="caption">
            {model.watchers}
          </AppText>
        </View>
      )}

      <Dialog
        actions={[
          {
            label: sharedRelayTcp.shared.allow,
            onPress: () => answer(true),
          },
          {
            label: sharedRelayTcp.shared.deny,
            onPress: () => answer(false),
            variant: 'secondary',
          },
        ]}
        testID="control-request"
        title={
          visibleRequest === null
            ? ''
            : sharedRelayTcp.shared.requestTitle(
                visibleRequest.requester.deviceName,
              )
        }
        visible={visibleRequest !== null}
      >
        {visibleRequest === null ? null : (
          <View style={styles.requestBody}>
            <AppText color="text2" style={styles.requestText} variant="body">
              {sharedRelayTcp.shared.requestBody(visibleRequest.timeoutS)}
            </AppText>
            <Countdown remaining={remaining} total={visibleRequest.timeoutS} />
          </View>
        )}
      </Dialog>
    </>
  );
}

function TextAction({
  color,
  label,
  onPress,
}: {
  color: 'good' | 'relay';
  label: string;
  onPress: () => void;
}) {
  return (
    <Pressable
      accessibilityLabel={label}
      accessibilityRole="button"
      onPress={onPress}
      style={[styles.textAction, { minHeight: hit.min }]}
    >
      <AppText color={color} variant="button" weight={500}>
        {label}
      </AppText>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  // 共享提示放在会话页顶部的提示栏里，和本机使用条、路径说明依次往下排（R37）。
  bar: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 12,
    minHeight: 56,
    paddingTop: 8,
    paddingRight: 8,
    paddingBottom: 8,
    paddingLeft: 10,
    borderRadius: radius.card,
    borderWidth: 1,
    borderColor: 'rgba(244, 178, 78, 0.35)',
    backgroundColor: 'rgba(28, 24, 16, 0.95)',
  },
  barLarge: {
    flexDirection: 'column',
    alignItems: 'stretch',
  },
  viewOnlyBar: {
    justifyContent: 'center',
    backgroundColor: tokens.hud,
  },
  handoffBar: {
    borderColor: 'rgba(93, 211, 154, 0.35)',
    backgroundColor: 'rgba(14, 26, 20, 0.95)',
  },
  barText: {
    flexShrink: 1,
    gap: 2,
  },
  watchers: {
    minHeight: 28,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    paddingHorizontal: 10,
    borderRadius: 14,
    backgroundColor: tokens.hud,
  },
  requestBody: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 12,
  },
  requestText: {
    flex: 1,
  },
  textAction: {
    justifyContent: 'center',
    paddingHorizontal: 8,
  },
});

export default SharedOverlay;
