import { render, screen } from '@testing-library/react-native';
import { Animated, StyleSheet, Text } from 'react-native';
import { StepList, type Step } from './StepList';

const steps: Step[] = [
  { key: 'scan', title: '扫描二维码', state: 'done' },
  { key: 'trust', title: '确认指纹', state: 'done' },
  { key: 'wait', title: '等待 Mac 确认', state: 'active' },
];

describe('StepList', () => {
  it('labels each step with its state and marks only the active step busy', async () => {
    await render(<StepList accessibilityLabel="配对进度" steps={steps} />);

    expect(screen.getByTestId('step-scan')).toHaveProp(
      'accessibilityLabel',
      '扫描二维码，已完成',
    );
    expect(screen.getByTestId('step-trust')).toHaveProp(
      'accessibilityLabel',
      '确认指纹，已完成',
    );
    expect(screen.getByTestId('step-wait')).toHaveProp(
      'accessibilityLabel',
      '等待 Mac 确认，进行中',
    );
    expect(screen.getByTestId('step-wait')).toHaveProp('accessibilityState', {
      busy: true,
    });
    expect(
      screen.getByTestId('step-scan').props.accessibilityState,
    ).toBeUndefined();
  });

  it('draws one connector between rows and colours it from the upper state', async () => {
    await render(<StepList accessibilityLabel="配对进度" steps={steps} />);

    const connectors = screen.getAllByTestId(/^step-.*-connector$/);
    expect(connectors).toHaveLength(2);
    expect(StyleSheet.flatten(connectors[0].props.style)).toMatchObject({
      backgroundColor: 'rgba(93,211,154,0.35)',
      width: 2,
      borderRadius: 1,
      left: 11,
    });
    expect(StyleSheet.flatten(connectors[1].props.style).backgroundColor).toBe(
      'rgba(93,211,154,0.35)',
    );
  });

  it('renders markers and right/extra slots', async () => {
    const richSteps: Step[] = [
      {
        key: 'done',
        title: '已连接',
        detail: '直连',
        right: <Text>直连 6 ms</Text>,
        state: 'done',
      },
      {
        key: 'failed',
        title: '校验指纹',
        extra: <Text>指纹块</Text>,
        state: 'failed',
      },
      { key: 'pending', title: '等待', state: 'pending' },
    ];

    await render(<StepList accessibilityLabel="连接步骤" steps={richSteps} />);

    expect(screen.getByText('直连 6 ms')).toBeOnTheScreen();
    expect(screen.getByText('指纹块')).toBeOnTheScreen();
    expect(screen.getByText('直连')).toBeOnTheScreen();
    expect(
      screen.getByTestId('step-done-marker-icon', {
        includeHiddenElements: true,
      }),
    ).toBeOnTheScreen();
    expect(
      screen.getByTestId('step-failed-marker-icon', {
        includeHiddenElements: true,
      }),
    ).toBeOnTheScreen();
    expect(
      StyleSheet.flatten(screen.getByTestId('step-pending-marker').props.style),
    ).toMatchObject({
      borderWidth: 2,
      borderColor: '#2A333A',
    });
  });

  it('does not start the spinner animation when reduceMotion is set', async () => {
    const loop = jest.spyOn(Animated, 'loop');
    try {
      await render(
        <StepList accessibilityLabel="配对进度" reduceMotion steps={steps} />,
      );
      expect(loop).not.toHaveBeenCalled();
    } finally {
      loop.mockRestore();
    }
  });

  it('spins the active marker when motion is allowed', async () => {
    const loop = jest.spyOn(Animated, 'loop');
    try {
      await render(<StepList accessibilityLabel="配对进度" steps={steps} />);
      expect(loop).toHaveBeenCalledTimes(1);
    } finally {
      loop.mockRestore();
    }
  });

  it('uses the card frame and step type metrics', async () => {
    await render(<StepList accessibilityLabel="配对进度" steps={steps} />);

    expect(screen.getByLabelText('配对进度')).toHaveStyle({
      borderRadius: 16,
      backgroundColor: '#151A1E',
      borderWidth: 1,
      borderColor: '#232B31',
      paddingVertical: 6,
      paddingHorizontal: 16,
    });
    expect(screen.getByText('等待 Mac 确认')).toHaveStyle({
      fontSize: 15,
      lineHeight: 22,
      fontFamily: 'Geist-SemiBold',
    });
    expect(screen.getByText('扫描二维码')).toHaveStyle({
      fontFamily: 'Geist-Medium',
    });
  });
});
