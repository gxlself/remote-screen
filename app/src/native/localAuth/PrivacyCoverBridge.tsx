import { useEffect } from 'react';
import { useSetting } from '../../features/settings/store';
import { setPrivacyCover } from './localAuth';

/** Keeps sensitive remote content out of the app switcher when App Lock is on. */
export default function PrivacyCoverBridge(): null {
  const [onOpen] = useSetting('lock.onOpen');
  const [beforeSession] = useSetting('lock.beforeSession');
  const enabled = onOpen || beforeSession;

  useEffect(() => {
    setPrivacyCover(enabled);
    return () => {
      setPrivacyCover(false);
    };
  }, [enabled]);

  return null;
}
