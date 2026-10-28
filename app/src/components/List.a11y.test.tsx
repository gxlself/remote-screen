import { fireEvent, screen } from '@testing-library/react-native';
import { Platform, processColor } from 'react-native';
import { renderWithProviders } from '../test/render';
import AppText from './AppText';
import { ListRow } from './List';
import { Switch } from './Switch';

beforeEach(() => {
  jest.replaceProperty(Platform, 'OS', 'ios');
});

afterEach(() => {
  jest.restoreAllMocks();
});

describe('ListRow accessible labels and grouping', () => {
  it('includes the subtitle in the default button label', async () => {
    await renderWithProviders(
      <ListRow title="标题" subtitle="副标题" onPress={jest.fn()} />,
    );
    expect(screen.getByRole('button', { name: '标题，副标题' })).toBeOnTheScreen();
  });

  it('joins the title, subtitle and a zero value in order', async () => {
    await renderWithProviders(
      <ListRow title="标题" subtitle="副标题" value={0} onPress={jest.fn()} />,
    );
    expect(screen.getByRole('button', { name: '标题，副标题，0' })).toBeOnTheScreen();
  });

  it('omits empty label parts', async () => {
    await renderWithProviders(
      <ListRow title="标题" subtitle="" value="" onPress={jest.fn()} />,
    );
    expect(screen.getByRole('button', { name: '标题' })).toBeOnTheScreen();
  });

  it('preserves a caller supplied button label', async () => {
    await renderWithProviders(
      <ListRow
        accessibilityLabel="完整说明"
        title="标题"
        subtitle="副标题"
        value="值"
        onPress={jest.fn()}
      />,
    );
    expect(screen.getByRole('button', { name: '完整说明' })).toBeOnTheScreen();
  });

  it('groups a read-only row with an explicit label and custom value', async () => {
    await renderWithProviders(
      <ListRow
        accessibilityLabel="账号头像，用户"
        title="用户"
        value={<AppText>在线</AppText>}
        testID="row"
      />,
    );
    const row = screen.getByTestId('row');
    expect(row).toHaveProp('accessible', true);
    expect(row).toHaveProp('accessibilityLabel', '账号头像，用户');
    expect(screen.queryByRole('button')).toBeNull();
  });

  it.each([undefined, null, '值', 0])(
    'groups a read-only row with a primitive value (%s)',
    async value => {
      await renderWithProviders(
        <ListRow title="标题" subtitle="副标题" value={value} testID="row" />,
      );
      const row = screen.getByTestId('row');
      expect(row).toHaveProp('accessible', true);
      expect(row).toHaveProp(
        'accessibilityLabel',
        value == null ? '标题，副标题' : `标题，副标题，${value}`,
      );
    },
  );

  it('leaves a right-hand switch independently accessible despite a row label', async () => {
    const onValueChange = jest.fn();
    await renderWithProviders(
      <ListRow
        accessibilityLabel="自动更新说明"
        title="自动更新"
        right={
          <Switch
            accessibilityLabel="自动更新"
            value={false}
            onValueChange={onValueChange}
          />
        }
        testID="row"
      />,
    );
    const row = screen.getByTestId('row');
    expect(row.props.accessible).toBeUndefined();
    expect(row.props.accessibilityLabel).toBeUndefined();
    await fireEvent.press(screen.getByRole('switch', { name: '自动更新' }));
    expect(onValueChange).toHaveBeenCalledWith(true);
  });

  it('does not group a read-only row with an unlabeled custom value', async () => {
    await renderWithProviders(
      <ListRow title="标题" value={<AppText>自定义值</AppText>} testID="row" />,
    );
    expect(screen.getByTestId('row').props.accessible).toBeUndefined();
    expect(screen.getByTestId('row').props.accessibilityLabel).toBeUndefined();
  });

  it('forwards accessibilityState while retaining disabled button behavior', async () => {
    const onPress = jest.fn();
    await renderWithProviders(
      <ListRow
        accessibilityState={{ busy: true, expanded: true, selected: true }}
        disabled
        title="标题"
        onPress={onPress}
      />,
    );
    const row = screen.getByRole('button', { name: '标题' });
    expect(row).toHaveProp(
      'accessibilityState',
      expect.objectContaining({
        busy: true,
        expanded: true,
        selected: true,
        disabled: true,
      }),
    );
    await fireEvent.press(row);
    expect(onPress).not.toHaveBeenCalled();
  });

  it('forwards accessibilityState to read-only rows', async () => {
    await renderWithProviders(
      <ListRow title="标题" accessibilityState={{ busy: true }} testID="row" />,
    );
    expect(screen.getByTestId('row')).toHaveProp('accessibilityState', {
      busy: true,
    });
  });

  it('forwards accessibilityState to enabled buttons', async () => {
    await renderWithProviders(
      <ListRow
        title="标题"
        accessibilityState={{ busy: true, expanded: false }}
        onPress={jest.fn()}
      />,
    );
    expect(screen.getByRole('button', { name: '标题' })).toHaveProp(
      'accessibilityState',
      expect.objectContaining({ busy: true, expanded: false }),
    );
  });

  it.each(['ios', 'android'] as const)(
    'uses native ripple feedback only on Android (%s)',
    async os => {
      jest.replaceProperty(Platform, 'OS', os);
      await renderWithProviders(<ListRow title="标题" onPress={jest.fn()} />);
      const row = screen.getByRole('button', { name: '标题' });
      // Pressable converts android_ripple into the native foreground prop.
      if (os === 'android') {
        expect(row).toHaveProp(
          'nativeForegroundAndroid',
          expect.objectContaining({
            type: 'RippleAndroid',
            color: processColor('rgba(255, 255, 255, 0.12)'),
          }),
        );
      } else {
        expect(row.props.nativeForegroundAndroid).toBeUndefined();
        expect(row.props.nativeBackgroundAndroid).toBeUndefined();
      }
    },
  );
});
