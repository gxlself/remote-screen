import { act, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { Route } from '../../app/router';
import type { SessionSummary } from '../../ipc';
import { FakeIpc } from '../../ipc/fake';
import { flush, renderPage } from '../../ipc/testing';

import CapsulePage from '.';

const route: Route = { page: 'capsule', query: {} };

function setRecording(ipc: FakeIpc, index: number, recording: boolean | undefined): void {
  const session = ipc.state.sessions[index];
  if (session === undefined) {
    throw new Error('fixture session is missing');
  }
  const mutableSession = session as SessionSummary & { recording?: boolean };
  if (recording === undefined) {
    delete mutableSession.recording;
  } else {
    mutableSession.recording = recording;
  }
}

afterEach(() => {
  vi.useRealTimers();
});

describe('CapsulePage', () => {
  it('shows the controller, live duration and a single disconnect button', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-10-21T12:00:00Z'));
    const ipc = new FakeIpc();
    renderPage(<CapsulePage route={route} />, { ipc });
    await flush();

    const status = screen.getByRole('status');
    expect(status).toHaveTextContent('小明的 iPhone 正在控制这台 Mac');
    expect(status).toHaveTextContent('42:18');
    expect(screen.getByRole('button', { name: '断开' })).toBeInTheDocument();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000);
    });

    expect(screen.getByRole('status')).toHaveTextContent('42:20');
  });

  it('disconnects the only session and renders the empty page afterwards', async () => {
    vi.useFakeTimers();
    const ipc = new FakeIpc();
    const { user } = renderPage(<CapsulePage route={route} />, { ipc });
    await flush();

    await user.click(screen.getByRole('button', { name: '断开' }));
    await flush();

    expect(ipc.calls).toContainEqual({
      method: 'sessions.disconnect',
      params: { session_id: 's-1' },
    });
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
  });

  it('shows the viewer chip and disconnects every session during handoff', async () => {
    vi.useFakeTimers();
    const ipc = FakeIpc.fromScenario('handoff');
    const { user } = renderPage(<CapsulePage route={route} />, { ipc });
    await flush();

    expect(screen.getByRole('status')).toHaveTextContent('小明的 iPad 在观看');
    await user.click(screen.getByRole('button', { name: '全部断开' }));
    await flush();

    expect(
      ipc.calls.filter((call) => call.method === 'sessions.disconnect').map((call) => call.params),
    ).toEqual([{ session_id: 's-1' }, { session_id: 's-2' }]);
  });

  it('describes one or multiple viewers when there is no controller', async () => {
    vi.useFakeTimers();
    const ipc = new FakeIpc();
    renderPage(<CapsulePage route={route} />, { ipc });
    await flush();

    const [firstSession] = ipc.state.sessions;
    if (firstSession === undefined) {
      throw new Error('fixture session is missing');
    }
    firstSession.role = 'viewer';
    act(() => ipc.emit('event.sessionsChanged', {}));
    await flush();
    expect(screen.getByRole('status')).toHaveTextContent('小明的 iPhone 正在观看这台 Mac');

    ipc.state.sessions.push({
      session_id: 's-2',
      device_id: 'viewer-device-2',
      device_name: '小明的 iPad',
      role: 'viewer',
      connected_at_us: firstSession.connected_at_us - 1000,
    });
    act(() => ipc.emit('event.sessionsChanged', {}));
    await flush();
    expect(screen.getByRole('status')).toHaveTextContent('2 台设备正在观看这台 Mac');
  });

  it('renders nothing while idle', async () => {
    const ipc = FakeIpc.fromScenario('idle');
    renderPage(<CapsulePage route={route} />, { ipc });
    await flush();

    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    expect(document.querySelector('main[data-page="capsule"]')).toBeEmptyDOMElement();
  });

  it('only marks the capsule as the window drag region', async () => {
    const ipc = new FakeIpc();
    renderPage(<CapsulePage route={route} />, { ipc });
    await flush();

    expect(screen.getByRole('status')).toHaveAttribute('data-tauri-drag-region');
    expect(screen.getByRole('button', { name: '断开' })).not.toHaveAttribute(
      'data-tauri-drag-region',
    );
  });

  it('keeps the button disabled until disconnect settles and recovers after a failure', async () => {
    vi.useFakeTimers();
    const ipc = new FakeIpc(undefined, {
      delay: (method) => (method === 'sessions.disconnect' ? 100 : 0),
    });
    const { user } = renderPage(<CapsulePage route={route} />, { ipc });
    await flush();

    await user.click(screen.getByRole('button', { name: '断开' }));
    expect(screen.getByRole('button', { name: '断开' })).toBeDisabled();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(100);
    });
    await flush();
    expect(screen.queryByRole('status')).not.toBeInTheDocument();

    const failure = new FakeIpc();
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    const failedRender = renderPage(<CapsulePage route={route} />, { ipc: failure });
    await flush();
    failure.failNext('sessions.disconnect', { code: -32000, message: 'failed' });

    await failedRender.user.click(screen.getByRole('button', { name: '断开' }));
    await flush();

    expect(screen.getByRole('button', { name: '断开' })).toBeEnabled();
    expect(warn).toHaveBeenCalledWith('sessions.disconnect failed');
  });

  it('refetches sessions after sessionsChanged', async () => {
    const ipc = new FakeIpc();
    renderPage(<CapsulePage route={route} />, { ipc });
    await flush();
    expect(ipc.calls.filter((call) => call.method === 'sessions.list')).toHaveLength(1);

    act(() => ipc.emit('event.sessionsChanged', {}));
    await flush();

    expect(ipc.calls.filter((call) => call.method === 'sessions.list')).toHaveLength(2);
  });

  it('shows who is recording when a controller records', async () => {
    const ipc = new FakeIpc();
    setRecording(ipc, 0, true);
    renderPage(<CapsulePage route={route} />, { ipc });
    await flush();

    const marker = screen.getByRole('status', {
      name: '小明的 iPhone 正在录制这台 Mac 的画面',
    });
    expect(marker).toHaveTextContent('录制中');
  });

  it('shows a single marker with a device count when multiple sessions record', async () => {
    const ipc = FakeIpc.fromScenario('handoff');
    setRecording(ipc, 0, true);
    setRecording(ipc, 1, true);
    renderPage(<CapsulePage route={route} />, { ipc });
    await flush();

    expect(screen.getAllByText('录制中')).toHaveLength(1);
    expect(
      screen.getByRole('status', { name: '2 台设备正在录制这台 Mac 的画面' }),
    ).toBeInTheDocument();
  });

  it('does not show a marker when no session is recording or the field is missing', async () => {
    const ipc = new FakeIpc();
    renderPage(<CapsulePage route={route} />, { ipc });
    await flush();

    expect(screen.queryByText('录制中')).not.toBeInTheDocument();

    setRecording(ipc, 0, false);
    act(() => ipc.emit('event.sessionsChanged', {}));
    await flush();
    expect(screen.queryByText('录制中')).not.toBeInTheDocument();
  });

  it('marks a recording viewer even when the controller is not recording', async () => {
    const ipc = FakeIpc.fromScenario('handoff');
    setRecording(ipc, 0, false);
    setRecording(ipc, 1, true);
    renderPage(<CapsulePage route={route} />, { ipc });
    await flush();

    expect(
      screen.getByRole('status', { name: '小明的 iPad 正在录制这台 Mac 的画面' }),
    ).toHaveTextContent('录制中');
  });
});
