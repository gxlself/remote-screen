import { createContext, useContext } from 'react';

/** 会话页顶部的第一行：左上角 HUD 胶囊，高 32，离顶 12。 */
export const TOP_EDGE = 12;
const HUD_ROW_HEIGHT = 32;
export const TOP_GAP = 8;

/** 顶部提示栏（路径说明、本机使用条、共享提示）从 HUD 那一行下面开始排。 */
export const TOP_STACK_TOP = TOP_EDGE + HUD_ROW_HEIGHT + TOP_GAP;

/**
 * Toast 和 HUD 提示放在提示栏下面，不和它叠在一起（R37）。
 * stackHeight 是提示栏当前的实际高度，没有提示时为 0。
 */
export function belowTopStack(stackHeight: number): number {
  return stackHeight > 0
    ? TOP_STACK_TOP + stackHeight + TOP_GAP
    : TOP_STACK_TOP;
}

/** 提示栏当前高度，由会话页量出来往下传。 */
export const TopStackContext = createContext(0);

export function useBelowTopStack(): number {
  return belowTopStack(useContext(TopStackContext));
}
