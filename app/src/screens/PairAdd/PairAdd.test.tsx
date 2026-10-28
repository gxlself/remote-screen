import { act, cleanup, fireEvent, screen } from '@testing-library/react-native';
import PairAddScreen from '.';
import type { RootScreenProps } from '../../app/navigation/routes';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { pairQrStudio } from '../../native/rscore/fixtures';
import { FakePlatform } from '../../native/platform/fakePlatform';
import { MemoryKv } from '../../features/storage/kv';
import { leaveHubUrl } from '../../test/localDirectory';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';

const LAB_HOST_ID = 'l4b8n2m6q1w5ec3r';
/** Studio Mac mini in the fixture directory, tailnet address 100.64.0.7. */
const STUDIO_HOST_ID = 'h7v2m9q3k8x4tb1a';

type RenderOptions = {
  core?: ReturnType<typeof createFakeCore>;
  /** An older version's Hub address still in MMKV (decision 9). */
  leftoverHubUrl?: boolean;
  platform?: FakePlatform;
};

function props(params?: RootScreenProps<'PairAdd'>['route']['params']) {
  return {
    navigation: mockNavigation<'PairAdd'>(),
    route: mockRoute('PairAdd', params),
  } as RootScreenProps<'PairAdd'>;
}

async function renderPairAdd(
  params?: RootScreenProps<'PairAdd'>['route']['params'],
  options?: RenderOptions,
) {
  const screenProps = props(params);
  const kv = new MemoryKv();
  if (options?.leftoverHubUrl === true) {
    leaveHubUrl(kv);
  }
  const view = await renderWithProviders(<PairAddScreen {...screenProps} />, {
    core: options?.core,
    platform: options?.platform,
    kv,
    localDirectory: {},
  });
  await act(async () => {
    await Promise.resolve();
  });
  return { ...view, ...screenProps };
}

async function openCodeTab() {
  await fireEvent.press(screen.getByLabelText('输入配对码'));
}

afterEach(async () => {
  await cleanup();
  jest.useRealTimers();
});

describe('PairAdd', () => {
  beforeEach(() => {
    jest.useFakeTimers();
  });

  async function renderScreen(
    params?: RootScreenProps<'PairAdd'>['route']['params'],
    options?: RenderOptions,
  ) {
    const rendered = await renderPairAdd(params, options);
    await act(async () => {
      await jest.advanceTimersByTimeAsync(1);
    });
    return rendered;
  }

  it('opens on scan and shows the viewfinder guidance', async () => {
    await renderScreen();
    expect(screen.getByLabelText('扫码配对')).toBeSelected();
    expect(screen.getByText('对准电脑屏幕上的配对二维码')).toBeOnTheScreen();
    expect(
      screen.getByText(
        '在电脑菜单栏或托盘的 Host 里点「显示配对二维码」，再对准屏幕上的二维码。',
      ),
    ).toBeOnTheScreen();
  });

  it('shows code guidance and all six code boxes after switching tabs', async () => {
    await renderScreen();
    await openCodeTab();
    expect(
      screen.getByText(
        '在电脑菜单栏或托盘的 Host 里点「显示配对二维码」，窗口里有 6 位配对码。',
      ),
    ).toBeOnTheScreen();
    for (let position = 1; position <= 6; position += 1) {
      expect(
        screen.getByLabelText(`配对码第 ${position} 位`),
      ).toBeOnTheScreen();
    }
  });

  it('passes a valid QR scan to PairWaiting', async () => {
    const { navigation } = await renderScreen();
    await fireEvent(screen.getByTestId('scanner-slot'), 'onScan', pairQrStudio);
    expect(navigation.replace).toHaveBeenCalledWith('PairWaiting', {
      qrText: pairQrStudio,
    });
  });

  it('rejects an invalid scan with a toast and no navigation', async () => {
    const { navigation } = await renderScreen();
    await fireEvent(screen.getByTestId('scanner-slot'), 'onScan', 'hello');
    expect(screen.getByText('这不是 Remote Screen 的配对码')).toBeOnTheScreen();
    expect(navigation.replace).not.toHaveBeenCalled();
  });

  it('ignores the same scan twice within two seconds', async () => {
    const { navigation } = await renderScreen();
    const slot = screen.getByTestId('scanner-slot');
    await fireEvent(slot, 'onScan', pairQrStudio);
    await fireEvent(slot, 'onScan', pairQrStudio);
    expect(navigation.replace).toHaveBeenCalledTimes(1);
  });

  it('sanitizes pasted code and requires a computer address', async () => {
    await renderScreen();
    await openCodeTab();
    await fireEvent.changeText(screen.getByTestId('pair-code-input'), '48 29');
    expect(screen.getByTestId('pair-code-box-1')).toHaveTextContent('4');
    expect(screen.getByTestId('pair-code-box-2')).toHaveTextContent('8');
    expect(screen.getByTestId('pair-code-box-3')).toHaveTextContent('2');
    expect(screen.getByTestId('pair-code-box-4')).toHaveTextContent('9');
    expect(screen.getByLabelText('电脑地址')).toHaveDisplayValue('');
    expect(screen.getByRole('button', { name: '配对' })).toBeDisabled();
  });

  it('opens the code tab for an older link that names a computer', async () => {
    const { navigation, core } = await renderScreen({ hostId: LAB_HOST_ID });
    expect(screen.getByLabelText('输入配对码')).toBeSelected();
    expect(screen.getByLabelText('电脑地址')).toHaveDisplayValue('');
    expect(screen.queryByText('tailnet 里待配对的电脑')).toBeNull();
    await fireEvent.changeText(screen.getByTestId('pair-code-input'), '482913');
    expect(navigation.replace).not.toHaveBeenCalled();
    expect(core.calls.some(call => call.method.startsWith('hub.'))).toBe(
      false,
    );
  });

  it('navigates to the alternate add flows and closes', async () => {
    const { navigation } = await renderScreen();
    // popTo goes back to an open page instead of stacking another copy.
    await fireEvent.press(screen.getByRole('button', { name: '按地址添加' }));
    expect(navigation.popTo).toHaveBeenCalledWith('AddByAddress');
    await fireEvent.press(
      screen.getByRole('button', { name: '添加 SSH 服务器' }),
    );
    expect(navigation.popTo).toHaveBeenCalledWith('AddServer');
    expect(navigation.navigate).not.toHaveBeenCalled();
    expect(screen.queryByText(/下载地址/)).toBeNull();
    await fireEvent.press(screen.getByLabelText('关闭'));
    expect(navigation.goBack).toHaveBeenCalledTimes(1);
  });

  it('fills in the address of the computer it was opened for', async () => {
    await renderScreen({ hostId: STUDIO_HOST_ID });
    expect(screen.getByLabelText('输入配对码')).toBeSelected();
    expect(screen.getByLabelText('电脑地址')).toHaveDisplayValue('100.64.0.7');
  });

  it('warns once for the same wrong QR and buzzes on a good one', async () => {
    const platform = new FakePlatform();
    const { navigation } = await renderScreen(undefined, { platform });
    const slot = screen.getByTestId('scanner-slot');
    await fireEvent(slot, 'onScan', 'hello');
    expect(screen.getByText('这不是 Remote Screen 的配对码')).toBeOnTheScreen();
    await act(async () => {
      await jest.advanceTimersByTimeAsync(2500);
    });
    await fireEvent(slot, 'onScan', 'hello');
    expect(
      platform.calls.filter(
        call => call.method === 'haptic' && call.args[0] === 'error',
      ),
    ).toHaveLength(1);
    await fireEvent(slot, 'onScan', pairQrStudio);
    expect(platform.calls).toContainEqual({
      method: 'haptic',
      args: ['success'],
    });
    expect(navigation.replace).toHaveBeenCalledTimes(1);
  });

  it('turns the camera off while another page covers this one', async () => {
    const { navigation } = await renderScreen();
    const viewfinder = () => screen.getByTestId('scanner-slot-viewfinder');
    expect(viewfinder()).not.toHaveStyle({ opacity: 0.45 });
    const addListener = navigation.addListener as unknown as jest.Mock;
    const blur = addListener.mock.calls.find(([name]) => name === 'blur')?.[1];
    await act(async () => blur());
    expect(viewfinder()).toHaveStyle({ opacity: 0.45 });
  });

  it('imports a pairing file from every tab', async () => {
    const { navigation } = await renderScreen();
    expect(
      screen.getByRole('button', { name: '导入配对文件' }),
    ).toBeOnTheScreen();
    await fireEvent.press(screen.getByRole('button', { name: '导入配对文件' }));
    expect(navigation.navigate).toHaveBeenCalledWith('PairFile');

    await openCodeTab();
    expect(
      screen.getByRole('button', { name: '导入配对文件' }),
    ).toBeOnTheScreen();
  });

  describe('by address', () => {
    async function typeAddress(text: string) {
      await fireEvent.changeText(screen.getByLabelText('电脑地址'), text);
    }

    async function typeCode(code: string) {
      await fireEvent.changeText(screen.getByTestId('pair-code-input'), code);
    }

    it.each([
      ['without a Hub', false],
      ['with a Hub address left in MMKV', true],
    ])('shows only the address form %s', async (_, leftoverHubUrl) => {
      const { core } = await renderScreen(
        { tab: 'code' },
        { leftoverHubUrl },
      );

      expect(screen.getByLabelText('电脑地址')).toBeOnTheScreen();
      expect(
        screen.getByPlaceholderText('100.64.0.3 或 home-mac'),
      ).toBeOnTheScreen();
      expect(screen.getByText('配对码')).toBeOnTheScreen();
      expect(
        screen.getByText(
          '在电脑菜单栏或托盘的 Host 里点「显示配对二维码」，窗口里有 6 位配对码。',
        ),
      ).toBeOnTheScreen();
      expect(screen.getByRole('button', { name: '配对' })).toBeDisabled();
      expect(
        screen.queryByText('tailnet 里待配对的电脑'),
      ).not.toBeOnTheScreen();
      expect(
        screen.queryByRole('button', { name: '用这台电脑的登录密码' }),
      ).not.toBeOnTheScreen();
      expect(screen.queryByText('让已配对的设备批准')).not.toBeOnTheScreen();
      expect(
        screen.getByRole('button', { name: '导入配对文件' }),
      ).toBeOnTheScreen();
      expect(core.calls.some(call => call.method.startsWith('hub.'))).toBe(
        false,
      );
    });

    it('prefills the address from the route', async () => {
      const first = await renderScreen({ addr: '100.64.0.3', port: 7400 });
      expect(screen.getByLabelText('输入配对码')).toBeSelected();
      expect(screen.getByLabelText('电脑地址')).toHaveDisplayValue(
        '100.64.0.3:7400',
      );
      await first.unmount();

      await renderScreen({ tab: 'code', addr: 'fd7a::1', port: 7400 });
      expect(screen.getByLabelText('电脑地址')).toHaveDisplayValue(
        '[fd7a::1]:7400',
      );
    });

    it('submits a host name with a port', async () => {
      const { navigation } = await renderScreen({ tab: 'code' });
      await typeAddress('home-mac:7400');
      await typeCode('482913');
      expect(navigation.replace).toHaveBeenCalledWith('PairWaiting', {
        addr: 'home-mac',
        port: 7400,
        code: '482913',
      });
    });

    it('submits a bracketed IPv6 address with a port', async () => {
      const { navigation } = await renderScreen({ tab: 'code' });
      await typeAddress('[fd7a::1]:7400');
      await typeCode('482913');
      expect(navigation.replace).toHaveBeenCalledWith('PairWaiting', {
        addr: 'fd7a::1',
        port: 7400,
        code: '482913',
      });
    });

    it('trims spaces and leaves the port to the default', async () => {
      const { navigation } = await renderScreen({ tab: 'code' });
      await typeAddress('  100.64.0.3 ');
      await typeCode('482913');
      expect(navigation.replace).toHaveBeenCalledWith('PairWaiting', {
        addr: '100.64.0.3',
        code: '482913',
      });
      const params = (navigation.replace as jest.Mock).mock.calls[0][1];
      expect(params).not.toHaveProperty('port');
    });

    it('keeps the button disabled for an empty or bad address or a short code', async () => {
      const { navigation } = await renderScreen({ tab: 'code' });
      const button = () => screen.getByRole('button', { name: '配对' });

      await typeCode('482913');
      expect(button()).toBeDisabled();

      await typeAddress('1.2.3.256');
      expect(button()).toBeDisabled();
      await typeAddress('home-mac:70000');
      expect(button()).toBeDisabled();

      await typeAddress('home-mac');
      await typeCode('4829');
      expect(button()).toBeDisabled();
      expect(navigation.replace).not.toHaveBeenCalled();
    });

    it('submits from the button only once for repeated taps', async () => {
      const { navigation } = await renderScreen({ tab: 'code' });
      await typeCode('482913');
      await typeAddress('100.64.0.3');
      const button = screen.getByRole('button', { name: '配对' });
      expect(button).toBeEnabled();
      await fireEvent.press(button);
      await fireEvent.press(button);
      expect(navigation.replace).toHaveBeenCalledTimes(1);
      expect(navigation.replace).toHaveBeenCalledWith('PairWaiting', {
        addr: '100.64.0.3',
        code: '482913',
      });
    });
  });
});
