import { bonjourAvailable, browseScreenSharing } from './bonjour';

let mockBonjour: {
  start: jest.Mock;
  stop: jest.Mock;
  onService: jest.Mock;
} | null = null;

jest.mock('../specs/NativeRsBonjour', () => ({
  __esModule: true,
  get default() {
    return mockBonjour;
  },
}));

beforeEach(() => {
  mockBonjour = null;
});

it('reports unavailable without a native module', () => {
  expect(bonjourAvailable()).toBe(false);
});

it('returns a callable no-op cleanup without a native module', () => {
  const onEvent = jest.fn();
  const stop = browseScreenSharing(onEvent);
  expect(typeof stop).toBe('function');
  expect(stop).not.toThrow();
  expect(stop).not.toThrow();
  expect(onEvent).not.toHaveBeenCalled();
});

it('subscribes before browsing and releases the subscription once', () => {
  const remove = jest.fn();
  const native = {
    start: jest.fn(),
    stop: jest.fn(),
    onService: jest.fn(() => ({ remove })),
  };
  mockBonjour = native;
  const onEvent = jest.fn();
  expect(bonjourAvailable()).toBe(true);
  const stop = browseScreenSharing(onEvent);
  expect(native.onService).toHaveBeenCalledWith(onEvent);
  expect(native.start).toHaveBeenCalledWith('_rfb._tcp');
  expect(native.onService.mock.invocationCallOrder[0]).toBeLessThan(
    native.start.mock.invocationCallOrder[0],
  );
  stop();
  stop();
  expect(remove).toHaveBeenCalledTimes(1);
  expect(native.stop).toHaveBeenCalledTimes(1);
});
