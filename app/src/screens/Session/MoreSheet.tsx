import { useState } from 'react';
import {
  ScrollView,
  StyleSheet,
  View,
} from 'react-native';
import { useWindowSize } from '../../features/session/WindowContext';
import { Dialog } from '../../components/Dialog';
import { ListGroup, ListRow } from '../../components/List';
import SessionPanel from '../../components/SessionPanel';
import { Switch } from '../../components/Switch';
import { Icon } from '../../components/icons/Icon';
import {
  AUDIO_MIN_HOST,
  audioAvailability,
} from '../../features/session/remoteAudio';
import { useHostPrefs, useSetting } from '../../features/settings/store';
import { useScreenReader } from '../../features/session/useScreenReader';
import { sendFeedbackAction } from '../../features/session/actionFeedback';
import { useViewport } from '../../features/session/videoStore';
import { sessionToolbar } from '../../i18n/zh/sessionToolbar';
import { clipboardMore } from '../../i18n/zh/clipboardMore';
import { useCore } from '../../native/rscore/CoreContext';
import type { SessionId } from '../../native/rscore/types';
import { space } from '../../theme/tokens';
import type { SheetProps } from './slots';
import { usePipMoreRow } from '../../features/pip/PipMoreRow';
import { useRecordingMoreRow } from '../../features/recording/RecordingMoreRow';

export function MoreSheet({ ctx, onClose }: SheetProps) {
  const core = useCore();
  const { width, height } = useWindowSize();
  const [lockOnEnd, setHostPrefs] = useHostPrefs(ctx.hostId);
  const [naturalScroll, setNaturalScroll] = useSetting('session.naturalScroll');
  const [sound, setSound] = useSetting('session.sound');
  const audioState = audioAvailability(ctx.host);
  const copy = clipboardMore.more;
  const screenReaderEnabled = useScreenReader();
  const sessionId: SessionId | null = ctx.sessionId;
  const pipRow = usePipMoreRow(ctx, onClose);
  const recordingRow = useRecordingMoreRow(ctx, onClose);
  const viewport = useViewport(sessionId);
  const zoomed = viewport !== null && viewport.zoom > 1.01;
  const [confirmLock, setConfirmLock] = useState(false);
  const action = (kind: 'lock' | 'lockOnEnd', enable?: boolean): boolean => {
    if (sessionId === null) {
      return false;
    }
    try {
      core.sessions.action(sessionId, kind, enable);
    } catch {
      return false;
    }
    return true;
  };
  const lock = () => {
    setConfirmLock(false);
    if (action('lock')) {
      ctx.showToast(copy.lockedToast(ctx.hostName));
    } else {
      ctx.showToast(copy.lockFailed, { tone: 'bad' });
    }
    onClose();
  };
  const ctrlAltDel = () => {
    if (sessionId === null) {
      return;
    }
    // 和工具栏同一条路：结果由 ActionFeedback 提示。
    if (!sendFeedbackAction(core, sessionId, 'ctrlAltDel')) {
      ctx.showToast(sessionToolbar.ctrlAltDelFailed, { tone: 'bad' });
    }
    onClose();
  };
  const endLock = (value: boolean) => {
    setHostPrefs({ lockOnEnd: value });
    action('lockOnEnd', value);
  };

  return (
    <SessionPanel
      backdropLabel={copy.backdrop}
      kind="popover"
      onClose={onClose}
      testID="more-sheet"
      title={copy.title}
    >
      <ScrollView
        contentContainerStyle={styles.content}
        style={{ maxHeight: Math.max(180, height - 132) }}
      >
        <ListGroup>
          <ListRow
            left={<Icon color="text2" name="lock" size={20} />}
            onPress={() => setConfirmLock(true)}
            title={copy.lockRemote}
          />
          {ctx.os === 'windows' ? (
            <ListRow
              accessibilityLabel={copy.ctrlAltDelA11y}
              left={<Icon color="text2" name="keyboard" size={20} />}
              onPress={ctrlAltDel}
              title={copy.ctrlAltDel}
            />
          ) : null}
          {ctx.view.toolbar.displayA11y === null ? null : (
            <ListRow
              accessibilityLabel={ctx.view.toolbar.displayA11y}
              left={<Icon color="text2" name="displays" size={20} />}
              onPress={() => ctx.openSheet('displays')}
              testID="more-displays"
              title={copy.displays}
            />
          )}
          <ListRow
            left={<Icon color="text2" name="swipeHand" size={20} />}
            onPress={() => {
              onClose();
              ctx.openCoach();
            }}
            title={copy.coach}
          />
          {pipRow}
          {recordingRow}
          <ListRow
            disabled={!zoomed && !screenReaderEnabled}
            left={<Icon color="text2" name="zoomIn" size={20} />}
            onPress={() => {
              ctx.videoRef.current?.zoomReset();
              onClose();
            }}
            subtitle={
              zoomed || screenReaderEnabled ? undefined : copy.zoomResetIdle
            }
            testID="more-zoom-reset"
            title={copy.zoomReset}
          />
          {width < 700 ? (
            <ListRow
              left={<Icon color="text2" name="clipboard" size={20} />}
              onPress={() => {
                ctx.openSheet('clipboard');
              }}
              title={copy.clipboard}
            />
          ) : null}
          <ListRow
            right={
              <Switch
                accessibilityLabel={copy.lockOnEnd}
                onValueChange={endLock}
                value={lockOnEnd.lockOnEnd}
              />
            }
            subtitle={copy.lockOnEndSubtitle}
            title={copy.lockOnEnd}
          />
          <ListRow
            right={
              <Switch
                accessibilityLabel={copy.sound}
                disabled={audioState === 'needsHost'}
                onValueChange={setSound}
                value={sound}
              />
            }
            subtitle={
              audioState === 'needsHost'
                ? copy.soundNeedsHost(AUDIO_MIN_HOST)
                : sound
                ? copy.soundOn
                : copy.soundOff
            }
            title={copy.sound}
          />
          <ListRow
            right={
              <Switch
                accessibilityLabel={copy.naturalScroll}
                onValueChange={setNaturalScroll}
                value={naturalScroll}
              />
            }
            subtitle={
              naturalScroll
                ? copy.naturalScrollOn[ctx.os]
                : copy.naturalScrollOff[ctx.os]
            }
            title={copy.naturalScroll}
          />
        </ListGroup>
        <View style={styles.disconnect}>
          <ListRow
            left={<Icon color="bad" name="blocked" size={20} />}
            onPress={ctx.disconnect}
            title={copy.disconnect}
            tone="danger"
          />
        </View>
      </ScrollView>
      <Dialog
        actions={[
          {
            label: copy.cancel,
            onPress: () => setConfirmLock(false),
            variant: 'secondary',
          },
          {
            label: copy.lockConfirm,
            onPress: lock,
            testID: 'more-lock-confirm',
          },
        ]}
        message={copy.lockConfirmBody[ctx.os]}
        onDismiss={() => setConfirmLock(false)}
        testID="more-lock-dialog"
        title={copy.lockConfirmTitle(ctx.hostName)}
        visible={confirmLock}
      />
    </SessionPanel>
  );
}

const styles = StyleSheet.create({
  content: { gap: space.md, paddingBottom: 10 },
  disconnect: { gap: 8 },
});

export default MoreSheet;
