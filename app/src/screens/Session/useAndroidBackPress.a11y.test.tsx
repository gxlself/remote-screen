import { act, cleanup, renderHook } from '@testing-library/react-native';
import { BackHandler, Platform } from 'react-native';
import type { SessionId } from '../../native/rscore/types';
import type { SessionController } from './useSessionController';
import { useAndroidBackPress } from './useAndroidBackPress';

function context(): SessionController {
  return {
    sessionId: 's1' as SessionId,
    toolbarHidden: false,
    sheet: null,
    keyboardOpen: false,
    coachVisible: false,
    view: { overlay: 'none' },
    onGesture: jest.fn(),
    showToast: jest.fn(),
    collapse: jest.fn(),
    closeSheet: jest.fn(),
    closeKeyboard: jest.fn(),
    closeCoach: jest.fn(),
  } as unknown as SessionController;
}

function captureBackPress() {
  let handler: (() => boolean | undefined) | null = null;
  const remove = jest.fn();
  const addEventListener = jest
    .spyOn(BackHandler, 'addEventListener')
    .mockImplementation((_event, listener) => {
      handler = listener as () => boolean | undefined;
      return { remove };
    });
  return {
    addEventListener,
    remove,
    press() {
      if (handler === null) {
        throw new Error('back handler was not registered');
      }
      return handler();
    },
  };
}

beforeEach(() => {
  jest.replaceProperty(Platform, 'OS', 'android');
  jest.spyOn(Date, 'now').mockReturnValue(1_000);
});

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
});

it('keeps the toolbar and still collapses on the second back press for screen readers', async () => {
  const ctx = context();
  const back = captureBackPress();
  await act(async () => {
    renderHook(() => useAndroidBackPress(ctx));
  });

  expect(back.press()).toBe(true);
  expect(ctx.onGesture).not.toHaveBeenCalled();
  expect(ctx.showToast).toHaveBeenCalledWith(
    '再按一次返回就收起会话，连接保持',
  );

  expect(back.press()).toBe(true);
  expect(ctx.collapse).toHaveBeenCalledTimes(1);
});
