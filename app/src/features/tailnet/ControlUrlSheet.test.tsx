import {
  act,
  cleanup,
  fireEvent,
  screen,
  waitFor,
} from '@testing-library/react-native';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { renderWithProviders } from '../../test/render';
import { tailnet } from '../../i18n/zh/tailnet';
import { ControlUrlSheet } from './ControlUrlSheet';
import { resetTailnetSwitchStore } from './switchStore';

beforeEach(() => resetTailnetSwitchStore());
afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
});

it.each([false, true])(
  'keeps invalid input open with saveOnly=%s',
  async saveOnly => {
    const onDone = jest.fn();
    await renderWithProviders(
      <ControlUrlSheet
        onClose={jest.fn()}
        onDone={onDone}
        saveOnly={saveOnly}
      />,
      { core: createFakeCore({ tailnetAvailable: true }) },
    );
    await fireEvent.changeText(
      screen.getByTestId('tailnet-control-url-input'),
      'ftp://x',
    );
    await fireEvent.press(screen.getByTestId('tailnet-control-url-submit'));
    await waitFor(() => screen.getByText(tailnet.controlUrl.invalid));
    expect(screen.getByText(tailnet.controlUrl.invalid)).toBeOnTheScreen();
    expect(onDone).not.toHaveBeenCalled();
  },
);

it('saves and switches from the default sheet', async () => {
  const onDone = jest.fn();
  const view = await renderWithProviders(
    <ControlUrlSheet onClose={jest.fn()} onDone={onDone} />,
    { core: createFakeCore({ tailnetAvailable: true }) },
  );
  await fireEvent.changeText(
    screen.getByTestId('tailnet-control-url-input'),
    'hs.example.com',
  );
  await fireEvent.press(screen.getByRole('button', { name: '继续' }));
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
  expect(onDone).toHaveBeenCalledWith('switched');
  expect(view.settingsStore.get('tailnet.controlUrl')).toBe(
    'https://hs.example.com',
  );
  expect(view.settingsStore.get('tailnet.mode')).toBe('embedded');
  expect(screen.getByText(tailnet.controlUrl.note)).toBeOnTheScreen();
});

it('supports save-only mode, including clearing the URL', async () => {
  const onDone = jest.fn();
  const view = await renderWithProviders(
    <ControlUrlSheet onClose={jest.fn()} onDone={onDone} saveOnly />,
    { core: createFakeCore({ tailnetAvailable: true }) },
  );
  await fireEvent.changeText(
    screen.getByTestId('tailnet-control-url-input'),
    'hs.example.com',
  );
  await fireEvent.press(screen.getByRole('button', { name: '保存' }));
  await act(async () => {
    await Promise.resolve();
  });
  expect(onDone).toHaveBeenCalledWith('saved');
  expect(view.settingsStore.get('tailnet.mode')).toBe('system');
  expect(view.settingsStore.get('tailnet.controlUrl')).toBe(
    'https://hs.example.com',
  );

  onDone.mockClear();
  await fireEvent.changeText(
    screen.getByTestId('tailnet-control-url-input'),
    '',
  );
  await fireEvent.press(screen.getByTestId('tailnet-control-url-submit'));
  // S08: clearing a saved address asks first.
  const hidden = { includeHiddenElements: true };
  expect(screen.getByText('清空 Headscale 地址？', hidden)).toBeOnTheScreen();
  expect(onDone).not.toHaveBeenCalled();
  await fireEvent.press(
    screen.getByTestId('tailnet-control-url-confirm', hidden),
  );
  await act(async () => {
    await Promise.resolve();
  });
  expect(onDone).toHaveBeenCalledWith('saved');
  expect(view.settingsStore.get('tailnet.controlUrl')).toBeNull();
});

it('loads the existing URL and reports unavailable without changing mode', async () => {
  const onDone = jest.fn();
  const view = await renderWithProviders(
    <ControlUrlSheet onClose={jest.fn()} onDone={onDone} />,
    { settings: { 'tailnet.controlUrl': 'https://hs.example.com' } },
  );
  expect(screen.getByTestId('tailnet-control-url-input')).toHaveProp(
    'value',
    'https://hs.example.com',
  );
  await fireEvent.press(screen.getByRole('button', { name: '继续' }));
  await waitFor(() => expect(onDone).toHaveBeenCalledWith('unavailable'));
  expect(view.settingsStore.get('tailnet.mode')).toBe('system');
});

it('shows loading and accepts one submit while availability is pending', async () => {
  const core = createFakeCore({ tailnetAvailable: true });
  const onDone = jest.fn();
  await renderWithProviders(
    <ControlUrlSheet onClose={jest.fn()} onDone={onDone} />,
    { core },
  );
  let finish!: (available: boolean) => void;
  const pending = new Promise<boolean>(resolve => {
    finish = resolve;
  });
  const available = jest
    .spyOn(core.tailnet, 'available')
    .mockImplementationOnce(() => pending);
  await fireEvent.changeText(
    screen.getByTestId('tailnet-control-url-input'),
    'hs.example.com',
  );
  await fireEvent.press(screen.getByTestId('tailnet-control-url-submit'));
  expect(screen.getByTestId('tailnet-control-url-submit')).toBeBusy();
  expect(onDone).not.toHaveBeenCalled();
  await fireEvent.press(screen.getByTestId('tailnet-control-url-submit'));
  expect(available).toHaveBeenCalledTimes(1);
  await act(() => finish(true));
  expect(onDone).toHaveBeenCalledTimes(1);
  expect(onDone).toHaveBeenCalledWith('switched');
});

it('renders inside its own modal so it covers the screen', async () => {
  await renderWithProviders(
    <ControlUrlSheet onClose={jest.fn()} onDone={jest.fn()} />,
    { core: createFakeCore({ tailnetAvailable: true }) },
  );
  // S01: as a bare View the sheet was clipped by the settings scroll view.
  let modal = screen.getByTestId('tailnet-control-url-sheet').parent;
  while (modal !== null && modal.type !== 'Modal') {
    modal = modal.parent;
  }
  expect(modal).not.toBeNull();
  expect(modal!.props.visible).toBe(true);
  expect(modal!.props.transparent).toBe(true);
  expect(screen.getByTestId('tailnet-control-url-sheet')).toBeOnTheScreen();
});

it('confirms before changing a saved address and keeps it on cancel', async () => {
  const onDone = jest.fn();
  const view = await renderWithProviders(
    <ControlUrlSheet onClose={jest.fn()} onDone={onDone} saveOnly />,
    {
      core: createFakeCore({ tailnetAvailable: true }),
      settings: { 'tailnet.controlUrl': 'https://hs.example.com' },
    },
  );
  const hidden = { includeHiddenElements: true };
  await fireEvent.changeText(
    screen.getByTestId('tailnet-control-url-input'),
    'hs2.example.com',
  );
  await fireEvent.press(screen.getByTestId('tailnet-control-url-submit'));
  // S08: a new address means logging in again; say so first.
  expect(screen.getByText('改 Headscale 地址？', hidden)).toBeOnTheScreen();
  expect(
    screen.getByText('改了以后，App 内连接要在浏览器里重新登录。', hidden),
  ).toBeOnTheScreen();
  await fireEvent.press(screen.getAllByText('取消', hidden).at(-1)!);
  expect(onDone).not.toHaveBeenCalled();
  expect(view.settingsStore.get('tailnet.controlUrl')).toBe(
    'https://hs.example.com',
  );

  await fireEvent.press(screen.getByTestId('tailnet-control-url-submit'));
  await fireEvent.press(
    screen.getByTestId('tailnet-control-url-confirm', hidden),
  );
  await act(async () => {
    await Promise.resolve();
  });
  expect(onDone).toHaveBeenCalledWith('saved');
  expect(view.settingsStore.get('tailnet.controlUrl')).toBe(
    'https://hs2.example.com',
  );
});

it('does not ask when the saved address stays the same', async () => {
  const onDone = jest.fn();
  await renderWithProviders(
    <ControlUrlSheet onClose={jest.fn()} onDone={onDone} saveOnly />,
    { settings: { 'tailnet.controlUrl': 'https://hs.example.com' } },
  );
  await fireEvent.press(screen.getByTestId('tailnet-control-url-submit'));
  await act(async () => {
    await Promise.resolve();
  });
  expect(
    screen.queryByTestId('tailnet-control-url-dialog', {
      includeHiddenElements: true,
    }),
  ).toBeNull();
  expect(onDone).toHaveBeenCalledWith('saved');
});

it('explains a save-only sheet and submits from the keyboard', async () => {
  const onDone = jest.fn();
  await renderWithProviders(
    <ControlUrlSheet onClose={jest.fn()} onDone={onDone} saveOnly />,
    { core: createFakeCore({ tailnetAvailable: true }) },
  );
  // S09: the save-only note does not promise a browser right now.
  expect(screen.getByText(tailnet.controlUrl.saveOnlyNote)).toBeOnTheScreen();
  expect(screen.queryByText(tailnet.controlUrl.note)).toBeNull();
  const input = screen.getByTestId('tailnet-control-url-input');
  expect(input).toHaveProp('returnKeyType', 'done');
  expect(input).toHaveProp('textContentType', 'URL');
  await fireEvent.changeText(input, 'hs.example.com');
  // S29
  await fireEvent(input, 'submitEditing');
  await act(async () => {
    await Promise.resolve();
  });
  expect(onDone).toHaveBeenCalledWith('saved');
});
