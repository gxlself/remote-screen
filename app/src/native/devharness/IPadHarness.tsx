import { useEffect, useMemo, useRef, useState } from 'react';
import { StyleSheet, Text, View } from 'react-native';
import { KeyCapture } from '../keyboard/KeyCapture';
import { maxFps } from '../platform/impl/system';
import { CoreProvider, useCore } from '../rscore/CoreContext';
import type { RsCoreApi } from '../rscore/RsCoreApi';
import { RemoteSessionView } from '../session/RemoteSessionView';
import type { HarnessProps } from './registry';

export default function IPadHarness({ arg, log }: HarnessProps) {
  const core = useCore();
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [fps, setFps] = useState(0);
  const latestFps = useRef(0);
  const lastLayout = useRef({ width: -1, height: -1 });
  const screenMaxFps = maxFps();

  // Observe the existing P5A wrapper's session boundary without logging text
  // or adding debug callbacks to the shared product wrapper.
  const captureCore = useMemo<RsCoreApi>(
    () => ({
      ...core,
      init: cfg => core.init(cfg),
      sessions: {
        ...core.sessions,
        key: (id, usage, down, mods) => {
          log('IPAD-KEY', { usage, down, mods });
          core.sessions.key(id, usage, down, mods);
        },
      },
    }),
    [core, log],
  );

  useEffect(() => {
    let mounted = true;
    let opened: string | null = null;
    setSessionId(null);
    setFps(0);
    latestFps.current = 0;
    if (arg) {
      core.sessions
        .open(arg, { intent: 'control', mode: 'touch', maxFps: screenMaxFps })
        .then(id => {
          if (!mounted) {
            core.sessions.close(id);
            return;
          }
          opened = id;
          setSessionId(id);
          log('IPAD-OPEN', { ok: true });
        })
        .catch(() => {
          if (mounted) {
            log('IPAD-OPEN', { ok: false });
          }
        });
    } else {
      log('IPAD-OPEN', { error: 'missing hostId' });
    }
    return () => {
      mounted = false;
      if (opened !== null) {
        core.sessions.close(opened);
      }
    };
  }, [arg, core, log, screenMaxFps]);

  useEffect(() => {
    if (sessionId === null) {
      return;
    }
    const timer = setInterval(() => log('IPAD-STATS', { fps: latestFps.current }), 5000);
    return () => clearInterval(timer);
  }, [log, sessionId]);

  return (
    <View
      style={styles.surface}
      onLayout={({ nativeEvent: { layout } }) => {
        const { width, height } = layout;
        if (lastLayout.current.width !== width || lastLayout.current.height !== height) {
          lastLayout.current = { width, height };
          log('IPAD-LAYOUT', { w: width, h: height });
        }
      }}
    >
      <RemoteSessionView
        sessionId={sessionId}
        mode="touch"
        reduceMotion={false}
        toolbarCollapsed
        keyboardInset={0}
        accessibilityLabel="iPad 远端画面，直接触控与放大镜"
        onVideoStats={stats => {
          latestFps.current = stats.fps;
          setFps(stats.fps);
        }}
        onPointerLock={s => log('IPAD-POINTER', s)}
      />
      {sessionId !== null && (
        <CoreProvider core={captureCore}>
          <KeyCapture
            sessionId={sessionId}
            active={false}
            inputMode="keys"
            stickyMods={0}
            os="macos"
          />
        </CoreProvider>
      )}
      <View style={styles.hud} pointerEvents="none">
        <Text style={styles.stats}>{fps} fps / {screenMaxFps} Hz</Text>
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  surface: { ...StyleSheet.absoluteFill, backgroundColor: '#000000' },
  hud: { position: 'absolute', top: 12, right: 12, padding: 8, backgroundColor: '#000000AA' },
  stats: { color: '#FFFFFF', fontFamily: 'GeistMono-Regular', fontSize: 13 },
});
