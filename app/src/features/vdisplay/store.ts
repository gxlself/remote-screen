import { create } from 'zustand';

export type VdisplayPending = { requestId: number; deadlineMs: number };

/**
 * 每个会话的虚拟屏状态。放在 store 里而不是面板里：新建请求发出后用户关掉
 * 面板，结果回来时会话页还能接住并提示（R07）。
 */
export type VdisplayState = {
  owned: Record<string, number>;
  /** 刚建好、显示器列表里还没出现的虚拟屏，列表更新前不当作已消失。 */
  fresh: Record<string, number>;
  pending: Record<string, VdisplayPending>;
  errors: Record<string, string>;
  setOwned(sessionId: string, displayId: number): void;
  clearOwned(sessionId: string): void;
  setFresh(sessionId: string, displayId: number | null): void;
  setPending(sessionId: string, pending: VdisplayPending | null): void;
  setError(sessionId: string, text: string | null): void;
};

function without<T>(
  record: Record<string, T>,
  key: string,
): Record<string, T> | null {
  if (!(key in record)) {
    return null;
  }
  const next = { ...record };
  delete next[key];
  return next;
}

export const useVdisplayStore = create<VdisplayState>()(set => ({
  owned: {},
  fresh: {},
  pending: {},
  errors: {},
  setOwned(sessionId, displayId) {
    set(state => ({
      owned: { ...state.owned, [sessionId]: displayId },
    }));
  },
  clearOwned(sessionId) {
    set(state => {
      const owned = without(state.owned, sessionId);
      return owned === null ? state : { owned };
    });
  },
  setFresh(sessionId, displayId) {
    set(state => {
      if (displayId !== null) {
        return { fresh: { ...state.fresh, [sessionId]: displayId } };
      }
      const fresh = without(state.fresh, sessionId);
      return fresh === null ? state : { fresh };
    });
  },
  setPending(sessionId, pending) {
    set(state => {
      if (pending !== null) {
        return { pending: { ...state.pending, [sessionId]: pending } };
      }
      const next = without(state.pending, sessionId);
      return next === null ? state : { pending: next };
    });
  },
  setError(sessionId, text) {
    set(state => {
      if (text !== null) {
        return { errors: { ...state.errors, [sessionId]: text } };
      }
      const errors = without(state.errors, sessionId);
      return errors === null ? state : { errors };
    });
  },
}));

/** 清空全部会话的虚拟屏状态（测试和退出登录时用）。 */
export function resetVdisplayStore(): void {
  useVdisplayStore.setState({ owned: {}, fresh: {}, pending: {}, errors: {} });
}
