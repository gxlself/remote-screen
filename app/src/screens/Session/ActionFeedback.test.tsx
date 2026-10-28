import { act } from '@testing-library/react-native';
import { sessionToolbar } from '../../i18n/zh/sessionToolbar';
import {
  noteSessionAction,
  resetActionFeedback,
} from '../../features/session/actionFeedback';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { renderWithProviders } from '../../test/render';
import { ActionFeedback } from './ActionFeedback';
import type { SessionCtx } from './slots';

function context(sessionId: string, showToast: jest.Mock): SessionCtx {
  return { sessionId, showToast } as unknown as SessionCtx;
}

beforeEach(() => {
  resetActionFeedback();
});

it('shows the reason when a noted action fails with code 101', async () => {
  const core = createFakeCore();
  const showToast = jest.fn();
  await renderWithProviders(
    <ActionFeedback ctx={context('s1', showToast)} />,
    { core },
  );
  noteSessionAction('s1', 'ctrlAltDel');
  await act(() =>
    core.emit({
      type: 'session.error',
      sessionId: 's1',
      hostId: 'h1',
      code: 101,
    }),
  );
  expect(showToast).toHaveBeenCalledWith(sessionToolbar.ctrlAltDelDenied, {
    tone: 'bad',
  });
});

it('ignores an error when no action was sent', async () => {
  const core = createFakeCore();
  const showToast = jest.fn();
  await renderWithProviders(
    <ActionFeedback ctx={context('s1', showToast)} />,
    { core },
  );
  await act(() =>
    core.emit({
      type: 'session.error',
      sessionId: 's1',
      hostId: 'h1',
      code: 101,
    }),
  );
  expect(showToast).not.toHaveBeenCalled();
});

it('ignores an error for a different session', async () => {
  const core = createFakeCore();
  const showToast = jest.fn();
  await renderWithProviders(
    <ActionFeedback ctx={context('s1', showToast)} />,
    { core },
  );
  noteSessionAction('s2', 'ctrlAltDel');
  await act(() =>
    core.emit({
      type: 'session.error',
      sessionId: 's2',
      hostId: 'h1',
      code: 101,
    }),
  );
  expect(showToast).not.toHaveBeenCalled();
});
