import { useEffect, useRef } from 'react';
import { useCoreEvent } from '../../features/events/EventsProvider';
import {
  ACTION_SUCCESS_DELAY_MS,
  isActionPending,
  onSessionAction,
  takeActionFailure,
} from '../../features/session/actionFeedback';
import { sessionToolbar } from '../../i18n/zh/sessionToolbar';
import type { SlotProps } from './slots';

/**
 * 会话动作（Ctrl+Alt+Del）的结果提示：失败事件来了就报失败；
 * 发出后一小段时间没有失败，才说「已发送」。工具栏、「更多」和软键盘都走这里。
 */
export function ActionFeedback({ ctx }: SlotProps): null {
  const sessionId = ctx.sessionId;
  const showToast = useRef(ctx.showToast);
  showToast.current = ctx.showToast;

  useEffect(() => {
    const timers = new Set<ReturnType<typeof setTimeout>>();
    const unsubscribe = onSessionAction((id, record) => {
      if (id !== sessionId) {
        return;
      }
      const timer = setTimeout(() => {
        timers.delete(timer);
        if (isActionPending(id, record)) {
          showToast.current(sessionToolbar.ctrlAltDelToast);
        }
      }, ACTION_SUCCESS_DELAY_MS);
      timers.add(timer);
    });
    return () => {
      unsubscribe();
      for (const timer of timers) {
        clearTimeout(timer);
      }
    };
  }, [sessionId]);

  useCoreEvent('session.error', event => {
    if (event.sessionId !== ctx.sessionId) {
      return;
    }
    const message = takeActionFailure(event);
    if (message !== null) {
      ctx.showToast(message, { tone: 'bad' });
    }
  });

  return null;
}

export default ActionFeedback;
