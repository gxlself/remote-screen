export {
  canCreate,
  statusText,
  VDISPLAY_TIMEOUT_MS,
  virtualDisplayOpts,
} from './request';
export type { ScreenInfo } from './request';
export { resetVdisplayStore, useVdisplayStore } from './store';
export type { VdisplayPending, VdisplayState } from './store';
export {
  useVirtualDisplay,
  useVirtualDisplayWatcher,
} from './useVirtualDisplay';
export type { VdState } from './useVirtualDisplay';
