import { useEffect, useState } from 'react';
import { Button, ScrollView, StyleSheet, Text, View } from 'react-native';
import type { EventSubscription } from 'react-native';
import { tokens } from '../../theme/tokens';
import type { HarnessProps } from './registry';
import NativeRsAudio from '../specs/NativeRsAudio';
import type { AudioStats, AudioStateEvent } from '../specs/NativeRsAudio';
import { setAudioMuted, startAudio, stopAudio } from '../audio/audio';

const EMPTY_STATS: AudioStats = {
  running: false,
  packets: 0,
  lost: 0,
  concealed: 0,
  underruns: 0,
  targetMs: 0,
  levelMs: 0,
  outputLatencyMs: 0,
  ioBufferMs: 0,
  route: '-',
};

export default function AudioHarness({ arg, log }: HarnessProps) {
  const sessionId = arg ?? '';
  const [state, setState] = useState<AudioStateEvent>({
    state: 'idle',
    reason: NativeRsAudio == null ? 'unavailable' : '',
  });
  const [stats, setStats] = useState<AudioStats>(EMPTY_STATS);
  const [statsError, setStatsError] = useState('');

  useEffect(() => {
    let mounted = true;
    let refreshing = false;
    const refresh = () => {
      if (NativeRsAudio == null || refreshing) return;
      refreshing = true;
      NativeRsAudio.getStats()
        .then(value => {
          if (mounted) {
            setStats(value);
            setStatsError('');
          }
        })
        .catch(() => {
          if (mounted) setStatsError('stats-unavailable');
        })
        .finally(() => { refreshing = false; });
    };
    refresh();
    const timer = setInterval(refresh, 500);
    const subscription = NativeRsAudio?.onAudioState((value: AudioStateEvent) => {
      if (mounted) {
        setState(value);
        log('AUDIO-STATE', value);
      }
    }) as EventSubscription | undefined;
    return () => {
      mounted = false;
      clearInterval(timer);
      subscription?.remove();
    };
  }, [log]);

  const run = async (action: string, operation: () => Promise<unknown> | void) => {
    try {
      const result = await operation();
      log(`AUDIO-${action}`, { sessionId, ok: result !== false });
    } catch {
      log(`AUDIO-${action}`, { sessionId, ok: false });
    }
  };

  return (
    <ScrollView>
      <Text style={styles.text}>session: {sessionId || '-'}</Text>
      <Text style={styles.text}>state: {state.state} ({state.reason || '-'})</Text>
      <Text style={styles.stats}>{JSON.stringify(stats, null, 2)}</Text>
      {statsError !== '' && <Text style={styles.error}>{statsError}</Text>}
      <View style={styles.actions}>
        <Button
          title="开始"
          disabled={sessionId === ''}
          onPress={() => run('START', () => startAudio(sessionId))}
        />
        <Button title="停止" onPress={() => run('STOP', stopAudio)} />
        <Button
          title="静音"
          onPress={() => run('MUTE', () => setAudioMuted(true))}
        />
        <Button
          title="取消静音"
          onPress={() => run('UNMUTE', () => setAudioMuted(false))}
        />
      </View>
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  text: { color: tokens.text, fontSize: 14, marginBottom: 8 },
  stats: { color: tokens.text2, fontSize: 12, fontFamily: 'GeistMono-Regular', marginBottom: 12 },
  error: { color: tokens.bad, fontSize: 14, marginBottom: 8 },
  actions: { flexDirection: 'row', flexWrap: 'wrap', gap: 8 },
});
