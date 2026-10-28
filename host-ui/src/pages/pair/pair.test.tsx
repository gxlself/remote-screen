import { act, fireEvent, render, screen } from '@testing-library/react';
import * as QRCode from 'qrcode';
import { StrictMode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { PAIR_STATUS } from '../../ipc/events';
import { FakeIpc } from '../../ipc/fake';
import { flush, renderPage } from '../../ipc/testing';
import { FakePlatform } from '../../platform';

import PairPage from '.';
import { QrCode } from './QrCode';

const NOW = new Date('2026-01-02T03:04:05.000Z').getTime();
const HOST_ID = 'h7v2m9q3k8x4tb1a';
const QR_VALUE = 'rsp://pair?v=1&h=h7v2m9q3k8x4tb1a&fp=abc&c=482913&n=studio-mac&a=100.64.0.7';

interface RenderPairOptions {
  ipc?: FakeIpc;
  platform?: FakePlatform;
  strict?: boolean;
}

function newIpc(): FakeIpc {
  return new FakeIpc(undefined, { now: () => Date.now() });
}

function renderPair(options: RenderPairOptions = {}) {
  const ipc = options.ipc ?? newIpc();
  const platform = options.platform ?? new FakePlatform();
  const page = <PairPage route={{ page: 'pair', query: {} }} />;

  return renderPage(options.strict === true ? <StrictMode>{page}</StrictMode> : page, {
    ipc,
    platform,
  });
}

async function advance(ms: number): Promise<void> {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
  await flush();
}

function emitPairing(ipc: FakeIpc, overrides: Record<string, unknown> = {}): void {
  ipc.emit('event.pairingResult', {
    status: PAIR_STATUS.BAD_CODE,
    host_id: HOST_ID,
    host_name: 'studio-mac',
    attempts_left: 3,
    device_fp: '',
    ...overrides,
  });
}

describe('QrCode', () => {
  it('is deterministic for the same value and varies by value', () => {
    const { container, rerender } = render(<QrCode size={200} value={QR_VALUE} />);
    const first = container.querySelector('path')?.getAttribute('d');
    expect(first).toBeTruthy();

    rerender(<QrCode size={200} value={QR_VALUE} />);
    expect(container.querySelector('path')?.getAttribute('d')).toBe(first);

    rerender(<QrCode size={200} value={`${QR_VALUE}2`} />);
    expect(container.querySelector('path')?.getAttribute('d')).not.toBe(first);
  });

  it('matches the module matrix size in viewBox and draws one path', () => {
    const { container } = render(<QrCode size={200} value={QR_VALUE} />);
    const svg = container.querySelector('svg');
    const modules = QRCode.create(QR_VALUE, { errorCorrectionLevel: 'M' }).modules.size;

    expect(svg).toHaveAttribute('viewBox', `0 0 ${modules} ${modules}`);
    expect(svg).toHaveAttribute('shape-rendering', 'crispEdges');
    expect(svg).toHaveAttribute('role', 'img');
    expect(svg).toHaveAttribute('aria-label', '配对二维码');
    expect(svg?.querySelectorAll('path')).toHaveLength(1);
  });
});

describe('PairPage', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(NOW);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('requests one code under StrictMode and renders both columns', async () => {
    const { ipc } = renderPair({ strict: true });
    await flush();

    expect(ipc.calls.filter((call) => call.method === 'pairing.start')).toHaveLength(1);
    expect(screen.getByText('482 913')).toBeInTheDocument();
    expect(screen.getByText('studio-mac · 100.64.0.7')).toBeInTheDocument();
    expect(screen.getByText('FWX9 3G00 0000 0000')).toBeInTheDocument(); // D39 统一指纹显示
    expect(screen.getByText('5:00').parentElement).toHaveTextContent('5:00 后失效 · 只能用一次');

    const qr = screen.getByRole('img', { name: '配对二维码' });
    const path = qr.querySelector('path');
    expect(path).not.toBeNull();
    expect(path?.getAttribute('d')).toBeTruthy();
    expect(screen.getByLabelText('4 8 2 9 1 3')).toHaveTextContent('482 913');
  });

  it('counts down and expires without automatically replacing the code', async () => {
    const { ipc } = renderPair();
    await flush();

    await advance(23_000);
    expect(screen.getByText('4:37').parentElement).toHaveTextContent('4:37 后失效 · 只能用一次');

    await advance(277_000);
    expect(screen.getByText('配对码已过期')).toBeInTheDocument();
    expect(screen.getByText('已过期')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '换一个码' })).toBeInTheDocument();
    expect(ipc.calls.filter((call) => call.method === 'pairing.start')).toHaveLength(1);
  });

  it('requests a new code when the primary action changed after expiry', async () => {
    const { ipc, platform, user } = renderPair();
    await flush();

    await advance(300_000);
    await user.click(screen.getByRole('button', { name: '换一个码' }));
    await flush();

    expect(ipc.calls.map((call) => call.method).slice(-2)).toEqual([
      'pairing.cancel',
      'pairing.start',
    ]);
    expect(screen.getByText('173 064')).toBeInTheDocument();
    expect(platform.calls).toEqual([]);
  });

  it('cancels the previous code before requesting a fresh one', async () => {
    const { ipc, platform, user } = renderPair();
    await flush();

    await user.click(screen.getByRole('button', { name: '换一个码' }));
    await flush();

    const methods = ipc.calls.map((call) => call.method);
    expect(methods.slice(-2)).toEqual(['pairing.cancel', 'pairing.start']);
    expect(ipc.calls.at(-2)?.params).toEqual({ code: '482913' });
    expect(screen.getByText('173 064')).toBeInTheDocument();
    expect(screen.getByText('5:00')).toBeInTheDocument();
    expect(platform.calls).toEqual([]);
  });

  it('continues to start when the old code cannot be cancelled', async () => {
    const ipc = newIpc();
    ipc.failNext('pairing.cancel', { code: -32000, message: 'gone' });
    const { user } = renderPair({ ipc });
    await flush();

    await user.click(screen.getByRole('button', { name: '换一个码' }));
    await flush();

    expect(ipc.calls.map((call) => call.method).slice(-2)).toEqual([
      'pairing.cancel',
      'pairing.start',
    ]);
    expect(screen.getByText('173 064')).toBeInTheDocument();
  });

  it('cancels the current code before hiding the window', async () => {
    const { ipc, platform, user } = renderPair();
    await flush();

    await user.click(screen.getByRole('button', { name: '完成' }));
    await flush();

    expect(ipc.calls).toContainEqual({ method: 'pairing.cancel', params: { code: '482913' } });
    expect(platform.calls).toContainEqual({ op: 'hideWindow', args: ['pair'] });
  });

  it('ignores results for another host and renders bad-code progress', async () => {
    const { ipc } = renderPair();
    await flush();

    emitPairing(ipc, { host_id: 'other' });
    await flush();
    expect(screen.queryByText('配对码不对，还能再试 3 次')).not.toBeInTheDocument();

    emitPairing(ipc);
    await flush();
    expect(screen.getByText('配对码不对，还能再试 3 次')).toBeInTheDocument();
  });

  it('renders waiting, success, denied, locked and disabled-on-path results', async () => {
    const { ipc, platform, user } = renderPair();
    await flush();

    emitPairing(ipc, { status: PAIR_STATUS.WAITING_LOCAL });
    await flush();
    expect(screen.getByText('手机已输入正确的码，请在弹出的窗口里确认')).toBeInTheDocument();

    emitPairing(ipc, { status: PAIR_STATUS.OK });
    await flush();
    expect(screen.getByText('配对成功')).toBeInTheDocument();
    expect(screen.getByText('可以在手机上开始连接了。')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '完成' })).toBeInTheDocument();

    await advance(300_000);
    expect(screen.getByRole('button', { name: '完成' })).toBeInTheDocument();
    expect(screen.queryByText('已过期')).not.toBeInTheDocument();

    emitPairing(ipc, { status: PAIR_STATUS.DENIED });
    await flush();
    expect(screen.getByText('你拒绝了这次配对')).toBeInTheDocument();

    emitPairing(ipc, { status: PAIR_STATUS.LOCKED, attempts_left: 0 });
    await flush();
    expect(screen.getByText('配对码错太多次，已失效')).toBeInTheDocument();
    expect(screen.getByText('已失效')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '换一个码' })).toBeInTheDocument();

    emitPairing(ipc, { status: PAIR_STATUS.DISABLED_ON_PATH });
    await flush();
    await user.click(screen.getByRole('button', { name: '网络与访问…' }));
    expect(platform.calls).toContainEqual({
      op: 'showWindow',
      args: ['settings', 'settings/network'],
    });
  });

  it('shows the start error and retries', async () => {
    const ipc = newIpc();
    ipc.failNext('pairing.start', { code: -32000, message: 'busy', data: { code: 'busy' } });
    const { user } = renderPair({ ipc });
    await flush();

    expect(screen.getByRole('alert')).toHaveTextContent('没拿到配对码：busy');
    await user.click(screen.getByRole('button', { name: '重试' }));
    await flush();

    expect(screen.getByText('482 913')).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('still shows the QR and code when the pairing URL cannot be parsed', async () => {
    class InvalidPairUrlIpc extends FakeIpc {
      override call(method: string, params: unknown): Promise<unknown> {
        if (method === 'pairing.start') {
          return Promise.resolve({
            qr_url: 'not-a-pair-url',
            code: '482913',
            expires_at: (Date.now() + 300_000) * 1000,
          });
        }
        return super.call(method, params);
      }
    }

    renderPair({ ipc: new InvalidPairUrlIpc(undefined, { now: () => Date.now() }) });
    await flush();

    expect(screen.getByText('482 913')).toBeInTheDocument();
    expect(screen.getByRole('img', { name: '配对二维码' })).toBeInTheDocument();
    expect(screen.queryByText('本机')).not.toBeInTheDocument();
    expect(screen.queryByText('证书指纹')).not.toBeInTheDocument();
  });

  it('cancels the latest code when unmounted', async () => {
    const { ipc, unmount } = renderPair();
    await flush();

    unmount();
    expect(ipc.calls.at(-1)).toEqual({ method: 'pairing.cancel', params: { code: '482913' } });
  });

  it('cancels the current code on pagehide', async () => {
    const { ipc } = renderPair();
    await flush();
    ipc.calls.length = 0;

    fireEvent(window, new Event('pagehide'));

    expect(ipc.calls).toEqual([{ method: 'pairing.cancel', params: { code: '482913' } }]);
  });
});
