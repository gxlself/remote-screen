import { act, cleanup, fireEvent, render } from '@testing-library/react-native';
import { AccessibilityInfo, PixelRatio, Platform } from 'react-native';
import type { RootScreenProps } from '../../app/navigation/routes';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { sshServers } from '../../native/rscore/fixtures';
import { renderWithProviders } from '../../test/render';
import { mockNavigation, mockRoute } from '../../test/navigation';
import AddServerScreen from './index';
import { NetworkPicker } from './PickerSheet';
import { TestResult } from './TestResult';
import type { TestState } from '../../features/ssh/serverForm';

jest.setTimeout(20_000);

function keyboardBehavior(instance: { unstable_fiber?: unknown }) {
  let fiber = instance.unstable_fiber as
    | { memoizedProps?: Record<string, unknown>; return?: unknown }
    | null
    | undefined;
  while (fiber != null) {
    if (fiber.memoizedProps?.behavior !== undefined) {
      return fiber.memoizedProps.behavior;
    }
    fiber = fiber.return as typeof fiber;
  }
  return undefined;
}

const companyNetwork = {
  id: 'company',
  name: '公司网络',
  cidrs: ['10.30.0.0/16'],
  vpnPools: ['10.99.0.0/16'],
};

function props(serverId?: string): RootScreenProps<'AddServer'> {
  return {
    navigation: mockNavigation<'AddServer'>(),
    route: mockRoute(
      'AddServer',
      serverId === undefined ? undefined : { serverId },
    ),
  } as RootScreenProps<'AddServer'>;
}

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('keeps the page still on iOS and insets only the form scroll', async () => {
  jest.replaceProperty(Platform, 'OS', 'ios');
  const view = await renderWithProviders(<AddServerScreen {...props()} />, {
    settings: { networks: [companyNetwork] },
  });
  expect(keyboardBehavior(view.getByTestId('add-server-form'))).toBeUndefined();
  const scroll = view.getByTestId('add-server-form-scroll');
  expect(scroll).toHaveProp('automaticallyAdjustKeyboardInsets', true);
  expect(scroll).toHaveProp('keyboardDismissMode', 'interactive');
  expect(scroll).toHaveProp('keyboardShouldPersistTaps', 'handled');
  expect(scroll).toContainElement(
    view.getByRole('header', { name: '手动添加服务器' }),
  );
  expect(scroll).toContainElement(view.getByRole('button', { name: '保存' }));
  expect(scroll).not.toContainElement(view.getByLabelText('返回'));
});

it('uses padding keyboard avoidance on android', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  const view = await renderWithProviders(<AddServerScreen {...props()} />, {
    settings: { networks: [companyNetwork] },
  });
  expect(keyboardBehavior(view.getByTestId('add-server-form'))).toBe('padding');
  expect(view.getByTestId('add-server-form-scroll')).not.toContainElement(
    view.getByLabelText('返回'),
  );
});

it('keeps the selected login method in read-only mode', async () => {
  const directory = sshServers.find(server => server.name === 'prod-api-01');
  if (directory === undefined) {
    throw new Error('directory fixture missing');
  }
  const view = await renderWithProviders(
    <AddServerScreen {...props(directory.serverId)} />,
    { settings: { networks: [companyNetwork] } },
  );
  await view.findByText('编辑服务器');
  const selected = view.getByTestId('add-server-auth-tailscale');
  await fireEvent.press(view.getByTestId('add-server-auth-password'));
  expect(selected).toHaveProp('accessibilityState', { selected: true });
  expect(view.getByTestId('add-server-auth-password')).toHaveProp(
    'accessibilityState',
    { selected: false },
  );
});

it('hides the form from TalkBack while the network picker is open', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  const view = await renderWithProviders(<AddServerScreen {...props()} />, {
    settings: { networks: [companyNetwork] },
  });
  await fireEvent.press(
    await view.findByLabelText('需要的网络：不需要，点按更改'),
  );
  const mainScroll = view.root
    ?.queryAll(node => node.props.importantForAccessibility !== undefined)
    .find(node => node.props.importantForAccessibility !== undefined);
  expect(mainScroll).toBeDefined();
  expect(mainScroll).toHaveProp(
    'importantForAccessibility',
    'no-hide-descendants',
  );
  await fireEvent.press(view.getByTestId('network-picker-close'));
  expect(mainScroll).toHaveProp('importantForAccessibility', 'auto');
});

it('labels the current network as selected', async () => {
  const view = await render(
    <NetworkPicker
      current="company"
      networks={[companyNetwork]}
      onClose={() => {}}
      onSelect={() => {}}
      visible
    />,
  );
  expect(view.getByLabelText('公司网络，已选')).toBeOnTheScreen();
});

it('announces completed connection tests on iOS', async () => {
  jest.replaceProperty(Platform, 'OS', 'ios');
  const announce = jest
    .spyOn(AccessibilityInfo, 'announceForAccessibility')
    .mockImplementation(() => {});
  const idle: TestState = { kind: 'idle' };
  const view = await render(
    <TestResult idleHint="点「测试连接」看看能不能连上" state={idle} />,
  );
  const ok: TestState = { kind: 'ok', ms: 31, via: null };
  await act(async () => {
    view.rerender(<TestResult idleHint="提示" state={ok} />);
  });
  expect(announce).toHaveBeenCalledWith(
    '端口能连上 · 31 ms，只测了端口。用户名和登录方式第一次连接时才验证，届时还会让你核对主机指纹',
  );
});

it('stacks the host fields at a large font scale', async () => {
  jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
  const view = await renderWithProviders(<AddServerScreen {...props()} />, {
    core: createFakeCore(),
    settings: { networks: [companyNetwork] },
  });
  expect(view.getByLabelText('地址').parent?.parent?.parent).toHaveStyle({
    flexDirection: 'column',
    alignItems: 'stretch',
  });
  expect(view.getByLabelText('端口').parent?.parent).toHaveStyle({
    width: undefined,
  });
});
