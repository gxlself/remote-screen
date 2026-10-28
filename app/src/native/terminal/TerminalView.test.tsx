import { createRef } from 'react';
import { act, render } from '@testing-library/react-native';
import { Commands } from '../specs/RsTerminalViewNativeComponent';
import type { NativeProps } from '../specs/RsTerminalViewNativeComponent';
import { TerminalView } from './TerminalView';
import type { TerminalViewHandle } from './TerminalView';

let mockLastProps: NativeProps;
const mockNativeHandle = {};

jest.mock('../specs/RsTerminalViewNativeComponent', () => {
  const React = require('react');
  const { View } = require('react-native');
  return {
    __esModule: true,
    default: function MockNativeTerminal(
      p: NativeProps & { ref?: React.Ref<unknown> },
    ) {
      mockLastProps = p;
      React.useImperativeHandle(p.ref, () => mockNativeHandle);
      return <View testID="native-terminal" />;
    },
    Commands: {
      sendSequence: jest.fn(),
      sendKey: jest.fn(),
      copySelection: jest.fn(),
      paste: jest.fn(),
    },
  };
});

const props = {
  sshId: null,
  fontSize: 13,
  theme: 'night' as const,
  accessibilityLabel: '终端',
};

beforeEach(() => jest.clearAllMocks());

it('normalizes an empty ssh id and forwards accessibility props', async () => {
  await render(<TerminalView {...props} />);
  expect(mockLastProps.sshId).toBe('');
  expect(mockLastProps.accessible).toBe(true);
  expect(mockLastProps.accessibilityLabel).toBe('终端');
  expect(mockLastProps.fontSize).toBe(13);
  expect(mockLastProps.theme).toBe('night');
});

it('forwards imperative commands to native', async () => {
  const ref = createRef<TerminalViewHandle>();
  await render(<TerminalView {...props} ref={ref} />);
  ref.current?.sendSequence('ls');
  ref.current?.sendKey('up');
  ref.current?.paste();
  ref.current?.copySelection();
  expect(Commands.sendSequence).toHaveBeenCalledWith(mockNativeHandle, 'ls');
  expect(Commands.sendKey).toHaveBeenCalledWith(mockNativeHandle, 'up');
  expect(Commands.paste).toHaveBeenCalledWith(mockNativeHandle);
  expect(Commands.copySelection).toHaveBeenCalledWith(mockNativeHandle);
});

it('unwraps native events', async () => {
  const onSizeChange = jest.fn();
  const onModifiersConsumed = jest.fn();
  const onKeyboardFrame = jest.fn();
  const onActiveChange = jest.fn();
  const onCopied = jest.fn();
  await render(
    <TerminalView
      {...props}
      {...{
        onSizeChange,
        onModifiersConsumed,
        onKeyboardFrame,
        onActiveChange,
        onCopied,
      }}
    />,
  );
  await act(async () => {
    mockLastProps.onSizeChange?.({
      nativeEvent: { cols: 80, rows: 24 },
    } as never);
    mockLastProps.onModifiersConsumed?.({ nativeEvent: { mods: 3 } } as never);
    mockLastProps.onKeyboardFrame?.({
      nativeEvent: { height: 300, visible: true, hardware: false },
    } as never);
    mockLastProps.onActiveChange?.({ nativeEvent: { active: true } } as never);
    mockLastProps.onCopied?.({ nativeEvent: { length: 2 } } as never);
  });
  expect(onSizeChange).toHaveBeenCalledWith(80, 24);
  expect(onModifiersConsumed).toHaveBeenCalledWith(3);
  expect(onKeyboardFrame).toHaveBeenCalledWith({
    height: 300,
    visible: true,
    hardware: false,
  });
  expect(onActiveChange).toHaveBeenCalledWith(true);
  expect(onCopied).toHaveBeenCalledWith(2);
});
