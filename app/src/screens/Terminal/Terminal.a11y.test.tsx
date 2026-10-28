import {
  act,
  cleanup,
  fireEvent,
  screen,
  waitFor,
} from '@testing-library/react-native';
import type { QueryClient } from '@tanstack/react-query';
import {
  AccessibilityInfo,
  PixelRatio,
  Platform,
  StyleSheet,
} from 'react-native';
import { localSyncGet } from '../../features/data/localSync';
import { resetSshStore } from '../../features/sessions/sshStore';
import { MemoryKv } from '../../features/storage/kv';
import type { KvStore } from '../../features/storage/kv';
import type { TermStatus } from '../../features/terminal/tabs';
import { snippets } from '../../i18n/zh/snippets';
import { terminal } from '../../i18n/zh/terminal';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { sshServers } from '../../native/rscore/fixtures';
import type { TerminalPaneProps } from '../../native/terminal/TerminalPane';
import { resetTerminalKeyboard } from '../../native/terminal/terminalKeyboard';
import { failLocalSync, seedLocalSync } from '../../test/localDirectory';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import { hit } from '../../theme/tokens';
import { NewTerminalSheet } from './NewTerminalSheet';
import { SnippetBar } from './SnippetBar';
import { SnippetsEditor } from './SnippetsEditor';
import { StatusLine } from './StatusLine';
import type { TerminalSlotProps } from './TerminalSlot';
import TerminalScreen from './index';

jest.mock('./TerminalSlot', () => {
  const React = jest.requireActual<typeof import('react')>('react');
  const { View } =
    jest.requireActual<typeof import('react-native')>('react-native');
  return {
    TerminalSlot: (p: TerminalSlotProps) => {
      React.useImperativeHandle(
        p.ref,
        () => ({
          sendSequence: jest.fn(),
          copySelection: jest.fn(),
          paste: jest.fn(),
        }),
        [],
      );
      return (
        <View accessibilityLabel={p.accessibilityLabel}>{p.accessory}</View>
      );
    },
  };
});

jest.mock('../../native/terminal/TerminalPane', () => {
  const React = jest.requireActual<typeof import('react')>('react');
  const { View } =
    jest.requireActual<typeof import('react-native')>('react-native');
  return {
    TerminalPane: (p: TerminalPaneProps) => {
      React.useImperativeHandle(
        p.ref,
        () => ({
          sendSequence: jest.fn(),
          copySelection: jest.fn(),
          paste: jest.fn(),
        }),
        [],
      );
      return <View testID="terminal-pane">{p.accessory}</View>;
    },
  };
});

const prod = sshServers.find(server => server.name === 'prod-api-01')!;
const items = [
  { id: 'docker', label: '', cmd: 'docker ps', run: true },
  { id: 'tail', label: '', cmd: 'tail -f api.log', run: false },
];
const connecting: TermStatus = {
  phase: 'connecting',
  tone: 'warn',
  auth: null,
  jump: null,
  message: null,
  canReconnect: false,
  tmux: false,
  resumed: false,
};
const clients: QueryClient[] = [];

/** The snippets saved on this phone. */
function snippetsKv(): KvStore {
  const kv = new MemoryKv();
  seedLocalSync(kv, 'snippets', { version: 1, value: { v: 1, items } });
  return kv;
}

async function renderEditor(kv = snippetsKv()) {
  const onClose = jest.fn();
  const view = await renderWithProviders(<SnippetsEditor onClose={onClose} />, {
    core: createFakeCore(),
    kv,
  });
  clients.push(view.queryClient);
  await waitFor(() =>
    expect(screen.queryByTestId('snippets-loading')).toBeNull(),
  );
  return { ...view, onClose };
}

beforeEach(() => {
  jest.clearAllMocks();
  jest.useFakeTimers();
  jest.replaceProperty(Platform, 'OS', 'ios');
  resetSshStore();
  resetTerminalKeyboard();
});

afterEach(async () => {
  await cleanup();
  for (const client of clients.splice(0)) client.clear();
  resetSshStore();
  resetTerminalKeyboard();
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it.each(['ios', 'android'] as const)(
  'exposes the tab group and token-sized touch padding on %s',
  async os => {
    jest.replaceProperty(Platform, 'OS', os);
    jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
    const view = await renderWithProviders(
      <TerminalScreen
        navigation={mockNavigation<'Terminal'>()}
        route={mockRoute('Terminal', { serverId: prod.serverId })}
      />,
      { core: createFakeCore(), kv: snippetsKv() },
    );
    clients.push(view.queryClient);
    await screen.findByLabelText(terminal.sendCommand('docker ps'));

    const tabs = screen.getByLabelText(terminal.tabs);
    expect(tabs).toHaveProp('accessibilityRole', 'tablist');
    expect(StyleSheet.flatten(tabs.props.style)).toMatchObject({
      alignSelf: 'stretch',
    });
    const tab = screen.getByRole('tab', {
      name: terminal.tabLabel(prod.name, terminal.phase.opening),
    });
    expect(tab).toHaveProp('hitSlop', (hit.min - 36) / 2);
    expect(tab).toHaveProp('accessibilityState', { selected: true });
    expect(StyleSheet.flatten(tab.props.style)).toMatchObject({
      minHeight: 36,
    });
    expect(StyleSheet.flatten(tab.props.style).height).toBeUndefined();
    expect(screen.getByLabelText(terminal.sendCommand('docker ps'))).toHaveProp(
      'hitSlop',
      (hit.min - 36) / 2,
    );
  },
);

it('gives the empty snippet action the same touch padding', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  const view = await renderWithProviders(
    <SnippetBar items={[]} onEdit={jest.fn()} onSend={jest.fn()} />,
  );
  clients.push(view.queryClient);
  expect(screen.getByRole('button', { name: terminal.addSnippet })).toHaveProp(
    'hitSlop',
    (hit.min - 36) / 2,
  );
});

it.each(['ios', 'android'] as const)(
  'announces changed status once on iOS and uses a live region on Android (%s)',
  async os => {
    jest.replaceProperty(Platform, 'OS', os);
    const announce = jest.spyOn(AccessibilityInfo, 'announceForAccessibility');
    const view = await renderWithProviders(
      <StatusLine status={connecting} onReconnect={jest.fn()} />,
    );
    clients.push(view.queryClient);
    expect(announce).not.toHaveBeenCalled();
    expect(screen.getByText(terminal.connecting)).toHaveProp(
      'accessibilityLiveRegion',
      'polite',
    );

    const open: TermStatus = {
      ...connecting,
      phase: 'open',
      tone: 'good',
      auth: 'tailscale',
    };
    await view.rerender(<StatusLine status={open} onReconnect={jest.fn()} />);
    await view.rerender(
      <StatusLine status={{ ...open }} onReconnect={jest.fn()} />,
    );
    expect(screen.getByText(terminal.auth.tailscale)).toHaveProp(
      'accessibilityLiveRegion',
      'polite',
    );
    if (os === 'ios') {
      expect(announce).toHaveBeenCalledTimes(1);
      expect(announce).toHaveBeenLastCalledWith(terminal.auth.tailscale);
    } else {
      expect(announce).not.toHaveBeenCalled();
    }

    // Switching tabs can change the visible status without changing its phase.
    await view.rerender(
      <StatusLine
        status={{ ...open, auth: 'deviceKey' }}
        onReconnect={jest.fn()}
      />,
    );
    await view.rerender(
      <StatusLine
        status={{ ...open, phase: 'check' }}
        onReconnect={jest.fn()}
      />,
    );
    await view.rerender(
      <StatusLine
        status={{ ...open, phase: 'failed', message: null }}
        onReconnect={jest.fn()}
      />,
    );
    if (os === 'ios') {
      expect(announce.mock.calls.map(call => call[0])).toEqual([
        terminal.auth.tailscale,
        terminal.auth.deviceKey,
        terminal.check,
        terminal.failed,
      ]);
    } else {
      expect(announce).not.toHaveBeenCalled();
    }
  },
);

it.each(['ios', 'android'] as const)(
  'keeps the reconnect target inside its parent on %s',
  async os => {
    jest.replaceProperty(Platform, 'OS', os);
    const view = await renderWithProviders(
      <StatusLine
        status={{ ...connecting, phase: 'failed', canReconnect: true }}
        onReconnect={jest.fn()}
      />,
    );
    clients.push(view.queryClient);
    const expected = os === 'android' ? hit.min : 28;
    expect(
      StyleSheet.flatten(screen.getByTestId('terminal-status').props.style)
        .minHeight,
    ).toBe(expected);
    expect(
      StyleSheet.flatten(
        screen.getByRole('button', { name: terminal.reconnect }).parent?.props
          .style,
      ).minHeight,
    ).toBe(expected);
    await view.rerender(
      <StatusLine status={connecting} onReconnect={jest.fn()} />,
    );
    expect(
      StyleSheet.flatten(screen.getByTestId('terminal-status').props.style)
        .minHeight,
    ).toBe(28);
  },
);

it('returns from an Android edit to the list before closing the modal', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  const view = await renderEditor();
  await fireEvent.press(
    screen.getByLabelText(snippets.list.editA11y('docker ps')),
  );
  await fireEvent.changeText(
    screen.getByLabelText(snippets.editor.commandLabel),
    'htop',
  );
  const modal = screen.getByTestId('snippets-editor').parent!;
  expect(modal.type).toBe('Modal');
  await fireEvent(modal, 'requestClose');
  // T10: the edited draft asks before it is dropped.
  await fireEvent.press(
    screen.getByTestId('snippet-discard-confirm', {
      includeHiddenElements: true,
    }),
  );
  expect(screen.getByRole('header')).toHaveTextContent(snippets.title);
  expect(view.onClose).not.toHaveBeenCalled();
  expect(localSyncGet(view.kv, 'snippets')?.version).toBe(1);
  await fireEvent(modal, 'requestClose');
  expect(view.onClose).toHaveBeenCalledTimes(1);
});

it('keeps run-mode hints on snippet rows and the close button closes an edit', async () => {
  const view = await renderEditor();
  expect(screen.getByLabelText(snippets.list.editA11y('docker ps'))).toHaveProp(
    'accessibilityHint',
    snippets.list.run,
  );
  expect(
    screen.getByLabelText(snippets.list.editA11y('tail -f api.log')),
  ).toHaveProp('accessibilityHint', snippets.list.typeOnly);
  await fireEvent.press(
    screen.getByLabelText(snippets.list.editA11y('docker ps')),
  );
  await fireEvent.press(screen.getByTestId('snippets-editor-close'));
  expect(view.onClose).toHaveBeenCalledTimes(1);
});

it.each(['ios', 'android'] as const)(
  'exposes save errors without duplicate platform announcements on %s',
  async os => {
    jest.replaceProperty(Platform, 'OS', os);
    const announce = jest.spyOn(AccessibilityInfo, 'announceForAccessibility');
    const kv = snippetsKv();
    failLocalSync(kv, 'snippets', new Error('offline'));
    await renderEditor(kv);
    await fireEvent.press(screen.getByLabelText(snippets.list.add));
    await fireEvent.changeText(
      screen.getByLabelText(snippets.editor.commandLabel),
      'htop',
    );
    await fireEvent.press(screen.getByLabelText('保存'));
    const error = snippets.editor.saveFailed('offline');
    expect(await screen.findByText(error)).toHaveProp(
      'accessibilityLiveRegion',
      'polite',
    );
    if (os === 'ios') {
      expect(announce).toHaveBeenCalledTimes(1);
      expect(announce).toHaveBeenCalledWith(error);
    } else {
      expect(announce).not.toHaveBeenCalled();
    }
  },
);

it('keeps server names as row labels and describes the OS and login in hints', async () => {
  const view = await renderWithProviders(
    <NewTerminalSheet
      onClose={jest.fn()}
      onOpen={jest.fn()}
      onAddServer={jest.fn()}
    />,
  );
  clients.push(view.queryClient);
  await screen.findByLabelText(prod.name);
  for (const server of sshServers) {
    const subtitle =
      [server.os, terminal.serverAuth[server.auth]]
        .filter(Boolean)
        .join(' · ') + (server.jump ? terminal.via(server.jump) : '');
    expect(screen.getByRole('button', { name: server.name })).toHaveProp(
      'accessibilityHint',
      subtitle,
    );
  }
  expect(screen.getAllByRole('header')).toHaveLength(1);
  await act(() => jest.advanceTimersByTimeAsync(1));
});
