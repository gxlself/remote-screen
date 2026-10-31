import type { DevicePlatform } from '../rscore/types';

export type AuthResult = 'ok' | 'cancelled' | 'unavailable' | 'failed';

export type AuthCapability = {
  /** iOS：faceId / touchId / opticId；Android（BiometricPrompt）：fingerprint / face / iris / biometric（多种并存）。 */
  biometry:
    | 'faceId'
    | 'touchId'
    | 'opticId'
    | 'fingerprint'
    | 'face'
    | 'iris'
    | 'biometric'
    | 'none';
  passcode: boolean;
};

export interface PlatformServices {
  /** P9：LAContext / BiometricPrompt。 */
  authenticate(reason: string): Promise<AuthResult>;
  /** P9：同步；biometry 只在已录入且可用时非 none。 */
  authCapability(): AuthCapability;
  copyText(text: string): Promise<void>;
  /** 07 未列，P12 实现；读手机剪贴板可能触发 iOS 粘贴授权。 */
  readText(): Promise<string | null>;
  /** 手机剪贴板变了几次（iOS changeCount，不读内容、不触发授权）；拿不到时 null（Android）。 */
  clipboardChangeCount(): number | null;
  /** 打开外部 App / 商店链接，返回是否成功。 */
  openUrl(url: string): Promise<boolean>;
  canOpenUrl(url: string): Promise<boolean>;
  /** P12：系统分享面板（日志 zip、公钥）。 */
  share(opts: { url?: string; text?: string; title?: string }): Promise<void>;
  /** P12：触感反馈。 */
  haptic(kind: 'light' | 'success' | 'warning' | 'error'): void;
  requestNotifications(): Promise<'granted' | 'denied'>;
  requestLocalNetwork(): Promise<'granted' | 'denied' | 'unknown'>;
  deviceInfo(): {
    platform: DevicePlatform;
    isPad: boolean;
    model: string;
    osVersion: string;
    appVersion: string;
  };
}
