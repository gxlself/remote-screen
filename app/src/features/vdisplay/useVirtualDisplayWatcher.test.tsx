import { act, cleanup } from '@testing-library/react-native';
import { liveSession } from '../../native/rscore/fixtures';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { renderWithProviders } from '../../test/render';
import { vdisplay } from '../../i18n/zh/vdisplay';
import { resetVdisplayStore, useVdisplayStore } from './store';
import { useVirtualDisplayWatcher } from './useVirtualDisplay';

function Watcher(props: {
  enabled: boolean;
  onCreated(): void;
  onError(text: string): void;
}) {
  useVirtualDisplayWatcher({ sessionId: liveSession.id, ...props });
  return null;
}

async function mount(enabled = true) {
  const core = createFakeCore({ sessions: [liveSession] });
  const onCreated = jest.fn();
  const onError = jest.fn();
  const view = await renderWithProviders(
    <Watcher enabled={enabled} onCreated={onCreated} onError={onError} />,
    { core },
  );
  return { ...view, core, onCreated, onError };
}

afterEach(async () => {
  await cleanup();
  jest.useRealTimers();
  resetVdisplayStore();
});

describe('useVirtualDisplayWatcher', () => {
  it('reports a virtual display created after the sheet closed', async () => {
    useVdisplayStore.getState().setPending(liveSession.id, {
      requestId: 4,
      deadlineMs: Date.now() + 15_000,
    });
    const view = await mount();
    await act(async () => {
      view.core.emit({
        type: 'session.vdisplay',
        sessionId: liveSession.id,
        requestId: 4,
        status: 'ok',
        display: {
          id: 9,
          name: '虚拟屏',
          widthPx: 2732,
          heightPx: 2048,
          primary: false,
          isVirtual: true,
        },
      });
    });
    expect(view.onCreated).toHaveBeenCalledTimes(1);
    expect(useVdisplayStore.getState().owned[liveSession.id]).toBe(9);
    expect(useVdisplayStore.getState().pending[liveSession.id]).toBeUndefined();
  });

  it('reports a failure and a timeout', async () => {
    jest.useFakeTimers();
    useVdisplayStore.getState().setPending(liveSession.id, {
      requestId: 5,
      deadlineMs: Date.now() + 15_000,
    });
    const view = await mount();
    await act(async () => {
      view.core.emit({
        type: 'session.vdisplay',
        sessionId: liveSession.id,
        requestId: 5,
        status: 'limit',
      });
    });
    expect(view.onError).toHaveBeenLastCalledWith(vdisplay.error.limit);

    await act(async () => {
      useVdisplayStore.getState().setPending(liveSession.id, {
        requestId: 6,
        deadlineMs: Date.now() + 1_000,
      });
    });
    await act(async () => {
      jest.advanceTimersByTime(1_000);
    });
    expect(view.onError).toHaveBeenLastCalledWith(vdisplay.error.timeout);
    expect(useVdisplayStore.getState().pending[liveSession.id]).toBeUndefined();
  });

  it('leaves the result to the open sheet while disabled', async () => {
    useVdisplayStore.getState().setPending(liveSession.id, {
      requestId: 7,
      deadlineMs: Date.now() + 15_000,
    });
    const view = await mount(false);
    await act(async () => {
      view.core.emit({
        type: 'session.vdisplay',
        sessionId: liveSession.id,
        requestId: 7,
        status: 'failed',
      });
    });
    expect(view.onError).not.toHaveBeenCalled();
    expect(useVdisplayStore.getState().pending[liveSession.id]?.requestId).toBe(
      7,
    );
  });
});
