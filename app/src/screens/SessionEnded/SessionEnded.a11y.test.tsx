import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from '@testing-library/react-native';
import {
  AccessibilityInfo,
  PixelRatio,
  Platform,
  StyleSheet,
} from 'react-native';
import AppText from '../../components/AppText';
import { hit } from '../../theme/tokens';
import { sessionEnded } from '../../i18n/zh/sessionEnded';
import { errBoards3 } from '../../i18n/zh/errBoards3';
import ErrorFrame from './ErrorFrame';
import VersionBoard from './boards/VersionBoard';
import VpnBoard from './boards/VpnBoard';
import type { EndActions } from './boardTypes';
import { FakePlatform } from '../../native/platform/fakePlatform';
import { computers } from '../../native/rscore/fixtures';
import { renderWithProviders } from '../../test/render';

function actions(): EndActions & Record<keyof EndActions, jest.Mock> {
  return {
    retry: jest.fn(),
    openSession: jest.fn(),
    back: jest.fn(),
    openTerminal: jest.fn(),
    openScreenSharing: jest.fn(async () => undefined),
    copyText: jest.fn(),
    copyCode: jest.fn(),
    pair: jest.fn(),
    openDiagnostics: jest.fn(),
    toast: jest.fn(),
  } as never;
}

function frame(overrides: Partial<React.ComponentProps<typeof ErrorFrame>> = {}) {
  return (
    <ErrorFrame
      code={101}
      desc="说明文字"
      illustration={<AppText>示意图</AppText>}
      kicker="错误"
      onCopyCode={jest.fn()}
      title="标题"
      {...overrides}
    />
  );
}

function host(hostId: string) {
  const found = computers.find(item => item.hostId === hostId);
  if (found === undefined) {
    throw new Error(`fixture missing: ${hostId}`);
  }
  return found;
}

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
});

describe('ErrorFrame accessibility', () => {
  it('joins an illustration summary into the description label', async () => {
    await render(frame({ illustrationSummary: '最后在线：刚刚' }));
    expect(screen.getByText('说明文字').props.accessibilityLabel).toBe(
      sessionEnded.a11y.withSummary('说明文字', '最后在线：刚刚'),
    );
  });

  it('uses a single visible text column at the largest font size', async () => {
    jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
    await render(frame({ illustrationSummary: '示意图摘要' }));
    expect(screen.queryByTestId('session-ended-illustration')).toBeNull();
    expect(screen.getByText('示意图摘要')).toBeOnTheScreen();
    expect(screen.getByText('说明文字').props.accessibilityLabel).toBeUndefined();
  });

  it('leaves the description unlabeled when there is no summary', async () => {
    await render(frame());
    expect(screen.getByText('说明文字').props.accessibilityLabel).toBeUndefined();
  });

  it('keeps the error code button at the platform minimum hit height', async () => {
    await render(frame());
    const button = screen.getByRole('button', { name: '复制错误码 RS-E101' });
    expect(StyleSheet.flatten(button.props.style)).toEqual(
      expect.objectContaining({ minHeight: hit.min }),
    );

    await cleanup();
    jest.replaceProperty(Platform, 'OS', 'android');
    await render(frame());
    const androidButton = screen.getByRole('button', {
      name: '复制错误码 RS-E101',
    });
    expect(StyleSheet.flatten(androidButton.props.style)).toEqual(
      expect.objectContaining({ minHeight: hit.min }),
    );
  });

  it('announces titles only on iOS and uses a live region on Android', async () => {
    const announce = jest.spyOn(AccessibilityInfo, 'announceForAccessibility');
    await render(frame({ title: 'iOS 标题' }));
    expect(announce).toHaveBeenCalledWith('iOS 标题');

    announce.mockClear();
    jest.replaceProperty(Platform, 'OS', 'android');
    await render(frame({ title: 'Android 标题' }));
    expect(announce).not.toHaveBeenCalled();
    expect(screen.getByText('Android 标题').props.accessibilityLiveRegion).toBe(
      'polite',
    );
  });
});

describe('SessionEnded board summaries', () => {
  it('exposes Android VPN details and the illustration note', async () => {
    jest.replaceProperty(Platform, 'OS', 'android');
    const boardHost = host('w3k8p2x7m4q9tb6c');
    const boardActions = actions();
    await renderWithProviders(
      <VpnBoard
        actions={boardActions}
        code={160}
        host={boardHost}
        hostName={boardHost.name}
        os={boardHost.os}
        params={{ hostId: boardHost.hostId, code: 160 }}
      />,
      { settings: { 'tailnet.enabled': true } },
    );
    expect(screen.getByText(/Android 同时只能开一个 VPN/)).toBeOnTheScreen();
    expect(screen.getByText(/Android 同时只能开一个 VPN/).props.accessibilityLabel).toContain(
      'Tailscale 100.64.0.12',
    );
  });

  it('uses R9F Android update URLs and reports a failure', async () => {
    jest.replaceProperty(Platform, 'OS', 'android');
    const platform = new FakePlatform();
    platform.openUrlResult = false;
    const boardHost = host('h7v2m9q3k8x4tb1a');
    const boardActions = actions();
    await renderWithProviders(
      <VersionBoard
        actions={boardActions}
        code={171}
        host={boardHost}
        hostName={boardHost.name}
        os={boardHost.os}
        params={{ hostId: boardHost.hostId, code: 171 }}
      />,
      { platform },
    );
    await fireEvent.press(screen.getByRole('button', { name: '去更新 App' }));
    await act(async () => {
      await Promise.resolve();
    });
    expect(platform.opened[0]).toBe('market://details?id=ai.gxlself.remotescreen');
    expect(boardActions.toast).toHaveBeenCalledWith(errBoards3.version.updateUnavailable);
  });
});
