import { useEffect, useRef, useState } from 'react';
import { StyleSheet, View } from 'react-native';
import { useCore } from '../rscore/CoreContext';
import { RemoteSessionView } from '../session/RemoteSessionView';
import type { HarnessProps } from './registry';

export default function SessionViewHarness({ arg, log }: HarnessProps) {
  const core = useCore();
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [mode, setMode] = useState<'trackpad' | 'touch'>('trackpad');
  const [keyboardInset, setKeyboardInset] = useState(0);
  const videoStatsCount = useRef(0);

  useEffect(() => {
    let mounted = true;
    let opened: string | null = null;
    videoStatsCount.current = 0;
    setSessionId(null);
    if (arg) {
      core.sessions
        .open(arg, { intent: 'control' })
        .then(id => {
          opened = id;
          if (!mounted) {
            core.sessions.close(id);
            return;
          }
          setSessionId(id);
          log('SESSVIEW-OPEN', { sessionId: id });
        })
        .catch(error => {
          log('SESSVIEW-OPEN', { error: String(error) });
        });
    } else {
      log('SESSVIEW-OPEN', { error: 'missing hostId' });
    }
    return () => {
      mounted = false;
      if (opened !== null) {
        core.sessions.close(opened);
      }
    };
  }, [arg, core, log]);

  return (
    <View style={styles.surface}>
      <RemoteSessionView
        sessionId={sessionId}
        mode={mode}
        reduceMotion={false}
        toolbarCollapsed
        keyboardInset={keyboardInset}
        accessibilityLabel="远端画面，双指双击切换直接触控"
        onFirstFrame={() => log('SESSVIEW-FIRSTFRAME', { sessionId })}
        onGesture={({ kind }) => {
          log('SESSVIEW-GESTURE', { kind });
          if (kind === 'twoFingerDoubleTap') {
            setMode(value => (value === 'trackpad' ? 'touch' : 'trackpad'));
          } else if (kind === 'threeFingerTap') {
            setKeyboardInset(value => (value === 0 ? 300 : 0));
          }
        }}
        onViewport={v => log('SESSVIEW-VIEWPORT', v)}
        onVideoStats={s => {
          videoStatsCount.current += 1;
          if (videoStatsCount.current % 5 === 0) {
            log('SESSVIEW-STATS', s);
          }
        }}
      />
    </View>
  );
}

const styles = StyleSheet.create({
  surface: { ...StyleSheet.absoluteFill, backgroundColor: '#000000' },
});
