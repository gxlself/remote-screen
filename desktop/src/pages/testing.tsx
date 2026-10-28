// Test helpers for the pages: preset paired computers and a rendered <Pages /> over the fake shell.
// Addresses stay in 100.64.0.x / 192.168.31.x.
import { act, render, screen, within } from '@testing-library/react';
import { vi } from 'vitest';

import { createRsCore } from '@app/native/rscore/createRsCore';
import type { CoreEvent, PairedHost } from '@app/native/rscore/types';

import { desktopBindings } from '../core/bindings';
import { CoreProvider } from '../core/CoreContext';
import { desktopApi } from '../core/desktopApi';
import { fakeTransport, type FakeOptions, type FakeTransport } from '../core/fake';
import type { Transport } from '../core/tauri';

import { text as listText } from './computers/text';

import { Pages } from '.';

export const LIVING_ID = 'l4v9m2q7k3x8tb1a';
export const OFFICE_ID = 'o2k7p9x4m3q8tb5c';
export const OLD_ID = 'd8m3q2x9k4v7tb6e';

export function pairedHost(
  fields: Partial<PairedHost> & Pick<PairedHost, 'hostId' | 'name'>,
): PairedHost {
  return {
    fp: '9o5xYm-IeATETQkkzOt1uxYegCsEjASBlpyGgnaYfks',
    addresses: [],
    pairedAt: 0,
    os: null,
    osVersion: null,
    hostVersion: null,
    features: null,
    displays: [],
    lastSeenAt: null,
    ...fields,
  };
}

export const LIVING_MAC = pairedHost({
  hostId: LIVING_ID,
  name: '客厅 Mac',
  os: 'macos',
  addresses: ['100.64.0.21:7400'],
});
export const OFFICE_WIN = pairedHost({
  hostId: OFFICE_ID,
  name: '办公 Windows',
  os: 'windows',
  addresses: ['100.64.0.22:7400'],
});
export const OLD_MAC = pairedHost({
  hostId: OLD_ID,
  name: '旧 Mac',
  os: 'macos',
  addresses: ['100.64.0.23:7400'],
});

export type ShellCommand = (args: Record<string, unknown>) => unknown;

export type PagesOptions = FakeOptions & {
  /** Answers for shell commands other than `rscore_call` (a throw becomes the rejection). */
  commands?: Record<string, ShellCommand>;
};

export type Harness = {
  t: FakeTransport;
  /** Delivers one RsCore event (consecutive seq). */
  emit: (event: CoreEvent) => void;
};

function withCommands(t: FakeTransport, commands: Record<string, ShellCommand>): Transport {
  return {
    async invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
      const command = commands[cmd];
      if (command === undefined) {
        return t.invoke<T>(cmd, args);
      }
      t.calls.push({ cmd, args });
      return (await command(args ?? {})) as T;
    },
    listen<T>(event: string, cb: (payload: T) => void) {
      return t.listen<T>(event, cb);
    },
    pickFile(title: string, extensions: string[]) {
      return t.pickFile(title, extensions);
    },
  };
}

/**
 * Renders <Pages /> over the fake shell. By default the three computers above are paired and a
 * pairing (by address or file) reaches 办公 Windows.
 */
export function renderPages(options: PagesOptions = {}): Harness {
  const { commands, ...fake } = options;
  const t = fakeTransport({
    paired: [LIVING_MAC, OFFICE_WIN, OLD_MAC],
    pairTarget: OFFICE_WIN,
    ...fake,
  });
  const shell = commands === undefined ? t : withCommands(t, commands);
  render(
    <CoreProvider core={createRsCore(desktopBindings(shell))} desktop={desktopApi(shell)}>
      <Pages />
    </CoreProvider>,
  );
  let seq = 0;
  return {
    t,
    emit(event) {
      seq += 1;
      t.emit(seq, event);
    },
  };
}

export function listRows(): HTMLElement[] {
  return within(screen.getByRole('list', { name: listText.listLabel })).getAllByRole('listitem');
}

/** The computer list row showing `name`. */
export function listRow(name: string): HTMLElement {
  const found = listRows().find((li) => within(li).queryByText(name) !== null);
  if (found === undefined) {
    throw new Error(`no row for ${name}`);
  }
  return found;
}

/** Argument lists of every `rscore_call` to `name`. */
export function rscoreArgs(t: FakeTransport, name: string): unknown[][] {
  return t.calls
    .filter((c) => c.cmd === 'rscore_call')
    .map((c) => c.args as { name: string; args: unknown[] })
    .filter((c) => c.name === name)
    .map((c) => c.args);
}

export function commandArgs(t: FakeTransport, cmd: string): unknown[] {
  return t.calls.filter((c) => c.cmd === cmd).map((c) => c.args);
}

/** Fake timers only: runs pending promise chains, advances the clock and renders the result. */
export async function advance(ms = 0): Promise<void> {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}
