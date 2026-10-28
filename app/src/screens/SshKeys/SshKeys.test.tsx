import { act, fireEvent, screen } from '@testing-library/react-native';
import type { ReactElement } from 'react';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import { FakePlatform } from '../../native/platform/fakePlatform';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { FakeCore } from '../../native/rscore/fakeCore';
import { sshKeys as fixtureKeys } from '../../native/rscore/fixtures';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import { tokens } from '../../theme/tokens';
import SshKeysScreen from './index';

function element(): ReactElement {
  return (
    <SshKeysScreen
      navigation={mockNavigation<'SshKeys'>()}
      route={mockRoute('SshKeys')}
    />
  );
}

async function settle(ms = 1) {
  await act(async () => {
    await jest.advanceTimersByTimeAsync(ms);
  });
}

async function renderScreen(core: FakeCore, tailnetEnabled = true) {
  // Tailscale SSH rows and hints need 外网访问 (Tailscale) on.
  const rendered = await renderWithProviders(element(), {
    core,
    settings: { 'tailnet.enabled': tailnetEnabled },
  });
  await settle();
  return rendered;
}

async function press(label: string) {
  await fireEvent.press(screen.getByLabelText(label));
  await settle();
}

function calls(core: FakeCore, method: string) {
  return core.calls.filter(call => call.method === method);
}

beforeEach(() => {
  jest.useFakeTimers();
});

afterEach(() => {
  jest.useRealTimers();
});

it('renders the local key, public-key actions and install rows', async () => {
  const core = createFakeCore();
  const rendered = await renderScreen(core);

  expect(screen.getByText('rs-iphone-se')).toBeOnTheScreen();
  expect(screen.getByText('已装 2 台')).toBeOnTheScreen();
  expect(screen.getByText('ECDSA P-256 · 9月2日 生成')).toBeOnTheScreen();
  expect(
    screen.getByText('私钥在安全芯片里，导不出来，也不会上传。'),
  ).toBeOnTheScreen();

  expect(screen.getAllByLabelText('已安装')).toHaveLength(2);
  expect(screen.getByLabelText('把公钥装到 build-srv')).toBeOnTheScreen();
  expect(screen.queryByTestId('install-row-prod-api-01')).toBeNull();
  expect(
    screen.getByText('走 Tailscale SSH 的机器（如 prod-api-01）不用装密钥'),
  ).toBeOnTheScreen();

  await press('复制公钥');
  expect(rendered.platform.copied[0]).toBe(fixtureKeys[0]!.publicKey);
  expect(screen.getByText('已复制')).toBeOnTheScreen();
  await settle(2000);
  expect(screen.getByText('复制公钥')).toBeOnTheScreen();

  await press('分享');
  expect(rendered.platform.calls).toContainEqual({
    method: 'share',
    args: [
      {
        text: fixtureKeys[0]!.publicKey,
        title: 'rs-iphone-se',
      },
    ],
  });
});

it('installs a public key without a maintenance key', async () => {
  const core = createFakeCore();
  await renderScreen(core);

  await press('把公钥装到 build-srv');
  expect(screen.getByText('把公钥装到 build-srv？')).toBeOnTheScreen();
  expect(screen.queryByTestId('maint-key-switch')).toBeNull();
  expect(screen.queryByText(/维护密钥/)).toBeNull();

  await press('安装');
  const installs = calls(core, 'ssh.installKey');
  expect(installs).toHaveLength(1);
  expect(installs[0]!.args).toEqual(['manual-build-srv', 'rs-iphone-se']);
  expect(screen.getByText('已装好')).toBeOnTheScreen();
  expect(screen.getByText('刚装好，下次直接用密钥登录')).toBeOnTheScreen();
});

it('shows no certificate card and never asks for certificates', async () => {
  const core = createFakeCore();
  await renderScreen(core);

  expect(screen.getByText('rs-iphone-se')).toBeOnTheScreen();
  expect(screen.queryByText(/证书/)).toBeNull();
  expect(screen.queryByTestId(/^cert-row-/)).toBeNull();
  expect(
    core.calls.filter(call =>
      ['ssh.certStatus', 'ssh.renewCert', 'ssh.trustCa'].includes(call.method),
    ),
  ).toEqual([]);
});

it('reports an install failure', async () => {
  const core = createFakeCore();
  core.failNext('ssh.installKey', new Error('auth failed'));
  await renderScreen(core);

  await press('把公钥装到 build-srv');
  await press('安装');
  expect(screen.getByText('没装上：auth failed')).toBeOnTheScreen();
});

it('shows Tailscale hosts and deletes a trusted fingerprint', async () => {
  const core = createFakeCore();
  await renderScreen(core);

  expect(screen.getByText('Tailscale 认证')).toBeOnTheScreen();
  expect(screen.getByText('Tailscale SSH（不用指纹）')).toBeOnTheScreen();
  expect(
    screen.getByLabelText('删除 gpu-train-02 的主机指纹'),
  ).toBeOnTheScreen();

  await press('删除 gpu-train-02 的主机指纹');
  expect(screen.getByText('删除 gpu-train-02 的主机指纹？')).toBeOnTheScreen();
  await press('删除');

  expect(calls(core, 'ssh.forgetHostKey')).toContainEqual({
    method: 'ssh.forgetHostKey',
    args: ['gpu-train-02'],
  });
  expect(screen.queryByTestId('trusted-row-gpu-train-02')).toBeNull();
  // T31: the delete says it is done.
  expect(screen.getByText('已删除主机指纹')).toBeOnTheScreen();
});

it('shows no Tailscale SSH rows or hints while Tailscale is switched off', async () => {
  await renderScreen(createFakeCore(), false);
  expect(screen.getByText('rs-iphone-se')).toBeOnTheScreen();
  expect(screen.queryByText('Tailscale 认证')).toBeNull();
  expect(screen.queryAllByText(/Tailscale/)).toHaveLength(0);
});

it('shows only Tailscale hosts when trusted host keys are unsupported', async () => {
  const core = createFakeCore();
  core.failNext('ssh.trustedHostKeys', new RsCoreError('unsupported', 'x'));
  await renderScreen(core);

  expect(screen.getByText('当前版本还看不到已信任的指纹')).toBeOnTheScreen();
  expect(screen.queryByTestId('trusted-row-gpu-train-02')).toBeNull();
  expect(screen.getByTestId('trusted-tailscale-prod-api-01')).toBeOnTheScreen();
});

it('keeps the row when forgetting a fingerprint is unsupported', async () => {
  const core = createFakeCore();
  core.failNext('ssh.forgetHostKey', new RsCoreError('unsupported', 'x'));
  await renderScreen(core);

  await press('删除 gpu-train-02 的主机指纹');
  await press('删除');

  expect(screen.getByText('当前版本还不能删除')).toBeOnTheScreen();
  expect(screen.getByTestId('trusted-row-gpu-train-02')).toBeOnTheScreen();
});

it('reports a key generation failure and keeps the empty state', async () => {
  const core = createFakeCore({ sshKeys: [] });
  core.failNext('ssh.createKey', new Error('no secure enclave'));
  await renderScreen(core);

  await press('生成密钥');

  expect(screen.getByText('没生成成功：no secure enclave')).toBeOnTheScreen();
  expect(screen.getByText('还没有 SSH 密钥')).toBeOnTheScreen();
});

it('creates the first key for a device without one', async () => {
  const core = createFakeCore({ sshKeys: [] });
  await renderScreen(core);

  expect(screen.getByText('还没有 SSH 密钥')).toBeOnTheScreen();
  await press('生成密钥');

  expect(calls(core, 'ssh.createKey')).toContainEqual({
    method: 'ssh.createKey',
    args: ['se-p256', 'rs-iphone'],
  });
  // A fresh install has no identity until something creates it.
  const order = core.calls.map(call => call.method);
  expect(order.indexOf('identity.ensure')).toBeGreaterThanOrEqual(0);
  expect(order.indexOf('identity.ensure')).toBeLessThan(
    order.indexOf('ssh.createKey'),
  );
  expect(screen.getByText('rs-iphone')).toBeOnTheScreen();
  // T13
  expect(screen.getByText('密钥生成好了')).toBeOnTheScreen();
});

it('shows progress on the generate button while the key is made', async () => {
  const core = createFakeCore({ sshKeys: [] });
  let finish: () => void = () => undefined;
  const create = core.ssh.createKey.bind(core.ssh);
  jest.spyOn(core.ssh, 'createKey').mockImplementation(
    (...args: Parameters<typeof core.ssh.createKey>) =>
      new Promise(resolve => {
        finish = () => resolve(create(...args));
      }),
  );
  await renderScreen(core);
  await fireEvent.press(screen.getByTestId('ssh-keys-generate'));
  expect(screen.getByTestId('ssh-keys-generate')).toBeBusy();
  await act(async () => finish());
  await settle();
  expect(screen.getByText('rs-iphone')).toBeOnTheScreen();
});

it('shows a read failure with retry instead of offering a new key', async () => {
  const core = createFakeCore();
  core.failNext('ssh.keys', new Error('keychain locked'));
  await renderScreen(core);

  // T14: a failed read never lands on the generate empty state.
  expect(screen.getByText('密钥没读出来')).toBeOnTheScreen();
  expect(screen.queryByText('还没有 SSH 密钥')).toBeNull();
  expect(screen.queryByTestId('ssh-keys-generate')).toBeNull();
  await fireEvent.press(screen.getByTestId('ssh-keys-retry'));
  await settle();
  expect(screen.getByTestId('key-card')).toBeOnTheScreen();
  expect(calls(core, 'ssh.createKey')).toEqual([]);
});

it('says so instead of drawing empty groups', async () => {
  const core = createFakeCore({ sshServers: [], trustedHostKeys: [] });
  await renderScreen(core);
  // T30
  expect(screen.getByText('还没有要装公钥的服务器')).toBeOnTheScreen();
  expect(screen.getByText('还没有信任过的主机')).toBeOnTheScreen();
  // T32: the group title and the dialog both speak of the existing session.
  expect(screen.getByText('通过已有会话安装公钥')).toBeOnTheScreen();
});

it('dims the back button while pressed', async () => {
  await renderScreen(createFakeCore());
  const back = screen.getByTestId('ssh-keys-back');
  // S23
  await fireEvent(back, 'responderGrant', {
    persist: () => {},
    nativeEvent: {},
    currentTarget: 1,
  });
  expect(back).toHaveStyle({ opacity: 0.6 });
});

it('says when the public key could not be copied', async () => {
  const platform = new FakePlatform();
  jest
    .spyOn(platform, 'copyText')
    .mockRejectedValue(new Error('clipboard busy'));
  await renderWithProviders(element(), { core: createFakeCore(), platform });
  await settle();
  await press('复制公钥');
  // T33
  expect(screen.getByText('没复制上，用「分享」试试')).toBeOnTheScreen();
  expect(screen.queryByText('已复制')).toBeNull();
});

it('keeps the install count neutral while no server has the key', async () => {
  const core = createFakeCore({
    sshKeys: fixtureKeys.map(key => ({ ...key, installedOn: [] })),
  });
  await renderScreen(core);
  // T33: 「已装 0 台」 is not a success.
  expect(screen.getByText('已装 0 台')).toBeOnTheScreen();
  expect(screen.getByText('已装 0 台')).not.toHaveStyle({ color: tokens.good });
});
