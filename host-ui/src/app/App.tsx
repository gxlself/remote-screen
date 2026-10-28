import { IpcRoot } from '../ipc';
import { PlatformRoot } from '../platform';

import { RouteView } from './RouteView';

export function App() {
  return (
    <PlatformRoot>
      <IpcRoot>
        <RouteView />
      </IpcRoot>
    </PlatformRoot>
  );
}
