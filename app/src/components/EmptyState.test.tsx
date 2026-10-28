import { fireEvent, render, screen } from '@testing-library/react-native';
import { StyleSheet } from 'react-native';
import { EmptyState } from './EmptyState';

describe('EmptyState', () => {
  it('renders a centred 56 px icon tile, title and message', async () => {
    await render(
      <EmptyState
        icon="computer"
        message="在同一 Tailnet 下的电脑会出现在这里"
        title="还没有设备"
      />,
    );

    expect(
      StyleSheet.flatten(screen.getByTestId('empty-icon').props.style),
    ).toMatchObject({
      width: 56,
      height: 56,
      borderRadius: 16,
      backgroundColor: '#1C2328',
    });
    expect(
      screen.getByTestId('empty-icon-glyph', { includeHiddenElements: true })
        .props.bbWidth,
    ).toBe(26);
    expect(screen.getByText('还没有设备')).toHaveStyle({
      fontSize: 17,
      lineHeight: 22,
      fontFamily: 'Geist-SemiBold',
      textAlign: 'center',
    });
    expect(screen.getByText('在同一 Tailnet 下的电脑会出现在这里')).toHaveStyle(
      {
        fontSize: 15,
        lineHeight: 22,
        color: '#A9B4BB',
        textAlign: 'center',
      },
    );
    expect(screen.getByTestId('empty-state')).toHaveStyle({ gap: 12 });
  });

  it('fires the optional action', async () => {
    const onPress = jest.fn();
    await render(
      <EmptyState
        action={{ label: '添加设备', onPress }}
        icon="plus"
        title="还没有设备"
      />,
    );

    await fireEvent.press(screen.getByText('添加设备'));
    expect(onPress).toHaveBeenCalledTimes(1);
  });

  it('omits the button when there is no action', async () => {
    await render(<EmptyState icon="computer" title="还没有设备" />);
    expect(screen.queryByRole('button')).toBeNull();
  });
  it('marks the title as a header', async () => {
    await render(<EmptyState icon="computer" title="还没有设备" />);
    expect(
      screen.getByRole('header', { name: '还没有设备' }),
    ).toHaveTextContent('还没有设备');
  });

  it('centres its action button under the copy', async () => {
    await render(
      <EmptyState
        action={{ label: '添加设备', onPress: jest.fn(), testID: 'empty-add' }}
        icon="plus"
        title="还没有设备"
      />,
    );
    expect(
      StyleSheet.flatten(screen.getByTestId('empty-add').props.style),
    ).toMatchObject({ alignSelf: 'center' });
  });

  it('shows a busy action and ignores presses while loading', async () => {
    const onPress = jest.fn();
    const view = await render(
      <EmptyState
        action={{ label: '重试', onPress, loading: true, testID: 'retry' }}
        icon="wifi"
        title="连不上"
      />,
    );
    expect(screen.getByTestId('retry')).toBeBusy();
    expect(screen.getByTestId('retry-spinner')).toBeOnTheScreen();
    await fireEvent.press(screen.getByTestId('retry'));
    expect(onPress).not.toHaveBeenCalled();

    await view.rerender(
      <EmptyState
        action={{ label: '重试', onPress, disabled: true, testID: 'retry' }}
        icon="wifi"
        title="连不上"
      />,
    );
    expect(screen.getByTestId('retry')).toBeDisabled();
    await fireEvent.press(screen.getByTestId('retry'));
    expect(onPress).not.toHaveBeenCalled();
  });
});
