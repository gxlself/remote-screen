import type { JSX } from 'react';

import type { RsCoreApi } from '@app/native/rscore/RsCoreApi';

import { CoreProvider } from './core/CoreContext';
import type { DesktopApi } from './core/desktopApi';
import { StartupScreen } from './onboarding/StartupScreen';
import { useStartup } from './onboarding/useStartup';
import { Pages } from './pages';

export function Root({ core, desktop }: { core: RsCoreApi; desktop: DesktopApi }): JSX.Element {
  const { state, retry } = useStartup(core, desktop);
  return (
    <CoreProvider core={core} desktop={desktop}>
      {state.phase === 'ready' ? <Pages /> : <StartupScreen onRetry={retry} state={state} />}
    </CoreProvider>
  );
}
