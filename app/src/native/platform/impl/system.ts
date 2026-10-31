import NativeRsSystem from '../../specs/NativeRsSystem';

const NOTIFICATION_RESULTS = new Set(['granted', 'denied']);
const LOCAL_NETWORK_RESULTS = new Set(['granted', 'denied', 'unknown']);

function constants() {
  return NativeRsSystem?.getConstants();
}

export async function copyText(text: string): Promise<void> {
  NativeRsSystem?.copyText(text);
}

/** Null where unknown (Android, or no native module). */
export function clipboardChangeCount(): number | null {
  const count = NativeRsSystem?.clipboardChangeCount();
  return count === undefined || count < 0 ? null : count;
}

export async function readText(): Promise<string | null> {
  if (NativeRsSystem == null) {
    return null;
  }
  try {
    return await NativeRsSystem.readText();
  } catch {
    return null;
  }
}

export function haptic(
  kind: 'light' | 'success' | 'warning' | 'error',
): void {
  NativeRsSystem?.haptic(kind);
}

export async function requestNotifications(): Promise<'granted' | 'denied'> {
  if (NativeRsSystem == null) {
    return 'denied';
  }
  try {
    const result = await NativeRsSystem.requestNotifications();
    return NOTIFICATION_RESULTS.has(result)
      ? (result as 'granted' | 'denied')
      : 'denied';
  } catch {
    return 'denied';
  }
}

export async function requestLocalNetwork(): Promise<
  'granted' | 'denied' | 'unknown'
> {
  if (NativeRsSystem == null) {
    return 'unknown';
  }
  try {
    const result = await NativeRsSystem.requestLocalNetwork(30000);
    return LOCAL_NETWORK_RESULTS.has(result)
      ? (result as 'granted' | 'denied' | 'unknown')
      : 'unknown';
  } catch {
    return 'unknown';
  }
}

export function appVersion(): string {
  const value = constants()?.appVersion;
  return typeof value === 'string' ? value : '0.0.0';
}

export function maxFps(): number {
  const value = constants()?.maxFps;
  return typeof value === 'number' && Number.isFinite(value) && value > 0
    ? value
    : 60;
}

export function modelId(): string {
  const value = constants()?.modelId;
  return typeof value === 'string' ? value : 'unknown';
}
