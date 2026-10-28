import { act, render, waitFor } from '@testing-library/react-native';
import SessionViewHarness from '../devharness/SessionViewHarness';
import { CoreProvider } from '../rscore/CoreContext';
import { createFakeCore } from '../rscore/fakeCore';
import type { RemoteSessionViewProps } from './RemoteSessionView';

let mockProps: RemoteSessionViewProps;

jest.mock('./RemoteSessionView', () => ({
  RemoteSessionView: (p: RemoteSessionViewProps) => {
    mockProps = p;
    return null;
  },
}));

it('opens a control session and closes it on unmount', async () => {
  const core = createFakeCore();
  const open = jest.spyOn(core.sessions, 'open').mockResolvedValue('123');
  const close = jest.spyOn(core.sessions, 'close');
  const log = jest.fn();
  const view = await render(
    <CoreProvider core={core}>
      <SessionViewHarness arg="host-id" log={log} />
    </CoreProvider>,
  );
  await waitFor(() => expect(mockProps.sessionId).toBe('123'));
  expect(open).toHaveBeenCalledWith('host-id', { intent: 'control' });
  expect(log).toHaveBeenCalledWith('SESSVIEW-OPEN', { sessionId: '123' });
  await view.unmount();
  expect(close).toHaveBeenCalledTimes(1);
  expect(close).toHaveBeenCalledWith('123');
});

it('closes a session whose open completes after unmount', async () => {
  const core = createFakeCore();
  let resolveOpen!: (id: string) => void;
  jest.spyOn(core.sessions, 'open').mockImplementation(
    () =>
      new Promise(resolve => {
        resolveOpen = resolve;
      }),
  );
  const close = jest.spyOn(core.sessions, 'close');
  const log = jest.fn();
  const view = await render(
    <CoreProvider core={core}>
      <SessionViewHarness arg="host-id" log={log} />
    </CoreProvider>,
  );
  await view.unmount();
  await act(async () => {
    resolveOpen('456');
  });
  expect(close).toHaveBeenCalledTimes(1);
  expect(close).toHaveBeenCalledWith('456');
  expect(log).not.toHaveBeenCalledWith('SESSVIEW-OPEN', { sessionId: '456' });
});

it('toggles input mode and keyboard inset from gesture events', async () => {
  const core = createFakeCore();
  jest.spyOn(core.sessions, 'open').mockResolvedValue('123');
  const log = jest.fn();
  await render(
    <CoreProvider core={core}>
      <SessionViewHarness arg="host-id" log={log} />
    </CoreProvider>,
  );
  await waitFor(() => expect(mockProps.sessionId).toBe('123'));
  expect(mockProps.mode).toBe('trackpad');
  await act(async () => {
    mockProps.onGesture?.({ kind: 'twoFingerDoubleTap' });
  });
  expect(mockProps.mode).toBe('touch');
  await act(async () => {
    mockProps.onGesture?.({ kind: 'threeFingerTap' });
  });
  expect(mockProps.keyboardInset).toBe(300);
  await act(async () => {
    mockProps.onGesture?.({ kind: 'twoFingerDoubleTap' });
    mockProps.onGesture?.({ kind: 'threeFingerTap' });
  });
  expect(mockProps.mode).toBe('trackpad');
  expect(mockProps.keyboardInset).toBe(0);
  expect(log).toHaveBeenCalledWith('SESSVIEW-GESTURE', {
    kind: 'threeFingerTap',
  });
});
