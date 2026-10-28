import { render, screen } from '@testing-library/react-native';
import { Pressable, View } from 'react-native';
import { StepList } from './StepList';

describe('StepList accessibility', () => {
  it('includes detail in the spoken step label', async () => {
    await render(
      <StepList
        accessibilityLabel="连接步骤"
        steps={[
          {
            detail: '请检查主机上的提示',
            key: 'connect',
            state: 'active',
            title: '等待连接',
          },
        ]}
      />,
    );

    expect(screen.getByTestId('step-connect')).toHaveProp(
      'accessibilityLabel',
      '等待连接，请检查主机上的提示，进行中',
    );
  });

  it('does not swallow an interactive right slot', async () => {
    await render(
      <StepList
        accessibilityLabel="连接步骤"
        steps={[
          {
            detail: '请检查主机上的提示',
            key: 'connect',
            right: (
              <Pressable
                accessibilityLabel="打开详情"
                onPress={jest.fn()}
                testID="right-action"
              />
            ),
            state: 'active',
            title: '等待连接',
          },
        ]}
      />,
    );

    const action = screen.getByTestId('right-action');
    let ancestor = action.parent;
    while (ancestor != null) {
      expect(ancestor.props.accessible).not.toBe(true);
      ancestor = ancestor.parent;
    }
    expect(screen.getByTestId('step-connect')).toHaveProp('accessible', true);
    expect(screen.getByTestId('step-connect')).toHaveProp(
      'accessibilityLabel',
      '等待连接，请检查主机上的提示，进行中',
    );
    expect(screen.getByTestId('step-connect')).toHaveProp(
      'accessibilityState',
      {
        busy: true,
      },
    );
    expect(screen.getByTestId('step-connect-row')).not.toHaveProp('accessible');
  });

  it('does not swallow an accessibility progress bar in extra', async () => {
    await render(
      <StepList
        accessibilityLabel="唤醒进度"
        steps={[
          {
            extra: (
              <View
                accessibilityLabel="已等待 3 秒"
                accessibilityRole="progressbar"
                testID="extra-progress"
              />
            ),
            key: 'wake',
            state: 'pending',
            title: '等待 Mac 唤醒',
          },
        ]}
      />,
    );

    const progress = screen.getByTestId('extra-progress');
    expect(progress).toHaveProp('accessibilityRole', 'progressbar');
    let ancestor = progress.parent;
    while (ancestor != null) {
      expect(ancestor.props.accessible).not.toBe(true);
      ancestor = ancestor.parent;
    }
  });

  it('keeps a step without slots as the accessible row', async () => {
    await render(
      <StepList
        accessibilityLabel="连接步骤"
        steps={[{ key: 'plain', state: 'done', title: '已连接' }]}
      />,
    );

    expect(screen.getByTestId('step-plain')).toHaveProp('accessible', true);
    expect(screen.queryByTestId('step-plain-row')).toBeNull();
  });
});
