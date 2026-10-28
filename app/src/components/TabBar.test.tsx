import type { BottomTabBarProps } from '@react-navigation/bottom-tabs';
import { fireEvent, render, screen } from '@testing-library/react-native';
import TabBar from './TabBar';

type TabName = 'Home' | 'Sessions' | 'Files' | 'Settings';

function setup(index = 0, badges: Partial<Record<TabName, number>> = {}) {
  const names: TabName[] = ['Home', 'Sessions', 'Files', 'Settings'];
  const routes = names.map(name => ({ key: `${name}-key`, name }));
  const descriptors = Object.fromEntries(
    routes.map(route => [
      route.key,
      { options: { tabBarBadge: badges[route.name as TabName] } },
    ]),
  );
  const navigation = {
    emit: jest.fn(() => ({ defaultPrevented: false })),
    navigate: jest.fn(),
  };
  const props = {
    state: { index, routes },
    descriptors,
    navigation,
    insets: { top: 0, right: 0, bottom: 0, left: 0 },
  } as unknown as BottomTabBarProps;

  return { props, navigation };
}

describe('TabBar', () => {
  it('renders the four designed tabs', async () => {
    const { props } = setup();
    await render(<TabBar {...props} />);
    expect(screen.getByText('设备')).toBeOnTheScreen();
    expect(screen.getByText('会话')).toBeOnTheScreen();
    expect(screen.getByText('文件')).toBeOnTheScreen();
    expect(screen.getByText('设置')).toBeOnTheScreen();
    expect(screen.getByText('设备').parent?.parent).toHaveProp(
      'accessibilityRole',
      'tablist',
    );
  });

  it('marks the current tab selected', async () => {
    const { props } = setup(0);
    await render(<TabBar {...props} />);
    expect(screen.getByLabelText('设备')).toHaveProp('accessibilityState', {
      selected: true,
    });
    expect(screen.getByLabelText('会话')).toHaveProp('accessibilityState', {
      selected: false,
    });
  });

  it('emits tabPress and navigates to an unselected tab', async () => {
    const { props, navigation } = setup(0);
    await render(<TabBar {...props} />);
    await fireEvent.press(screen.getByLabelText('会话'));
    expect(navigation.emit).toHaveBeenCalledWith({
      type: 'tabPress',
      target: 'Sessions-key',
      canPreventDefault: true,
    });
    expect(navigation.navigate).toHaveBeenCalledWith('Sessions');
  });

  it('shows a positive badge and includes the count in the tab label', async () => {
    const { props } = setup(0, { Sessions: 3 });
    await render(<TabBar {...props} />);
    expect(screen.getByText('3')).toBeOnTheScreen();
    expect(screen.getByLabelText('会话，3 个进行中')).toBeOnTheScreen();
  });

  it('hides a zero badge', async () => {
    const { props } = setup(0, { Sessions: 0 });
    await render(<TabBar {...props} />);
    expect(screen.queryByText('0')).toBeNull();
    expect(screen.getByLabelText('会话')).toBeOnTheScreen();
  });

  it('does not navigate when the current tab is pressed', async () => {
    const { props, navigation } = setup(0);
    await render(<TabBar {...props} />);
    await fireEvent.press(screen.getByLabelText('设备'));
    expect(navigation.emit).toHaveBeenCalled();
    expect(navigation.navigate).not.toHaveBeenCalled();
  });

  it('does not navigate when tabPress is prevented', async () => {
    const { props, navigation } = setup(0);
    navigation.emit.mockReturnValueOnce({ defaultPrevented: true });
    await render(<TabBar {...props} />);
    await fireEvent.press(screen.getByLabelText('会话'));
    expect(navigation.navigate).not.toHaveBeenCalled();
  });
});
