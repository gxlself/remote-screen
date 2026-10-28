import { getSetting, useSetting } from '../settings/store';

/**
 * Tailscale is opt-in (Settings › 网络接入 › 外网访问). Off — the default, and
 * what an App Store user sees — the app is LAN and company-VPN only: no
 * Tailscale wording anywhere, and nothing in the background talks to a tailnet.
 */
export function useTailnetEnabled(): boolean {
  return useSetting('tailnet.enabled')[0];
}

/** The same switch for code outside React (startup, queries, copy helpers). */
export function isTailnetEnabled(): boolean {
  return getSetting('tailnet.enabled');
}
