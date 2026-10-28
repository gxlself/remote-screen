import type {
  AuthCapability,
  AuthResult,
  PlatformServices,
} from './PlatformServices';
import type { DevicePlatform } from '../rscore/types';

export type FakeDeviceInfo = {
  platform: DevicePlatform;
  isPad: boolean;
  model: string;
  osVersion: string;
  appVersion: string;
};

type Call = { method: string; args: unknown[] };

const DEFAULT_INFO: FakeDeviceInfo = {
  platform: 'ios',
  isPad: false,
  model: 'iPhone',
  osVersion: '26.1',
  appVersion: '0.3.1',
};

export class FakePlatform implements PlatformServices {
  readonly calls: Call[] = [];
  readonly copied: string[] = [];
  readonly opened: string[] = [];

  authResult: AuthResult = 'ok';
  authCapabilityResult: AuthCapability = {
    biometry: 'faceId',
    passcode: true,
  };
  openUrlResult = true;
  info: FakeDeviceInfo = { ...DEFAULT_INFO };
  clipboardText: string | null = null;

  private record(method: string, ...args: unknown[]): void {
    this.calls.push({ method, args });
  }

  async authenticate(reason: string): Promise<AuthResult> {
    this.record('authenticate', reason);
    return this.authResult;
  }

  authCapability(): AuthCapability {
    this.record('authCapability');
    return this.authCapabilityResult;
  }

  async copyText(text: string): Promise<void> {
    this.record('copyText', text);
    this.copied.push(text);
  }

  async readText(): Promise<string | null> {
    this.record('readText');
    return this.clipboardText;
  }

  async openUrl(url: string): Promise<boolean> {
    this.record('openUrl', url);
    this.opened.push(url);
    return this.openUrlResult;
  }

  async canOpenUrl(url: string): Promise<boolean> {
    this.record('canOpenUrl', url);
    return this.openUrlResult;
  }

  async share(opts: {
    url?: string;
    text?: string;
    title?: string;
  }): Promise<void> {
    this.record('share', opts);
  }

  haptic(kind: 'light' | 'success' | 'warning' | 'error'): void {
    this.record('haptic', kind);
  }

  async requestNotifications(): Promise<'granted' | 'denied'> {
    this.record('requestNotifications');
    return 'granted';
  }

  async requestLocalNetwork(): Promise<'granted' | 'denied' | 'unknown'> {
    this.record('requestLocalNetwork');
    return 'granted';
  }

  deviceInfo(): FakeDeviceInfo {
    this.record('deviceInfo');
    return { ...this.info };
  }
}

export function createFakePlatform(): FakePlatform {
  return new FakePlatform();
}
