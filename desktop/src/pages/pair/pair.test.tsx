import { fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent, { type UserEvent } from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { PairFileSummary, PairOutcome } from '@app/native/rscore/types';
import bannerStyles from '@host-ui/components/Banner.module.css';

import type { FakeTransport } from '../../core/fake';
import { text as listText } from '../computers/text';
import { OFFICE_ID, advance, renderPages, rscoreArgs } from '../testing';

import { DONE_DELAY_MS } from './PairPage';
import { text } from './text';

async function openPair(user: UserEvent): Promise<void> {
  await user.click(await screen.findByRole('button', { name: listText.addByAddress }));
  await screen.findByRole('heading', { name: text.titleByAddress });
}

/** Address 192.168.31.5 with `code`. */
async function fillAddress(user: UserEvent, code: string): Promise<void> {
  await user.type(screen.getByLabelText(text.addressLabel), '192.168.31.5');
  await user.type(screen.getByLabelText(text.codeLabel), code);
}

function startJson(t: FakeTransport): unknown[] {
  return rscoreArgs(t, 'pairingStart').map(([json]) => JSON.parse(String(json)) as unknown);
}

function toneClass(tone: 'good' | 'bad' | 'info'): string {
  return bannerStyles[tone] ?? tone;
}

const OUTCOMES: Array<[string, PairOutcome, string]> = [
  [
    'ok',
    { status: 'ok', hostId: OFFICE_ID, hostName: '办公 Windows' },
    text.outcome.ok('办公 Windows'),
  ],
  ['badCode', { status: 'badCode', attemptsLeft: 2 }, text.outcome.badCode(2)],
  ['locked', { status: 'locked', retryAfterS: 60 }, text.outcome.locked(60)],
  ['locked without retryAfterS', { status: 'locked' }, text.outcome.locked()],
  ['denied', { status: 'denied' }, text.outcome.denied],
  ['expired', { status: 'expired' }, text.outcome.expired],
  ['disabledOnPath', { status: 'disabledOnPath' }, text.outcome.disabledOnPath],
  ['methodDisabled', { status: 'methodDisabled' }, text.outcome.methodDisabled],
  ['mitm', { status: 'mitm' }, text.outcome.mitm],
  ['cancelled', { status: 'cancelled' }, text.outcome.cancelled],
  ['timeout', { status: 'timeout' }, text.outcome.timeout],
  [
    'error',
    { status: 'error', code: 5, message: 'tls handshake failed' },
    text.outcome.error('tls handshake failed'),
  ],
];

afterEach(() => {
  vi.useRealTimers();
});

describe('PairPage', () => {
  it('pair_progress_and_done', async () => {
    vi.useFakeTimers();
    let finish: (json: string) => void = () => undefined;
    const { t, emit } = renderPages({
      rscore: {
        pairingStart: () =>
          new Promise<string>((resolve) => {
            finish = resolve;
          }),
      },
    });
    await advance();

    fireEvent.click(screen.getByRole('button', { name: listText.addByAddress }));
    await advance();
    expect(screen.getByRole('heading', { name: text.titleByAddress })).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText(text.addressLabel), {
      target: { value: '192.168.31.5' },
    });
    const input = screen.getByLabelText(text.codeLabel);
    const start = screen.getByRole('button', { name: text.start });
    fireEvent.change(input, { target: { value: '12345' } });
    expect(start).toBeDisabled();
    fireEvent.change(input, { target: { value: '12 34 56' } });
    expect(input).toHaveValue('123456');
    expect(start).toBeEnabled();

    fireEvent.click(start);
    await advance();
    expect(startJson(t)).toEqual([{ addr: '192.168.31.5', code: '123456' }]);
    expect(screen.getByText(text.progress.dialing)).toBeInTheDocument();

    emit({ type: 'pairing.progress', step: 'waitingLocal', hostName: '办公 Windows' });
    await advance();
    expect(screen.getByText(text.progress.waitingLocal)).toBeInTheDocument();

    finish(JSON.stringify({ status: 'ok', hostId: OFFICE_ID, hostName: '办公 Windows' }));
    await advance();
    expect(screen.getByText(text.outcome.ok('办公 Windows'))).toBeInTheDocument();
    expect(screen.queryByText(text.progress.waitingLocal)).toBeNull();

    await advance(DONE_DELAY_MS - 1);
    expect(screen.queryByRole('heading', { name: listText.title })).toBeNull();
    await advance(1);
    expect(screen.getByRole('heading', { name: listText.title })).toBeInTheDocument();
  });

  it.each(OUTCOMES)('pair_outcomes_text: %s', async (_name, outcome, expected) => {
    const user = userEvent.setup();
    renderPages({ pairOutcome: outcome });
    await openPair(user);

    await fillAddress(user, '123456');
    await user.click(screen.getByRole('button', { name: text.start }));

    const title = await screen.findByText(expected);
    const banner = title.closest('[role]');
    const tone = outcome.status === 'ok' ? 'good' : outcome.status === 'mitm' ? 'bad' : 'info';
    expect(banner).toHaveClass(toneClass(tone));
    if (outcome.status === 'mitm') {
      expect(banner).toHaveAttribute('role', 'alert');
    } else {
      expect(banner).not.toHaveClass(toneClass('bad'));
    }
  });

  it('pair_file_flow', async () => {
    const user = userEvent.setup();
    const { t } = renderPages({ pickFile: '/Users/u/x.rspair' });
    await openPair(user);

    await user.click(screen.getByRole('radio', { name: text.methods.file }));
    const start = screen.getByRole('button', { name: text.start });
    expect(start).toBeDisabled();
    await user.click(screen.getByRole('button', { name: text.pickFile }));

    expect(await screen.findByText('办公 Windows', { selector: 'dd' })).toBeInTheDocument();
    expect(rscoreArgs(t, 'pairingLoadFile')).toEqual([['{"uri":"file:///Users/u/x.rspair"}']]);
    expect(start).toBeEnabled();

    await user.click(start);
    await waitFor(() => expect(startJson(t)).toEqual([{ fileId: 'AAAAAAAAAAAAAAAAAAAAAA' }]));
  });

  it('pair_file_expired', async () => {
    const user = userEvent.setup();
    const summary: PairFileSummary = {
      fileId: 'BBBBBBBBBBBBBBBBBBBBBB',
      hostId: OFFICE_ID,
      hostName: '办公 Windows',
      fp: '9o5xYm-IeATETQkkzOt1uxYegCsEjASBlpyGgnaYfks',
      addrs: ['100.64.0.22:7400'],
      expiresAt: 0,
      expired: true,
    };
    renderPages({
      pickFile: '/Users/u/old.rspair',
      rscore: { pairingLoadFile: () => JSON.stringify(summary) },
    });
    await openPair(user);

    await user.click(screen.getByRole('radio', { name: text.methods.file }));
    await user.click(screen.getByRole('button', { name: text.pickFile }));

    expect(await screen.findByText(text.fileExpired)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: text.start })).toBeDisabled();
  });

  it('cancel_on_leave', async () => {
    const user = userEvent.setup();
    const { t } = renderPages({ rscore: { pairingStart: () => new Promise(() => undefined) } });

    await openPair(user);
    await user.click(screen.getByRole('button', { name: text.back }));
    expect(await screen.findByRole('heading', { name: listText.title })).toBeInTheDocument();
    expect(rscoreArgs(t, 'pairingCancel')).toEqual([]);

    await openPair(user);
    await fillAddress(user, '123456');
    await user.click(screen.getByRole('button', { name: text.start }));
    expect(await screen.findByText(text.progress.dialing)).toBeInTheDocument();
    expect(rscoreArgs(t, 'pairingCancel')).toEqual([]);

    await user.click(screen.getByRole('button', { name: text.back }));

    expect(rscoreArgs(t, 'pairingCancel')).toEqual([[]]);
    expect(await screen.findByRole('heading', { name: listText.title })).toBeInTheDocument();
    expect(rscoreArgs(t, 'pairingCancel')).toEqual([[]]);
  });

  it('cancel_button_cancels', async () => {
    const user = userEvent.setup();
    const { t } = renderPages({ rscore: { pairingStart: () => new Promise(() => undefined) } });
    await openPair(user);

    await fillAddress(user, '123456');
    await user.click(screen.getByRole('button', { name: text.start }));
    await user.click(await screen.findByRole('button', { name: text.cancel }));

    expect(rscoreArgs(t, 'pairingCancel')).toEqual([[]]);
  });

  it('pair_by_address', async () => {
    const user = userEvent.setup();
    const { t } = renderPages();
    await user.click(await screen.findByRole('button', { name: listText.addByAddress }));

    expect(screen.getByRole('heading', { name: text.titleByAddress })).toBeInTheDocument();
    expect(screen.getByRole('radio', { name: text.methods.address })).toHaveAttribute(
      'aria-checked',
      'true',
    );
    expect(screen.getByRole('radio', { name: text.methods.file })).toBeInTheDocument();
    expect(screen.getAllByRole('radio')).toHaveLength(2);

    const start = screen.getByRole('button', { name: text.start });
    const port = screen.getByLabelText(text.portLabel);
    await user.type(screen.getByLabelText(text.codeLabel), '123456');
    expect(start).toBeDisabled();
    await user.type(screen.getByLabelText(text.addressLabel), '192.168.31.5');
    expect(start).toBeEnabled();

    await user.type(port, '70000');
    expect(start).toBeDisabled();
    await user.clear(port);
    await user.type(port, '0');
    expect(start).toBeDisabled();
    await user.clear(port);
    expect(start).toBeEnabled();

    await user.click(start);
    await waitFor(() => expect(rscoreArgs(t, 'pairingStart')).toHaveLength(1));
    expect(rscoreArgs(t, 'pairingStart')).toEqual([['{"addr":"192.168.31.5","code":"123456"}']]);
  });

  it('pair_by_address_with_port', async () => {
    const user = userEvent.setup();
    const { t } = renderPages({ pairOutcome: { status: 'timeout' } });
    await user.click(await screen.findByRole('button', { name: listText.addByAddress }));

    await user.type(screen.getByLabelText(text.addressLabel), ' 192.168.31.5 ');
    await user.type(screen.getByLabelText(text.portLabel), '7401');
    await user.type(screen.getByLabelText(text.codeLabel), '654321');
    await user.click(screen.getByRole('button', { name: text.start }));

    expect(await screen.findByText(text.outcome.timeout)).toBeInTheDocument();
    expect(rscoreArgs(t, 'pairingStart')).toEqual([
      ['{"addr":"192.168.31.5","port":7401,"code":"654321"}'],
    ]);
  });
});
