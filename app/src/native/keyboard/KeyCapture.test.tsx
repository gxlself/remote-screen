import {
  cleanup,
  fireEvent,
  render,
  screen,
} from '@testing-library/react-native';
import { StyleSheet } from 'react-native';
import { CoreProvider } from '../rscore/CoreContext';
import { createFakeCore } from '../rscore/fakeCore';
import { KeyCapture, parseSessionShortcut } from './KeyCapture';
import type { KeyCaptureProps } from './KeyCapture';

jest.mock('../specs/RsKeyCaptureNativeComponent', () => {
  const { View } = require('react-native');
  return {
    __esModule: true,
    default: (props: Record<string, unknown>) => (
      <View {...props} testID="keycapture" />
    ),
  };
});

const defaults: KeyCaptureProps = {
  sessionId: 's-keyboard',
  active: true,
  inputMode: 'text',
  stickyMods: 0,
  os: 'macos',
};

async function mount(props: Partial<KeyCaptureProps> = {}) {
  const core = createFakeCore();
  await render(
    <CoreProvider core={core}>
      <KeyCapture {...defaults} {...props} />
    </CoreProvider>,
  );
  return core;
}

function capture() {
  return screen.getByTestId('keycapture', { includeHiddenElements: true });
}

it('forwards native key down/up to the current session unchanged', async () => {
  const core = await mount();
  await fireEvent(capture(), 'key', {
    nativeEvent: { usage: 6, down: true, mods: 8 },
  });
  await fireEvent(capture(), 'key', {
    nativeEvent: { usage: 6, down: false, mods: 8 },
  });

  expect(core.calls).toEqual([
    { method: 'sessions.key', args: ['s-keyboard', 6, true, 8] },
    { method: 'sessions.key', args: ['s-keyboard', 6, false, 8] },
  ]);
});

it('forwards committed text while composition only notifies its callback', async () => {
  const onComposition = jest.fn();
  const core = await mount({ onComposition });
  await fireEvent(capture(), 'composition', {
    nativeEvent: { text: 'nihao' },
  });
  expect(core.calls).toEqual([]);
  await fireEvent(capture(), 'text', {
    nativeEvent: { text: '你好' },
  });
  await fireEvent(capture(), 'composition', {
    nativeEvent: { text: '' },
  });

  expect(core.calls).toEqual([
    { method: 'sessions.text', args: ['s-keyboard', '你好'] },
  ]);
  expect(onComposition.mock.calls).toEqual([['nihao'], ['']]);
});

it('drops key and text events without a session', async () => {
  const core = await mount({ sessionId: null });
  await fireEvent(capture(), 'key', {
    nativeEvent: { usage: 6, down: true, mods: 8 },
  });
  await fireEvent(capture(), 'text', {
    nativeEvent: { text: '你好' },
  });
  expect(core.calls).toEqual([]);
});

it('unwraps sticky, keyboard frame and active change events', async () => {
  const onStickyConsumed = jest.fn();
  const onKeyboardFrame = jest.fn();
  const onActiveChange = jest.fn();
  await mount({ onStickyConsumed, onKeyboardFrame, onActiveChange });
  await fireEvent(capture(), 'stickyConsumed', {
    nativeEvent: { mods: 8 },
  });
  await fireEvent(capture(), 'keyboardFrame', {
    nativeEvent: { height: 291, visible: true, hardware: false },
  });
  await fireEvent(capture(), 'activeChange', {
    nativeEvent: { active: false },
  });

  expect(onStickyConsumed).toHaveBeenCalledWith(8);
  expect(onKeyboardFrame).toHaveBeenCalledWith({
    height: 291,
    visible: true,
    hardware: false,
  });
  expect(onActiveChange).toHaveBeenCalledWith(false);
});

it('logs failures without including text, session IDs or error contents', async () => {
  const core = await mount();
  core.failNext('sessions.key', new Error('private key error details'));
  core.failNext('sessions.text', new Error('private text error details'));
  await fireEvent(capture(), 'key', {
    nativeEvent: { usage: 6, down: true, mods: 8 },
  });
  await fireEvent(capture(), 'text', {
    nativeEvent: { text: 'private committed text' },
  });

  expect(core.calls.filter(call => call.method === 'logs.write')).toEqual([
    {
      method: 'logs.write',
      args: ['warn', 'keyboard', 'Failed to send key event'],
    },
    {
      method: 'logs.write',
      args: ['warn', 'keyboard', 'Failed to send text event'],
    },
  ]);
});

it('passes configuration to an invisible, inaccessible native view', async () => {
  await mount({ active: false, inputMode: 'keys', stickyMods: 0x80, os: 'windows' });
  const native = capture();
  expect(native.props).toMatchObject({
    active: false,
    inputMode: 'keys',
    stickyMods: 0x80,
    os: 'windows',
    pointerEvents: 'none',
    accessible: false,
    importantForAccessibility: 'no',
    accessibilityElementsHidden: true,
  });
  expect(StyleSheet.flatten(native.props.style)).toMatchObject({
    width: 1,
    height: 1,
    position: 'absolute',
  });
});

it('asks for paste sync only with a handler and forwards the held modifiers', async () => {
  await mount({ syncPaste: true });
  expect(capture()).toHaveProp('syncPaste', false);
  await cleanup();

  const onPaste = jest.fn();
  const core = await mount({ syncPaste: true, onPaste });
  expect(capture()).toHaveProp('syncPaste', true);
  await fireEvent(capture(), 'paste', { nativeEvent: { mods: 0x08 } });
  expect(onPaste).toHaveBeenCalledWith(0x08);
  // The keys go out after the clipboard push, from the caller.
  expect(core.calls).toEqual([]);
});

it('hands ⌃⌥⌘ session shortcuts to JS and sends nothing to the computer', async () => {
  const onShortcut = jest.fn();
  const core = await mount({ onShortcut });

  for (const nativeEvent of [
    { action: 'next', index: 0 },
    { action: 'previous', index: 0 },
    { action: 'index', index: 3 },
    { action: 'switcher', index: 0 },
  ]) {
    await fireEvent(capture(), 'shortcut', { nativeEvent });
  }
  expect(onShortcut.mock.calls).toEqual([
    [{ action: 'next' }],
    [{ action: 'previous' }],
    [{ action: 'index', index: 3 }],
    [{ action: 'switcher' }],
  ]);
  expect(core.calls).toEqual([]);
});

it('drops shortcut events it does not know', async () => {
  const onShortcut = jest.fn();
  await mount({ onShortcut });

  for (const nativeEvent of [
    { action: 'index', index: 0 },
    { action: 'index', index: 1.5 },
    { action: 'zoom', index: 0 },
  ]) {
    await fireEvent(capture(), 'shortcut', { nativeEvent });
  }
  expect(onShortcut).not.toHaveBeenCalled();
  expect(parseSessionShortcut({ action: 'index', index: 9 })).toEqual({
    action: 'index',
    index: 9,
  });
});
