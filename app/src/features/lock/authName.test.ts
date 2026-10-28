import { act, cleanup, renderHook } from '@testing-library/react-native';
import { createElement } from 'react';
import type { ReactNode } from 'react';
import { AppState, Platform, type AppStateStatus } from 'react-native';
import { PlatformProvider } from '../../native/platform/PlatformContext';
import { FakePlatform } from '../../native/platform/fakePlatform';
import type { AuthCapability } from '../../native/platform/PlatformServices';
import {
  authIcon,
  authName,
  fallbackAuthName,
  useAuthCapability,
} from './authName';

let onChange: ((state: AppStateStatus) => void) | undefined;

beforeEach(() => {
  onChange = undefined;
  jest
    .spyOn(AppState, 'addEventListener')
    .mockImplementation((_event, listener) => {
      onChange = listener;
      return { remove: jest.fn() };
    });
});

afterEach(() => {
  cleanup();
  jest.restoreAllMocks();
});

function capability(
  biometry: AuthCapability['biometry'],
  passcode = true,
): AuthCapability {
  return { biometry, passcode };
}

describe('authName', () => {
  it('names each biometric capability', () => {
    expect(authName(capability('faceId'))).toBe('Face ID');
    expect(authName(capability('touchId'))).toBe('Touch ID');
    expect(authName(capability('opticId'))).toBe('Optic ID');
    expect(authName(capability('fingerprint'))).toBe('指纹');
    expect(authName(capability('face'))).toBe('人脸');
    expect(authName(capability('iris'))).toBe('虹膜');
    expect(authName(capability('biometric'))).toBe('生物识别');
  });

  it('uses platform-specific fallback names', () => {
    jest.replaceProperty(Platform, 'OS', 'ios');
    expect(fallbackAuthName()).toBe('Face ID');
    jest.replaceProperty(Platform, 'OS', 'android');
    expect(fallbackAuthName()).toBe('生物识别');
  });

  it('falls back to the device passcode or no method', () => {
    expect(authName(capability('none'))).toBe('设备密码');
    expect(authName(capability('none', false))).toBeNull();
  });
});

describe('authIcon', () => {
  it('uses the Face ID icon only for Face ID', () => {
    expect(authIcon(capability('faceId'))).toBe('faceId');
    expect(authIcon(capability('touchId'))).toBe('lock');
  });
});

describe('useAuthCapability', () => {
  it('refreshes when the app returns to the foreground', async () => {
    const platform = new FakePlatform();
    const rendered = await renderHook(() => useAuthCapability(), {
      wrapper: ({ children }: { children: ReactNode }) =>
        createElement(PlatformProvider, { platform, children }),
    });

    expect(rendered.result.current).toEqual({
      biometry: 'faceId',
      passcode: true,
    });

    platform.authCapabilityResult = {
      biometry: 'touchId',
      passcode: true,
    };
    await act(async () => {
      onChange?.('active');
    });

    expect(rendered.result.current).toEqual({
      biometry: 'touchId',
      passcode: true,
    });
    expect(
      platform.calls.filter(call => call.method === 'authCapability'),
    ).toHaveLength(2);
  });
});
