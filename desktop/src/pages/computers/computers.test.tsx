import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { FAKE_SETTINGS } from '../../core/fake';
import {
  LIVING_ID,
  LIVING_MAC,
  OFFICE_WIN,
  OLD_MAC,
  advance,
  commandArgs,
  listRow,
  listRows,
  pairedHost,
  renderPages,
  rscoreArgs,
} from '../testing';

import { text } from './text';

function buttonNames(li: HTMLElement): string[] {
  return within(li)
    .getAllByRole('button')
    .map((b) => b.textContent ?? '');
}

afterEach(() => {
  vi.useRealTimers();
});

describe('ComputersPage', () => {
  it('list_paired_hosts', async () => {
    const user = userEvent.setup();
    const { t } = renderPages({
      paired: [
        OLD_MAC,
        pairedHost({ hostId: 'x5n8q2m7k4v9tb3d', name: '书房电脑' }),
        LIVING_MAC,
        OFFICE_WIN,
      ],
    });

    expect(await screen.findByText('客厅 Mac')).toBeInTheDocument();
    const names = ['书房电脑', '办公 Windows', '客厅 Mac', '旧 Mac'].sort((a, b) =>
      a.localeCompare(b),
    );
    expect(listRows().map((li) => names.find((n) => within(li).queryByText(n) !== null))).toEqual(
      names,
    );
    for (const li of listRows()) {
      expect(buttonNames(li)).toEqual([text.connect, text.viewOnly]);
      expect(within(li).getByText(text.paired)).toBeInTheDocument();
      expect(li.querySelector('[data-kind]')).toHaveAttribute('data-kind', 'computer');
    }
    expect(screen.queryByRole('alert')).toBeNull();
    expect(document.body.textContent).not.toMatch(/hub/i);
    expect(screen.getByRole('button', { name: text.addByAddress })).toBeInTheDocument();
    expect(rscoreArgs(t, 'hubDirectory')).toEqual([]);

    await user.click(within(listRow('书房电脑')).getByRole('button', { name: text.connect }));
    await waitFor(() => expect(commandArgs(t, 'desktop_open_session')).toHaveLength(1));
    expect(commandArgs(t, 'desktop_open_session')[0]).toMatchObject({
      req: { hostId: 'x5n8q2m7k4v9tb3d', hostName: '书房电脑', hostOs: 'unknown', control: true },
    });
  });

  it('list_empty', async () => {
    renderPages({ paired: [] });

    expect(await screen.findByText(text.empty)).toBeInTheDocument();
    expect(screen.queryByRole('list', { name: text.listLabel })).toBeNull();
  });

  it('connect_opens_session', async () => {
    const user = userEvent.setup();
    const { t } = renderPages({
      commands: { desktop_settings_get: () => ({ ...FAKE_SETTINGS, defaultQuality: 'clear' }) },
    });
    await screen.findByText('客厅 Mac');

    await user.click(within(listRow('客厅 Mac')).getByRole('button', { name: text.connect }));
    await waitFor(() => expect(commandArgs(t, 'desktop_open_session')).toHaveLength(1));
    await user.click(within(listRow('客厅 Mac')).getByRole('button', { name: text.viewOnly }));
    await waitFor(() => expect(commandArgs(t, 'desktop_open_session')).toHaveLength(2));

    const base = { hostId: LIVING_ID, hostName: '客厅 Mac', hostOs: 'macos', quality: 'clear' };
    expect(commandArgs(t, 'desktop_open_session')).toEqual([
      { req: { ...base, control: true } },
      { req: { ...base, control: false } },
    ]);
  });

  it('open_error_banner', async () => {
    const user = userEvent.setup();
    let answer: unknown = { tag: 'Unsupported', inner: { message: 'no hardware decoder' } };
    renderPages({
      commands: {
        desktop_open_session: () => {
          throw answer;
        },
      },
    });
    await screen.findByText('客厅 Mac');

    await user.click(within(listRow('客厅 Mac')).getByRole('button', { name: text.connect }));
    expect(await screen.findByText(text.noDecoder)).toBeInTheDocument();
    expect(screen.getByRole('alert')).toHaveTextContent(text.noDecoder);

    answer = { tag: 'Network', inner: { message: 'no path to 客厅 Mac' } };
    await user.click(within(listRow('客厅 Mac')).getByRole('button', { name: text.connect }));
    expect(await screen.findByText('no path to 客厅 Mac')).toBeInTheDocument();
    expect(screen.queryByText(text.noDecoder)).toBeNull();
  });

  it('paired_error_keeps_list', async () => {
    let calls = 0;
    renderPages({
      rscore: {
        hostsPaired: () => {
          calls += 1;
          if (calls === 1) {
            return JSON.stringify([LIVING_MAC, OFFICE_WIN, OLD_MAC]);
          }
          throw { tag: 'Internal', inner: { message: 'store locked' } };
        },
      },
    });
    await screen.findByText('客厅 Mac');

    fireEvent.focus(window);

    await waitFor(() => expect(calls).toBe(2));
    expect(listRows()).toHaveLength(3);
    expect(
      within(listRow('客厅 Mac')).getByRole('button', { name: text.connect }),
    ).toBeInTheDocument();
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('refresh_on_focus', async () => {
    const { t } = renderPages();
    await screen.findByText('客厅 Mac');
    const before = rscoreArgs(t, 'hostsPaired').length;
    expect(before).toBe(1);

    fireEvent.focus(window);

    await waitFor(() => expect(rscoreArgs(t, 'hostsPaired')).toHaveLength(before + 1));
  });

  it('refresh_every_30_s', async () => {
    vi.useFakeTimers();
    const { t } = renderPages();
    await advance();
    expect(rscoreArgs(t, 'hostsPaired')).toHaveLength(1);

    await advance(29_999);
    expect(rscoreArgs(t, 'hostsPaired')).toHaveLength(1);
    await advance(1);
    expect(rscoreArgs(t, 'hostsPaired')).toHaveLength(2);
  });

  it('refresh_after_pairing_and_session_end', async () => {
    const { t, emit } = renderPages();
    await screen.findByText('客厅 Mac');
    expect(rscoreArgs(t, 'hostsPaired')).toHaveLength(1);

    act(() => emit({ type: 'pairing.done', outcome: { status: 'denied' } }));
    act(() =>
      emit({
        type: 'pairing.done',
        outcome: { status: 'ok', hostId: OFFICE_WIN.hostId, hostName: OFFICE_WIN.name },
      }),
    );
    await waitFor(() => expect(rscoreArgs(t, 'hostsPaired')).toHaveLength(2));

    act(() => t.emitSession({ type: 'ended', sessionId: '3', hostId: LIVING_ID, byUser: true }));
    await waitFor(() => expect(rscoreArgs(t, 'hostsPaired')).toHaveLength(3));
  });
});
