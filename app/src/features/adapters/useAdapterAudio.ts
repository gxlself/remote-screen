import { useEffect, useRef } from 'react';
import { setAudioMuted, startAudio, stopAudio } from '../../native/audio/audio';
import { useCore } from '../../native/rscore/CoreContext';
import type { AdapterProtocol, SessionId } from '../../native/rscore/types';
import { useSetting } from '../settings/store';

/**
 * Plays an RDP session's remote sound through the shared native player
 * (C13.4). VNC carries no sound. Adapter sessions have no
 * `sessions.setAudio`, so toggling the setting only mutes locally.
 */
export function useAdapterAudio(
  sessionId: SessionId | null,
  protocol: AdapterProtocol,
  streaming: boolean,
  onStartFailed?: () => void,
): void {
  const core = useCore();
  const [sound] = useSetting('session.sound');
  const audible = protocol === 'rdp' && streaming && sessionId !== null;
  const playable = audible && sound;
  const playing = useRef(false);
  const previous = useRef({ sessionId, sound });
  const loggedStartFailure = useRef(false);
  const onStartFailedRef = useRef(onStartFailed);
  onStartFailedRef.current = onStartFailed;

  useEffect(() => {
    if (!playable || sessionId === null) {
      if (playing.current) {
        playing.current = false;
        stopAudio();
      }
      return;
    }
    playing.current = true;
    // startAudio reports native errors as `false`.
    const failed = () => {
      if (!loggedStartFailure.current) {
        loggedStartFailure.current = true;
        core.logs.write('warn', 'audio', 'adapter startAudio failed');
        onStartFailedRef.current?.();
      }
    };
    startAudio(sessionId).then(started => {
      if (!started) {
        failed();
      }
    }, failed);
    return () => {
      if (playing.current) {
        playing.current = false;
        stopAudio();
      }
    };
  }, [core, playable, sessionId]);

  useEffect(() => {
    const prior = previous.current;
    previous.current = { sessionId, sound };
    if (!audible || prior.sessionId !== sessionId || prior.sound === sound) {
      return;
    }
    setAudioMuted(!sound);
  }, [audible, sessionId, sound]);
}
