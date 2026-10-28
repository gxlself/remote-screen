import { cleanup, fireEvent, screen } from '@testing-library/react-native';
import { switcher as copy } from '../../i18n/zh/switcher';
import { renderWithProviders } from '../../test/render';
import { SessionSwitcher } from './SessionSwitcher';
import { buildSwitcherList } from './switcher';
import type { SwitcherItem, SwitcherList } from './switcher';

const mac: SwitcherItem = {
  kind: 'host',
  id: 's1',
  hostId: 'h1',
  name: 'Studio Mac mini',
  status: 'connected',
  startedAtUs: 100,
};
const windows: SwitcherItem = {
  kind: 'rdp',
  id: 'a2',
  params: { target: { protocol: 'rdp', addr: '100.64.0.9' }, name: 'Win PC' },
  name: 'Win PC',
  status: 'reconnecting',
  startedAtUs: 200,
};
const nas: SwitcherItem = {
  kind: 'ssh',
  id: 'x3',
  serverId: 'srv',
  name: 'NAS',
  status: 'connected',
  startedAtUs: 50,
};

function vncAt(index: number): SwitcherItem {
  return {
    kind: 'vnc',
    id: `v${index}`,
    params: { target: { protocol: 'vnc', addr: `10.0.0.${index}` } },
    name: `Mac ${index}`,
    status: 'connected',
    startedAtUs: 1000 + index,
  };
}

async function mount(
  list: SwitcherList,
  currentId: string | null,
  shortcuts = false,
) {
  const onSelect = jest.fn();
  const onOther = jest.fn();
  const onClose = jest.fn();
  await renderWithProviders(
    <SessionSwitcher
      currentId={currentId}
      list={list}
      onClose={onClose}
      onOther={onOther}
      onSelect={onSelect}
      shortcuts={shortcuts}
    />,
  );
  return { onSelect, onOther, onClose };
}

afterEach(async () => {
  await cleanup();
});

it('lists every session with its kind and status, the current one marked', async () => {
  await mount(buildSwitcherList([windows, mac, nas]), 'a2');

  expect(screen.getByText(copy.title)).toBeOnTheScreen();
  const current = screen.getByTestId('switcher-item-a2');
  expect(current).toBeSelected();
  expect(current).toHaveProp(
    'accessibilityLabel',
    'Win PC，远程桌面，重连中，当前会话',
  );
  expect(screen.getByTestId('switcher-item-s1')).not.toBeSelected();
  expect(screen.getByText('远控 · 已连接')).toBeOnTheScreen();
  expect(screen.getByText('远程桌面 · 重连中')).toBeOnTheScreen();
  // Terminals have their own group.
  expect(screen.getByText(copy.terminals)).toBeOnTheScreen();
  expect(screen.getByText('终端 · 已连接')).toBeOnTheScreen();
  expect(screen.getByTestId('switcher-other')).toBeOnTheScreen();
  expect(screen.queryByTestId('switcher-many')).toBeNull();
});

it('with one session shows it and 连接其他电脑', async () => {
  await mount(buildSwitcherList([], vncAt(1)), 'v1');

  expect(screen.getByTestId('switcher-item-v1')).toBeSelected();
  expect(screen.queryByText(copy.terminals)).toBeNull();
  expect(screen.getByText(copy.other)).toBeOnTheScreen();
});

it('reports the tapped session, 连接其他电脑 and closing', async () => {
  const { onSelect, onOther, onClose } = await mount(
    buildSwitcherList([windows, mac, nas]),
    'a2',
  );

  await fireEvent.press(screen.getByTestId('switcher-item-s1'));
  expect(onSelect).toHaveBeenCalledWith(mac);
  await fireEvent.press(screen.getByTestId('switcher-item-x3'));
  expect(onSelect).toHaveBeenLastCalledWith(nas);
  await fireEvent.press(screen.getByTestId('switcher-other'));
  expect(onOther).toHaveBeenCalledTimes(1);
  expect(onClose).not.toHaveBeenCalled();
});

it('numbers the first nine screens on iPad', async () => {
  const items = Array.from({ length: 10 }, (_, index) => vncAt(index + 1));
  await mount(buildSwitcherList(items), 'v1', true);

  expect(screen.getByText(copy.current)).toBeOnTheScreen();
  expect(screen.getByText('⌃⌥⌘2')).toBeOnTheScreen();
  expect(screen.getByText('⌃⌥⌘9')).toBeOnTheScreen();
  expect(screen.queryByText('⌃⌥⌘10')).toBeNull();
  expect(screen.getByTestId('switcher-item-v3')).toHaveProp(
    'accessibilityHint',
    copy.a11y.shortcut(3),
  );
});

it('says it costs battery only above four sessions', async () => {
  await mount(
    buildSwitcherList([vncAt(1), vncAt(2), vncAt(3), vncAt(4)]),
    'v1',
  );
  expect(screen.queryByTestId('switcher-many')).toBeNull();
  await cleanup();

  await mount(
    buildSwitcherList([vncAt(1), vncAt(2), vncAt(3), vncAt(4), nas]),
    'v1',
  );
  expect(screen.getByTestId('switcher-many')).toHaveTextContent(
    '同时连着 5 台，会更耗电',
  );
});

describe('iPad windows (RS-WIN1)', () => {
  it('offers 在新窗口打开 next to the other screens', async () => {
    const onNewWindow = jest.fn();
    const onSelect = jest.fn();
    await renderWithProviders(
      <SessionSwitcher
        currentId="s1"
        list={buildSwitcherList([windows, mac, nas])}
        onClose={jest.fn()}
        onNewWindow={onNewWindow}
        onOther={jest.fn()}
        onSelect={onSelect}
      />,
    );
    // Not for the current session unless it can move, nor for terminals.
    expect(screen.queryByTestId('switcher-new-window-s1')).toBeNull();
    expect(screen.queryByTestId('switcher-new-window-x3')).toBeNull();
    const button = screen.getByTestId('switcher-new-window-a2');
    expect(button).toHaveProp('accessibilityLabel', '在新窗口打开 Win PC');
    await fireEvent.press(button);
    expect(onNewWindow).toHaveBeenCalledWith(windows);
    expect(onSelect).not.toHaveBeenCalled();
  });

  it('lets the main window move its current session', async () => {
    await renderWithProviders(
      <SessionSwitcher
        currentId="s1"
        currentNewWindow
        list={buildSwitcherList([mac])}
        onClose={jest.fn()}
        onNewWindow={jest.fn()}
        onOther={jest.fn()}
        onSelect={jest.fn()}
      />,
    );
    expect(screen.getByTestId('switcher-new-window-s1')).toBeOnTheScreen();
  });

  it('marks sessions shown in another window', async () => {
    const onSelect = jest.fn();
    await renderWithProviders(
      <SessionSwitcher
        currentId={null}
        elsewhereIds={['a2']}
        list={buildSwitcherList([windows, mac])}
        onClose={jest.fn()}
        onNewWindow={jest.fn()}
        onOther={jest.fn()}
        onSelect={onSelect}
      />,
    );
    expect(screen.getByTestId('switcher-elsewhere-a2')).toHaveTextContent(
      '在其他窗口',
    );
    expect(screen.queryByTestId('switcher-new-window-a2')).toBeNull();
    expect(screen.getByTestId('switcher-item-a2')).toHaveProp(
      'accessibilityHint',
      '点按切到那个窗口',
    );
    await fireEvent.press(screen.getByTestId('switcher-item-a2'));
    expect(onSelect).toHaveBeenCalledWith(windows);
  });

  it('shows no window buttons without system windows', async () => {
    await mount(buildSwitcherList([windows, mac]), 's1');
    expect(screen.queryByTestId('switcher-new-window-a2')).toBeNull();
  });
});
