import { TOP_STACK_TOP, belowTopStack } from './topStack';

describe('top notice stack', () => {
  it('starts below the HUD row', () => {
    expect(TOP_STACK_TOP).toBe(12 + 32 + 8);
  });

  it('puts toasts and tips right under the stack, or under the HUD when empty', () => {
    expect(belowTopStack(0)).toBe(TOP_STACK_TOP);
    expect(belowTopStack(66)).toBe(TOP_STACK_TOP + 66 + 8);
  });
});
