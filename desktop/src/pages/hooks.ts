import { useEffect, useEffectEvent } from 'react';

import type { CoreEvent } from '@app/native/rscore/types';

import { useCore, useDesktop } from '../core/CoreContext';
import type { DesktopSessionEvent } from '../core/desktopApi';

/** Every RsCore event while the caller is mounted. */
export function useCoreEvents(onEvent: (event: CoreEvent) => void): void {
  const core = useCore();
  const handle = useEffectEvent(onEvent);
  useEffect(() => core.events.subscribe((event) => handle(event)), [core]);
}

/** Every `desktop://session` event while the caller is mounted. */
export function useSessionEvents(onEvent: (event: DesktopSessionEvent) => void): void {
  const desktop = useDesktop();
  const handle = useEffectEvent(onEvent);
  useEffect(() => {
    let live = true;
    let stop: (() => void) | null = null;
    desktop
      .onSession((event) => handle(event))
      .then((unlisten) => {
        if (live) {
          stop = unlisten;
        } else {
          unlisten();
        }
      })
      .catch(() => {
        // Without the shell's session events the list just refreshes on its timer.
      });
    return () => {
      live = false;
      stop?.();
    };
  }, [desktop]);
}
