import { harnessLog } from '../native/devharness/log';
import {
  getLaunchOptions,
  parseLaunchProps,
  setLaunchOptions,
} from './launchOptions';

jest.mock('../native/devharness/log', () => ({ harnessLog: jest.fn() }));

beforeEach(() => jest.clearAllMocks());

it('defaults to no launch options before they are set', () => {
  expect(getLaunchOptions()).toEqual(parseLaunchProps({}));
});

it.each([
  ['30', 30],
  ['1', 1],
  ['86400', 86400],
  ['0', undefined],
  ['abc', undefined],
  ['99999', undefined],
  ['1.5', undefined],
  ['1e2', undefined],
  ['-1', undefined],
])('parses disconnectAfter %s as %s', (value, expected) => {
  expect(parseLaunchProps({ disconnectAfter: value }).disconnectAfterS).toBe(
    expected,
  );
});

it.each(['YES', 'yes', '1', 'true', 'TrUe'])(
  'accepts selfTestMove %s',
  value => {
    expect(parseLaunchProps({ selfTestMove: value }).selfTestMove).toBe(true);
  },
);

it('defaults selfTestMove to false', () => {
  expect(parseLaunchProps({}).selfTestMove).toBe(false);
  expect(parseLaunchProps({ selfTestMove: 'NO' }).selfTestMove).toBe(false);
});

it('ignores the removed Hub URL and fake directory launch properties', () => {
  const options = parseLaunchProps({
    hubUrl: 'http://100.64.0.10:7300',
    fakeDirectoryJson: '{"computers":[],"servers":[]}',
    fakeDirectoryError: 'notFound',
  });
  expect(options).toEqual(parseLaunchProps({}));
  expect(options).not.toHaveProperty('hubUrl');
  expect(options).not.toHaveProperty('fakeDirectoryJson');
  expect(options).not.toHaveProperty('fakeDirectoryError');
  expect(harnessLog).not.toHaveBeenCalled();
});

it.each([42, {}, [], true, null, undefined, '', '   '])(
  'ignores non-string or empty properties: %s',
  value => {
    expect(
      parseLaunchProps({
        dataDir: value,
        autoConnect: value,
        disconnectAfter: value,
        selfTestMove: value,
        devHarness: value,
        harnessArg: value,
      }),
    ).toEqual(parseLaunchProps({}));
    expect(harnessLog).not.toHaveBeenCalled();
  },
);

it('preserves all native string launch properties and stores the parsed options', () => {
  const props = {
    dataDir: '/app/support/core',
    autoConnect: '100.64.0.10',
    disconnectAfter: '30',
    selfTestMove: 'YES',
    devHarness: 'ping',
    harnessArg: 'example',
  };
  const options = parseLaunchProps(props);
  expect(options).toEqual({
    dataDir: props.dataDir,
    autoConnect: props.autoConnect,
    disconnectAfterS: 30,
    selfTestMove: true,
    devHarness: props.devHarness,
    harnessArg: props.harnessArg,
  });
  setLaunchOptions(options);
  expect(getLaunchOptions()).toBe(options);
  setLaunchOptions(parseLaunchProps({}));
});
