import { act, fireEvent, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { Participant } from '@app/native/rscore/types';

import { LIVING_ID, OFFICE_ID, advance, renderPages, rscoreArgs } from '../testing';

import { MAX_ENDED } from './SessionNotices';
import { text } from './text';

const REQUESTER: Participant = {
  deviceId: 'k3jq7x2m9p4tbw8a',
  deviceName: 'iPhone 17',
  user: 'demo@',
  path: 'tailnetDirect',
  since: 0,
};

function notices(): HTMLElement[] {
  return screen.queryAllByRole('alert');
}

afterEach(() => {
  vi.useRealTimers();
});

describe('SessionNotices', () => {
  it('ended_banner', async () => {
    const { t, emit } = renderPages();
    await screen.findByText('客厅 Mac');

    act(() =>
      emit({ type: 'session.error', sessionId: '7', hostId: LIVING_ID, reason: 'hostShutdown' }),
    );
    expect(notices()).toHaveLength(0);
    act(() => t.emitSession({ type: 'ended', sessionId: '7', hostId: LIVING_ID, byUser: false }));

    expect(
      await screen.findByText(text.ended('客厅 Mac', text.reasons.hostShutdown)),
    ).toBeInTheDocument();
    expect(notices()).toHaveLength(1);

    act(() =>
      emit({ type: 'session.error', sessionId: '8', hostId: OFFICE_ID, reason: 'replaced' }),
    );
    act(() => t.emitSession({ type: 'ended', sessionId: '8', hostId: OFFICE_ID, byUser: true }));
    act(() => t.emitSession({ type: 'ended', sessionId: '9', hostId: OFFICE_ID, byUser: true }));

    expect(notices()).toHaveLength(1);
    expect(screen.queryByText(text.ended('办公 Windows', text.reasons.replaced))).toBeNull();
  });

  it('ended_reason_fallbacks', async () => {
    const { t, emit } = renderPages();
    await screen.findByText('客厅 Mac');

    act(() =>
      emit({
        type: 'session.error',
        sessionId: '4',
        hostId: OFFICE_ID,
        reason: 'userClosed',
        message: 'stream reset by peer',
      }),
    );
    act(() => t.emitSession({ type: 'ended', sessionId: '4', hostId: OFFICE_ID, byUser: false }));
    act(() =>
      t.emitSession({ type: 'ended', sessionId: '5', hostId: 'z9z9z9z9z9z9z9z9', byUser: false }),
    );

    expect(
      await screen.findByText(text.ended('办公 Windows', 'stream reset by peer')),
    ).toBeInTheDocument();
    expect(screen.getByText(text.ended(text.unknownHost, null))).toBeInTheDocument();
  });

  it('ended_banners_cap_and_close', async () => {
    const user = userEvent.setup();
    const { t, emit } = renderPages();
    await screen.findByText('客厅 Mac');

    const reasons = ['idleTimeout', 'revoked', 'hostUpdating', 'permissionLost'] as const;
    reasons.forEach((reason, i) => {
      act(() =>
        emit({ type: 'session.error', sessionId: String(i + 1), hostId: LIVING_ID, reason }),
      );
      act(() =>
        t.emitSession({
          type: 'ended',
          sessionId: String(i + 1),
          hostId: LIVING_ID,
          byUser: false,
        }),
      );
    });

    expect(notices()).toHaveLength(MAX_ENDED);
    expect(screen.queryByText(text.ended('客厅 Mac', text.reasons.idleTimeout))).toBeNull();
    const newest = screen.getByText(text.ended('客厅 Mac', text.reasons.permissionLost));
    expect(notices()[0]).toBe(newest.closest('[role="alert"]'));

    const notice = newest.closest('[role="alert"]')?.parentElement?.parentElement as HTMLElement;
    await user.click(within(notice).getByRole('button', { name: text.close }));

    expect(notices()).toHaveLength(MAX_ENDED - 1);
    expect(screen.queryByText(text.ended('客厅 Mac', text.reasons.permissionLost))).toBeNull();
  });

  it('control_request_dialog', async () => {
    vi.useFakeTimers();
    const { t, emit } = renderPages();
    await advance();

    act(() =>
      emit({ type: 'session.controlRequest', sessionId: '7', requester: REQUESTER, timeoutS: 10 }),
    );
    let dialog = screen.getByRole('alertdialog');
    expect(
      within(dialog).getByText(text.controlRequest('iPhone 17', 'demo@', 10)),
    ).toBeInTheDocument();

    fireEvent.click(within(dialog).getByRole('button', { name: text.allow }));
    expect(rscoreArgs(t, 'sessionsRespondControl')).toEqual([['7', REQUESTER.deviceId, true]]);
    expect(screen.queryByRole('alertdialog')).toBeNull();

    act(() =>
      emit({ type: 'session.controlRequest', sessionId: '7', requester: REQUESTER, timeoutS: 10 }),
    );
    fireEvent.click(
      within(screen.getByRole('alertdialog')).getByRole('button', { name: text.deny }),
    );
    expect(rscoreArgs(t, 'sessionsRespondControl')).toEqual([
      ['7', REQUESTER.deviceId, true],
      ['7', REQUESTER.deviceId, false],
    ]);

    act(() =>
      emit({ type: 'session.controlRequest', sessionId: '7', requester: REQUESTER, timeoutS: 10 }),
    );
    await advance(3000);
    dialog = screen.getByRole('alertdialog');
    expect(
      within(dialog).getByText(text.controlRequest('iPhone 17', 'demo@', 7)),
    ).toBeInTheDocument();

    await advance(6999);
    expect(screen.getByRole('alertdialog')).toBeInTheDocument();
    await advance(1);
    expect(screen.queryByRole('alertdialog')).toBeNull();
    expect(rscoreArgs(t, 'sessionsRespondControl')).toHaveLength(2);
  });
});
