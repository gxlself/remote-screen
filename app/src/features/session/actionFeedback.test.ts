import { sessionToolbar } from '../../i18n/zh/sessionToolbar';
import type { CoreEventOf } from '../../native/rscore/types';
import {
  noteSessionAction,
  resetActionFeedback,
  takeActionFailure,
} from './actionFeedback';

function errorEvent(
  overrides: Partial<CoreEventOf<'session.error'>> = {},
): CoreEventOf<'session.error'> {
  return {
    type: 'session.error',
    sessionId: 's1',
    hostId: 'h1',
    ...overrides,
  };
}

beforeEach(() => {
  resetActionFeedback();
});

it('returns the policy message once for code 101', () => {
  noteSessionAction('s1', 'ctrlAltDel', 1000);
  expect(takeActionFailure(errorEvent({ code: 101 }), 2000)).toBe(
    sessionToolbar.ctrlAltDelDenied,
  );
  expect(takeActionFailure(errorEvent({ code: 101 }), 2001)).toBeNull();
});

it.each([{ code: 199 }, {}])(
  'returns the generic failure message for %p',
  overrides => {
    noteSessionAction('s1', 'ctrlAltDel', 1000);
    expect(takeActionFailure(errorEvent(overrides), 2000)).toBe(
      sessionToolbar.ctrlAltDelFailed,
    );
  },
);

it('ignores a record older than five seconds', () => {
  noteSessionAction('s1', 'ctrlAltDel', 1000);
  expect(takeActionFailure(errorEvent({ code: 101 }), 6001)).toBeNull();
  expect(takeActionFailure(errorEvent({ code: 101 }), 2000)).toBeNull();
});

it('accepts an error exactly at the end of the window', () => {
  noteSessionAction('s1', 'ctrlAltDel', 1000);
  expect(takeActionFailure(errorEvent({ code: 101 }), 6000)).toBe(
    sessionToolbar.ctrlAltDelDenied,
  );
});

it('only consumes the matching session record', () => {
  noteSessionAction('s1', 'ctrlAltDel', 1000);
  expect(
    takeActionFailure(errorEvent({ sessionId: 's2', code: 101 }), 2000),
  ).toBeNull();
  expect(takeActionFailure(errorEvent({ code: 101 }), 2000)).toBe(
    sessionToolbar.ctrlAltDelDenied,
  );
});

it('does not consume the record for an error with a reason', () => {
  noteSessionAction('s1', 'ctrlAltDel', 1000);
  expect(
    takeActionFailure(
      errorEvent({ code: 140, reason: 'userClosed' }),
      2000,
    ),
  ).toBeNull();
  expect(takeActionFailure(errorEvent({ code: 101 }), 2000)).toBe(
    sessionToolbar.ctrlAltDelDenied,
  );
});

it('does not consume the record for code 141', () => {
  noteSessionAction('s1', 'ctrlAltDel', 1000);
  expect(takeActionFailure(errorEvent({ code: 141 }), 2000)).toBeNull();
  expect(takeActionFailure(errorEvent({ code: 101 }), 2000)).toBe(
    sessionToolbar.ctrlAltDelDenied,
  );
});
