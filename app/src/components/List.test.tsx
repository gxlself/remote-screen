import { act, fireEvent, render, screen } from '@testing-library/react-native';
import { Platform, StyleSheet } from 'react-native';
import { ListGroup, ListRow, SectionHeader } from './List';

function rowStyle(testID = 'row') {
  return StyleSheet.flatten(screen.getByTestId(testID).props.style);
}

const touch = { persist: () => {}, nativeEvent: {}, currentTarget: 1 };

async function pressIn(testID = 'row') {
  await fireEvent(screen.getByTestId(testID), 'responderGrant', touch);
}

async function pressOut(testID = 'row') {
  await fireEvent(screen.getByTestId(testID), 'responderRelease', touch);
  await act(async () => {
    jest.advanceTimersByTime(200);
  });
}

describe('SectionHeader', () => {
  it('renders a header with optional right text', async () => {
    await render(<SectionHeader title="我的设备" right="3 台 · SSH" />);
    expect(screen.getByRole('header')).toHaveTextContent('我的设备');
    expect(screen.getByText('3 台 · SSH')).toBeOnTheScreen();
    expect(
      StyleSheet.flatten(screen.getByText('我的设备').props.style),
    ).toMatchObject({
      fontSize: 13,
      letterSpacing: 0.52,
      color: '#A9B4BB',
    });
  });

  it('lets long right text shrink and wrap beside the title', async () => {
    await render(
      <SectionHeader title="最近" right="长按电脑卡片可以查看详情和更多操作" />,
    );
    const right = screen.getByText('长按电脑卡片可以查看详情和更多操作');
    expect(StyleSheet.flatten(right.props.style)).toMatchObject({
      flexShrink: 1,
      textAlign: 'right',
    });
    expect(right.props.numberOfLines).toBeUndefined();
    expect(
      StyleSheet.flatten(screen.getByText('最近').props.style).flexShrink,
    ).toBe(1);
  });
});

describe('ListGroup', () => {
  it('draws separators between three rows', async () => {
    await render(
      <ListGroup>
        <ListRow title="一" />
        <ListRow title="二" />
        <ListRow title="三" />
      </ListGroup>,
    );
    expect(screen.getAllByTestId('list-separator')).toHaveLength(2);
  });

  it('skips null children when drawing separators', async () => {
    await render(
      <ListGroup>
        <ListRow title="一" />
        {null}
        <ListRow title="二" />
      </ListGroup>,
    );
    expect(screen.getAllByTestId('list-separator')).toHaveLength(1);
  });
});

describe('ListRow', () => {
  it('is a pressable button with the title and value label', async () => {
    const onPress = jest.fn();
    await render(
      <ListRow title="我的设备" value="3 台" onPress={onPress} testID="row" />,
    );
    const row = screen.getByLabelText('我的设备，3 台');
    expect(row).toHaveProp('accessibilityRole', 'button');
    await fireEvent.press(row);
    expect(onPress).toHaveBeenCalledTimes(1);
  });

  it('does not expose button semantics without onPress', async () => {
    await render(<ListRow title="自动更新" right={<></>} testID="row" />);
    expect(screen.queryByRole('button')).toBeNull();
    expect(
      StyleSheet.flatten(screen.getByTestId('row').props.style).minHeight,
    ).toBe(52);
  });

  it('uses the taller row with a subtitle', async () => {
    await render(
      <ListRow title="Studio Mac mini" subtitle="在线" testID="row" />,
    );
    expect(
      StyleSheet.flatten(screen.getByTestId('row').props.style),
    ).toMatchObject({ minHeight: 60, paddingVertical: 8 });
  });

  it('renders a muted chevron and the danger title colour', async () => {
    await render(
      <ListRow title="作废设备" tone="danger" chevron onPress={jest.fn()} />,
    );
    expect(
      screen.getByTestId('list-row-chevron', { includeHiddenElements: true }),
    ).toBeOnTheScreen();
    expect(
      StyleSheet.flatten(screen.getByText('作废设备').props.style).color,
    ).toBe('#FF8873');
  });

  it('highlights the row while pressed on iOS', async () => {
    jest.replaceProperty(Platform, 'OS', 'ios');
    jest.useFakeTimers();
    try {
      await render(<ListRow title="网络" onPress={jest.fn()} testID="row" />);
      expect(rowStyle().backgroundColor).toBeUndefined();
      await pressIn();
      expect(rowStyle().backgroundColor).toBe('rgba(255, 255, 255, 0.08)');
      await pressOut();
      expect(rowStyle().backgroundColor).toBeUndefined();
    } finally {
      jest.useRealTimers();
      jest.restoreAllMocks();
    }
  });

  it('keeps the Android ripple instead of the highlight', async () => {
    jest.replaceProperty(Platform, 'OS', 'android');
    jest.useFakeTimers();
    // The RN preset omits the native commands used by Android ripple feedback.
    const nativeView = jest.requireMock(
      'react-native/Libraries/Components/View/ViewNativeComponent',
    );
    const previousCommands = nativeView.Commands;
    const commands = { hotspotUpdate: jest.fn(), setPressed: jest.fn() };
    nativeView.Commands = commands;
    try {
      await render(<ListRow title="网络" onPress={jest.fn()} testID="row" />);
      await pressIn();
      expect(rowStyle().backgroundColor).toBeUndefined();
      expect(commands.setPressed).toHaveBeenCalledWith(expect.anything(), true);
      await pressOut();
    } finally {
      nativeView.Commands = previousCommands;
      jest.useRealTimers();
      jest.restoreAllMocks();
    }
  });

  it('dims a disabled row, announces it and stays silent', async () => {
    const onPress = jest.fn();
    await render(
      <ListRow title="网络" onPress={onPress} disabled testID="row" />,
    );
    const row = screen.getByTestId('row');
    expect(rowStyle().opacity).toBe(0.4);
    expect(row.props.accessibilityState.disabled).toBe(true);
    await fireEvent.press(row);
    expect(onPress).not.toHaveBeenCalled();
  });

  it('dims a disabled read-only row the same way', async () => {
    await render(<ListRow title="网络" disabled testID="row" />);
    expect(rowStyle().opacity).toBe(0.4);
    expect(screen.getByTestId('row').props.accessibilityState).toEqual({
      disabled: true,
    });
  });

  it('keeps an enabled row at full opacity', async () => {
    await render(<ListRow title="网络" onPress={jest.fn()} testID="row" />);
    expect(rowStyle().opacity).toBeUndefined();
  });
});
