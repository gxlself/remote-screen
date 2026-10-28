import { sessionToolbar } from '../../i18n/zh/sessionToolbar';
import type { RsCoreApi } from '../../native/rscore/RsCoreApi';
import type { CoreEventOf, SessionId } from '../../native/rscore/types';

export type FeedbackAction = 'ctrlAltDel';
export const ACTION_FEEDBACK_WINDOW_MS = 5000;

/** 发出后等这么久没收到失败，才提示「已发送」。 */
export const ACTION_SUCCESS_DELAY_MS = 1500;

type PendingAction = { action: FeedbackAction; atMs: number };
type ActionListener = (sessionId: SessionId, record: PendingAction) => void;

const pending = new Map<SessionId, PendingAction>();
const listeners = new Set<ActionListener>();

/** 发出 sessions.action 时调用。同一会话再按一次，覆盖上一次的记录。 */
export function noteSessionAction(
  sessionId: SessionId,
  action: FeedbackAction,
  atMs: number = Date.now(),
): void {
  const record = { action, atMs };
  pending.set(sessionId, record);
  for (const listener of listeners) {
    listener(sessionId, record);
  }
}

/** 订阅新发出的动作；返回取消订阅。会话页用它在没有失败时补「已发送」。 */
export function onSessionAction(listener: ActionListener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** 这条记录还没被失败消耗掉（也没被新的一次覆盖）。 */
export function isActionPending(
  sessionId: SessionId,
  record: PendingAction,
): boolean {
  return pending.get(sessionId) === record;
}

/**
 * 工具栏、「更多」和软键盘共用的发送入口：先记下再发，发送本身抛错时撤掉
 * 记录并返回 false，由调用方立刻提示失败。成功与否的提示交给 ActionFeedback。
 */
export function sendFeedbackAction(
  core: Pick<RsCoreApi, 'sessions'>,
  sessionId: SessionId,
  action: FeedbackAction,
): boolean {
  noteSessionAction(sessionId, action);
  try {
    core.sessions.action(sessionId, action);
  } catch {
    pending.delete(sessionId);
    return false;
  }
  return true;
}

/**
 * 收到 session.error 时调用，返回要弹的文案，不弹就返回 null。
 * - 带 reason（会话结束）：返回 null，不消耗记录；
 * - code === 141：交给 SharedOverlay 处理，返回 null，不消耗记录；
 * - 这个会话没有记录，或者记录早于 nowMs - 5000：删掉记录，返回 null；
 * - 其余情况：删掉记录，code === 101 返回策略文案，其他 code（包括没有 code）返回通用失败文案。
 */
export function takeActionFailure(
  event: CoreEventOf<'session.error'>,
  nowMs: number = Date.now(),
): string | null {
  if (event.reason !== undefined || event.code === 141) {
    return null;
  }
  const record = pending.get(event.sessionId);
  if (record === undefined) {
    return null;
  }
  if (nowMs - record.atMs > ACTION_FEEDBACK_WINDOW_MS) {
    pending.delete(event.sessionId);
    return null;
  }
  pending.delete(event.sessionId);
  return event.code === 101
    ? sessionToolbar.ctrlAltDelDenied
    : sessionToolbar.ctrlAltDelFailed;
}

/** 只给测试用。 */
export function resetActionFeedback(): void {
  pending.clear();
}
