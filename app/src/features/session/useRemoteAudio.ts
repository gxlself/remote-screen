import { useEffect, useRef } from 'react';
import { startAudio, setAudioMuted, stopAudio } from '../../native/audio/audio';
import { useCore } from '../../native/rscore/CoreContext';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import type { Computer, SessionId } from '../../native/rscore/types';
import { useSetting } from '../settings/store';
import { audioAvailability } from './remoteAudio';

export function useRemoteAudio(
  sessionId: SessionId | null,
  host: Computer | undefined,
): void {
  const core = useCore();
  const [sound] = useSetting('session.sound');
  const availability = audioAvailability(host);
  const playable = sessionId !== null && sound && availability !== 'needsHost';
  const playing = useRef(false);
  const previous = useRef({ sessionId, sound });
  const loggedUnsupported = useRef(false);
  const loggedStartFailure = useRef(false);

  useEffect(() => {
    if (!playable || sessionId === null) {
      if (playing.current) {
        playing.current = false;
        stopAudio();
      }
      return;
    }
    playing.current = true;
    startAudio(sessionId).catch(() => {
      if (!loggedStartFailure.current) {
        loggedStartFailure.current = true;
        core.logs.write('warn', 'audio', 'startAudio failed');
      }
    });
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
    if (
      sessionId === null ||
      prior.sessionId !== sessionId ||
      prior.sound === sound
    ) {
      return;
    }
    setAudioMuted(!sound);
    try {
      core.sessions.setAudio(sessionId, sound);
    } catch (error) {
      if (!(error instanceof RsCoreError) || error.kind !== 'unsupported') {
        throw error;
      }
      if (!loggedUnsupported.current) {
        loggedUnsupported.current = true;
        core.logs.write('warn', 'audio', 'sessions.setAudio unsupported');
      }
    }
  }, [core, sessionId, sound]);
}
