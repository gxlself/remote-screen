import '@fontsource-variable/geist';
import '@fontsource-variable/geist-mono';
import '@host-ui/theme/global.css';

import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';

import { createRsCore } from '@app/native/rscore/createRsCore';

import { desktopBindings } from './core/bindings';
import { desktopApi } from './core/desktopApi';
import { fakeTransport } from './core/fake';
import { isTauri, tauriTransport } from './core/tauri';
import { Root } from './Root';

const container = document.getElementById('root');
if (container === null) {
  throw new Error('Missing #root element');
}

// Without Tauri (`pnpm dev` in a browser) the fake shell serves preset paired computers.
const transport = isTauri() ? tauriTransport() : fakeTransport();
const core = createRsCore(desktopBindings(transport));
const desktop = desktopApi(transport);

createRoot(container).render(
  <StrictMode>
    <Root core={core} desktop={desktop} />
  </StrictMode>,
);
