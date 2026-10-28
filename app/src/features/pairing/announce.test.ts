import type { FailureModel } from './outcome';
import { pairStatusText } from './announce';

const failure: FailureModel = {
  icon: 'alert',
  tone: 'bad',
  title: '配对失败',
  message: '请检查电脑上的提示',
  primary: { action: 'close', label: '关闭' },
};

describe('pairStatusText', () => {
  it('announces the local approval step', () => {
    expect(
      pairStatusText({
        failure: null,
        hostWord: 'Mac',
        phase: 'progress',
        step: 'waitingLocal',
      }),
    ).toBe('等待你在 Mac 上点「允许」，确认窗口已经弹在 Mac 屏幕正中间');
  });

  it('announces failure title and detail', () => {
    expect(
      pairStatusText({
        failure,
        hostWord: 'Mac',
        phase: 'failed',
        step: 'waitingLocal',
      }),
    ).toBe('配对失败，请检查电脑上的提示');
  });

  it.each([
    ['dialing', 'progress'],
    ['verifying', 'progress'],
    [null, 'success'],
  ] as const)('does not announce %s in %s', (step, phase) => {
    expect(
      pairStatusText({
        failure: null,
        hostWord: 'Mac',
        phase,
        step,
      }),
    ).toBeNull();
  });
});
