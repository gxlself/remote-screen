import { useEffect, useRef, useState } from 'react';
import {
  AccessibilityInfo,
  Platform,
  StatusBar,
  StyleSheet,
  View,
} from 'react-native';
import type { RootScreenProps } from '../../app/navigation/routes';
import { Dialog } from '../../components/Dialog';
import { Toast } from '../../components/Toast';
import { session } from '../../i18n/zh/session';
import { vdisplay } from '../../i18n/zh/vdisplay';
import { useVirtualDisplayWatcher } from '../../features/vdisplay';
import { useRemoteAudio } from '../../features/session/useRemoteAudio';
import { useViewport } from '../../features/session/videoStore';
import type { SessionId } from '../../native/rscore/types';
import { useScreenReader } from '../../features/session/useScreenReader';
import ActionFeedback from './ActionFeedback';
import ApprovalOverlay from './ApprovalOverlay';
import ClipboardSheet from './ClipboardSheet';
import ClipboardSync from './ClipboardSync';
import ConnectingOverlay from './ConnectingOverlay';
import DisplaySheet from './DisplaySheet';
import FollowZoomChip from './FollowZoomChip';
import GestureCoach from './GestureCoach';
import IdleOverlay from './IdleOverlay';
import KeyboardSlot from './KeyboardSlot';
import LocalUseBar from './LocalUseBar';
import MoreSheet from './MoreSheet';
import PathNotice, { PathCaption } from './PathNotice';
import PointerLockChip from './PointerLockChip';
import QualitySheet from './QualitySheet';
import ReconnectOverlay from './ReconnectOverlay';
import SessionHud from './SessionHud';
import SessionMinimap from './SessionMinimap';
import SessionToolbar from './SessionToolbar';
import SharedOverlay from './SharedOverlay';
import ToolbarHandle from './ToolbarHandle';
import { TOP_STACK_TOP, TopStackContext, belowTopStack } from './topStack';
import VideoSlot from './VideoSlot';
import { useAndroidBackPress } from './useAndroidBackPress';
import { useAndroidPip } from './useAndroidPip';
import { useIosPip } from '../../features/pip/useIosPip';
import PipPlaceholder from '../../features/pip/PipPlaceholder';
import { useSessionController } from './useSessionController';
import { useToolbarIdle } from './useToolbarIdle';
import { useSessionRecording } from '../../features/recording/useSessionRecording';
import SessionSwitcher from '../../features/session/SessionSwitcher';
import SpaceIndicator from '../../features/session/SpaceIndicator';

/** 只有放大时才显示「跟随输入位置」，没放大时不占位置也不误导。 */
function ZoomChipSlot({ sessionId }: { sessionId: SessionId | null }) {
  const viewport = useViewport(sessionId);
  if (viewport === null || viewport.zoom <= ZOOMED_MIN) {
    return null;
  }
  return <FollowZoomChip sessionId={sessionId} />;
}

const ZOOMED_MIN = 1.01;

const HIDDEN_FROM_A11Y = {
  accessibilityElementsHidden: true,
  importantForAccessibility: 'no-hide-descendants',
} as const;

/**
 * A switch to another Host session swaps the route's params in place
 *; the key remounts the page for it, as a new page would.
 */
export default function SessionScreen(props: RootScreenProps<'Session'>) {
  return <SessionPage key={props.route.params.switchSeq ?? 0} {...props} />;
}

function SessionPage(props: RootScreenProps<'Session'>) {
  const ctx = useSessionController(props);
  const screenReader = useScreenReader();
  useRemoteAudio(ctx.sessionId, ctx.host);
  useAndroidBackPress(ctx);
  useAndroidPip(ctx);
  useIosPip(ctx);
  useSessionRecording(ctx);
  const { view, sheet } = ctx;
  // 显示器面板关着时，新建虚拟屏的结果由这里提示。
  useVirtualDisplayWatcher({
    sessionId: ctx.sessionId,
    enabled: sheet !== 'displays',
    onCreated: () => ctx.showToast(vdisplay.created, { tone: 'good' }),
    onError: text => ctx.showToast(text, { tone: 'bad' }),
  });
  // switched here, the last picture covers the wait instead.
  const connecting =
    view.overlay === 'opening' ||
    view.overlay === 'connecting' ||
    (ctx.rec?.phase === 'streaming' &&
      ctx.firstFrameAtMs === null &&
      !ctx.spaces.showSnapshot);
  const toolbarHidden = ctx.toolbarHidden && !screenReader;
  // 闲置倒计时压在面板和手势说明之上，开着面板也看得到。
  const idleShown =
    view.overlay === 'none' && !connecting && ctx.rec?.idleWarning != null;
  const top = idleShown
    ? 'idle'
    : ctx.coachVisible
    ? 'coach'
    : sheet !== null
    ? 'sheet'
    : view.overlay === 'reconnecting'
    ? 'reconnect'
    : view.overlay === 'approval'
    ? 'approval'
    : connecting
    ? 'connecting'
    : null;
  const modalA11yHidden = screenReader && top !== null;
  const [measuredTopStack, setTopStackHeight] = useState(0);
  // 提示栏只在没有整屏浮层时渲染；浮层盖住时按没有提示算。
  const topStackHeight = view.overlay === 'none' ? measuredTopStack : 0;
  const toolbarIdle = useToolbarIdle(
    view.overlay === 'none' &&
      !connecting &&
      sheet === null &&
      !ctx.keyboardOpen &&
      !ctx.coachVisible &&
      !screenReader &&
      !toolbarHidden,
  );
  const announcedSession = useRef<string | null>(null);
  const previousConnecting = useRef(connecting);

  useEffect(() => {
    const wasConnecting = previousConnecting.current;
    previousConnecting.current = connecting;
    if (
      !wasConnecting ||
      connecting ||
      view.overlay !== 'none' ||
      ctx.sessionId === null ||
      announcedSession.current === ctx.sessionId
    ) {
      return;
    }
    announcedSession.current = ctx.sessionId;
    // There is no live region for this transient shell state, so announce on
    // both platforms when the connection becomes usable.
    AccessibilityInfo.announceForAccessibility(
      session.a11y.connected(ctx.hostName),
    );
  }, [connecting, ctx.hostName, ctx.sessionId, view.overlay]);

  const escape = () => {
    if (idleShown) {
      return;
    }
    if (ctx.sheet !== null) {
      ctx.closeSheet();
      return;
    }
    if (ctx.keyboardOpen) {
      ctx.closeKeyboard();
      return;
    }
    if (ctx.coachVisible) {
      ctx.closeCoach();
      return;
    }
    if (connecting || view.overlay === 'approval') {
      ctx.cancelConnect();
      return;
    }
    if (view.overlay === 'reconnecting' || ctx.rec?.idleWarning != null) {
      return;
    }
    ctx.collapse();
  };

  return (
    <TopStackContext.Provider value={topStackHeight}>
      <View
        onAccessibilityEscape={escape}
        onMagicTap={() => {
          if (top === null) {
            ctx.onGesture({ kind: 'twoFingerDoubleTap' });
          }
        }}
        onTouchStart={toolbarIdle.wake}
        style={styles.root}
        testID="session-root"
      >
        <StatusBar hidden />
        {ctx.sessionId !== null && <ClipboardSync ctx={ctx} />}
        {ctx.sessionId !== null && <ActionFeedback ctx={ctx} />}
        <View
          {...(modalA11yHidden ? HIDDEN_FROM_A11Y : null)}
          testID="layer-video"
          pointerEvents="box-none"
          style={styles.layer}
        >
          <VideoSlot
            ref={ctx.videoRef}
            sessionId={ctx.sessionId}
            mode={ctx.mode}
            reduceMotion={ctx.reduceMotion}
            toolbarCollapsed={toolbarHidden}
            keyboardInset={ctx.keyboardInset}
            accessibilityLabel={(Platform.OS === 'android'
              ? session.videoLabelAndroid
              : session.videoLabel)[ctx.mode](ctx.hostName)}
            onGesture={ctx.onGesture}
            onFirstFrame={ctx.onFirstFrame}
            spaces={ctx.spaces.spaces}
            showSnapshot={ctx.spaces.showSnapshot}
            onSpaceSwipe={ctx.spaces.onSpaceSwipe}
            onSpaceSwitch={ctx.spaces.onSpaceSwitch}
          />
        </View>
        {ctx.spaces.indicator === null ? null : (
          <SpaceIndicator
            fading={ctx.spaces.indicator.fading}
            highlightId={ctx.spaces.indicator.highlightId}
            reduceMotion={ctx.reduceMotion}
            spaces={ctx.spaces.spaces}
          />
        )}
        {view.overlay === 'none' && (
          <View
            {...(modalA11yHidden ? HIDDEN_FROM_A11Y : null)}
            testID="layer-status"
            pointerEvents="box-none"
            style={styles.layer}
          >
            {/* 顶部提示栏：各条提示依次往下排，不互相叠压（R37）。 */}
            <View
              onLayout={event =>
                setTopStackHeight(Math.round(event.nativeEvent.layout.height))
              }
              pointerEvents="box-none"
              style={styles.topStack}
              testID="top-stack"
            >
              <PathCaption ctx={ctx} />
              <LocalUseBar ctx={ctx} />
              <SharedOverlay ctx={ctx} />
            </View>
            <PathNotice ctx={ctx} />
            <PointerLockChip ctx={ctx} />
            <PipPlaceholder ctx={ctx} />
          </View>
        )}
        {view.overlay === 'none' && (
          <View
            {...(modalA11yHidden ? HIDDEN_FROM_A11Y : null)}
            testID="layer-hud"
            pointerEvents="box-none"
            style={styles.layer}
          >
            <SessionHud ctx={ctx} />
          </View>
        )}
        {view.overlay === 'none' && (
          <View
            {...(modalA11yHidden ? HIDDEN_FROM_A11Y : null)}
            testID="layer-minimap"
            pointerEvents="box-none"
            style={styles.layer}
          >
            <SessionMinimap ctx={ctx} />
          </View>
        )}
        {view.overlay === 'none' && !toolbarHidden && (
          <View
            {...(modalA11yHidden ? HIDDEN_FROM_A11Y : null)}
            testID="layer-toolbar"
            // 空闲淡出后不挡触摸，点到的是电脑的 Dock / 开始按钮；任何触摸都会让它回来。
            pointerEvents={toolbarIdle.faded ? 'none' : 'box-none'}
            style={[styles.layer, toolbarIdle.faded ? styles.faded : null]}
          >
            <SessionToolbar ctx={ctx} />
          </View>
        )}
        {view.overlay === 'none' && toolbarHidden && (
          <View
            {...(modalA11yHidden ? HIDDEN_FROM_A11Y : null)}
            testID="layer-toolbar-handle"
            pointerEvents="box-none"
            style={styles.layer}
          >
            <ToolbarHandle
              bottom={
                HANDLE_BOTTOM + (ctx.keyboardOpen ? ctx.keyboardInset : 0)
              }
              onPress={ctx.showToolbar}
            />
          </View>
        )}
        <View
          {...(modalA11yHidden ? HIDDEN_FROM_A11Y : null)}
          testID="layer-keyboard"
          pointerEvents="box-none"
          style={styles.layer}
        >
          <KeyboardSlot
            open={ctx.keyboardOpen}
            sessionId={ctx.sessionId}
            os={ctx.os}
            leading={<ZoomChipSlot sessionId={ctx.sessionId} />}
            onClose={ctx.closeKeyboard}
            onShortcut={ctx.onShortcut}
          />
        </View>
        {connecting && (
          <View
            {...(modalA11yHidden && top !== 'connecting'
              ? HIDDEN_FROM_A11Y
              : null)}
            testID="layer-connecting"
            pointerEvents="box-none"
            style={styles.layer}
          >
            <ConnectingOverlay ctx={ctx} onRetry={ctx.retryConnect} />
          </View>
        )}
        {view.overlay === 'approval' && (
          <View
            {...(modalA11yHidden && top !== 'approval'
              ? HIDDEN_FROM_A11Y
              : null)}
            testID="layer-approval"
            pointerEvents="box-none"
            style={styles.layer}
          >
            <ApprovalOverlay ctx={ctx} />
          </View>
        )}
        {view.overlay === 'reconnecting' && (
          <View
            {...(modalA11yHidden && top !== 'reconnect'
              ? HIDDEN_FROM_A11Y
              : null)}
            testID="layer-reconnect"
            pointerEvents="box-none"
            style={styles.layer}
          >
            <ReconnectOverlay ctx={ctx} />
          </View>
        )}
        {sheet !== null && (
          <View
            {...(modalA11yHidden && top !== 'sheet' ? HIDDEN_FROM_A11Y : null)}
            testID="layer-sheet"
            pointerEvents="box-none"
            style={styles.layer}
          >
            {sheet === 'quality' && (
              <QualitySheet ctx={ctx} onClose={ctx.closeSheet} />
            )}
            {sheet === 'displays' && (
              <DisplaySheet ctx={ctx} onClose={ctx.closeSheet} />
            )}
            {sheet === 'clipboard' && (
              <ClipboardSheet ctx={ctx} onClose={ctx.closeSheet} />
            )}
            {sheet === 'more' && (
              <MoreSheet ctx={ctx} onClose={ctx.closeSheet} />
            )}
            {sheet === 'switcher' && (
              <SessionSwitcher
                currentId={ctx.sessionId}
                currentNewWindow={ctx.inMainWindow}
                elsewhereIds={ctx.elsewhereIds}
                list={ctx.switcherList}
                onClose={ctx.closeSheet}
                onOther={ctx.connectOther}
                onSelect={ctx.switchTo}
                shortcuts={ctx.isPad}
                {...(ctx.openNewWindow === undefined
                  ? {}
                  : { onNewWindow: ctx.openNewWindow })}
              />
            )}
          </View>
        )}
        {ctx.coachVisible && (
          <View
            {...(modalA11yHidden && top !== 'coach' ? HIDDEN_FROM_A11Y : null)}
            testID="layer-coach"
            pointerEvents="box-none"
            style={styles.layer}
          >
            <GestureCoach ctx={ctx} />
          </View>
        )}
        {view.overlay === 'none' && (
          <View
            {...(modalA11yHidden && top !== 'idle' ? HIDDEN_FROM_A11Y : null)}
            testID="layer-idle"
            pointerEvents="box-none"
            style={styles.layer}
          >
            <IdleOverlay ctx={ctx} />
          </View>
        )}
        {ctx.disconnectPrompt === null ? null : (
          <Dialog
            actions={[
              {
                label: session.disconnect.cancel,
                onPress: ctx.dismissDisconnect,
                variant: 'secondary',
              },
              {
                label: session.disconnect.confirm,
                onPress: ctx.confirmDisconnect,
                variant: 'pillDanger',
              },
            ]}
            message={
              ctx.disconnectPrompt.recording
                ? session.disconnect.bodyRecording
                : session.disconnect.body
            }
            onDismiss={ctx.dismissDisconnect}
            testID="disconnect-confirm"
            title={session.disconnect.title(ctx.hostName)}
            visible
          />
        )}
        {ctx.toast !== null && (
          <Toast
            offset={belowTopStack(topStackHeight)}
            onHide={ctx.hideToast}
            position="top"
            toast={ctx.toast}
          />
        )}
      </View>
    </TopStackContext.Provider>
  );
}

const HANDLE_BOTTOM = 10;

const styles = StyleSheet.create({
  faded: { opacity: 0 },
  root: { flex: 1, backgroundColor: '#000' },
  layer: StyleSheet.absoluteFill,
  topStack: {
    position: 'absolute',
    top: TOP_STACK_TOP,
    left: 0,
    right: 0,
    alignItems: 'center',
    gap: 8,
  },
});
