import { Children, isValidElement } from 'react';
import type { ReactNode } from 'react';
import { act, render } from '@testing-library/react-native';
import { Dimensions, Platform } from 'react-native';
import type { ScaledSize } from 'react-native';
import RootNavigator from './RootNavigator';

type Options = Record<string, unknown>;
type DimensionsChange = (dims: { window: ScaledSize; screen: ScaledSize }) => void;
type NavigatorProps = { screenOptions?: Options; children?: ReactNode };

const mockNavigator = jest.fn<null, [NavigatorProps]>(() => null);

// Read the options RootNavigator declares without mounting any screen.
jest.mock('@react-navigation/native-stack', () => ({
  createNativeStackNavigator: () => ({
    Navigator: (props: NavigatorProps) => mockNavigator(props),
    Group: () => null,
    Screen: () => null,
  }),
}));

function routeOptions(): Map<string, Options> {
  const props = mockNavigator.mock.lastCall?.[0];
  if (props === undefined) {
    throw new Error('RootNavigator did not render the stack');
  }
  const routes = new Map<string, Options>();
  const visit = (children: ReactNode, inherited: Options) => {
    Children.forEach(children, child => {
      if (!isValidElement<Record<string, unknown>>(child)) {
        return;
      }
      const { name, screenOptions, children: nested } = child.props;
      if (typeof name === 'string') {
        const own = child.props.options as Options | undefined;
        routes.set(name, { ...inherited, ...own });
      } else {
        visit(nested as ReactNode, {
          ...inherited,
          ...(screenOptions as Options),
        });
      }
    });
  };
  visit(props.children, props.screenOptions ?? {});
  return routes;
}

function options(name: string): Options {
  const found = routeOptions().get(name);
  if (found === undefined) {
    throw new Error(`no route ${name}`);
  }
  return found;
}

function mockScreen(size: { width: number; height: number }) {
  const get = Dimensions.get.bind(Dimensions);
  jest
    .spyOn(Dimensions, 'get')
    .mockImplementation(dim =>
      dim === 'screen' ? { ...get('screen'), ...size } : get(dim),
    );
}

function mockIos({ pad }: { pad: boolean }) {
  jest.replaceProperty(Platform, 'OS', 'ios');
  // Platform's static type is a union; only the iOS object has `isPad`.
  const ios = Platform as unknown as { isPad: boolean };
  jest.spyOn(ios, 'isPad', 'get').mockReturnValue(pad);
}

beforeEach(() => {
  mockNavigator.mockClear();
});

afterEach(() => jest.restoreAllMocks());

describe('RootNavigator session screens', () => {
  it.each(['Session', 'AdapterSession'])(
    'hides the home indicator and Android navigation bar on %s',
    async name => {
      await render(<RootNavigator />);
      expect(options(name)).toMatchObject({
        presentation: 'fullScreenModal',
        statusBarHidden: true,
        autoHideHomeIndicator: true,
        navigationBarHidden: true,
        gestureEnabled: false,
      });
    },
  );

  it.each(['SessionEnded', 'MainTabs', 'Terminal'])(
    'keeps the system bars on %s',
    async name => {
      await render(<RootNavigator />);
      expect(options(name).autoHideHomeIndicator).toBeUndefined();
      expect(options(name).navigationBarHidden).toBeUndefined();
    },
  );
});

describe('RootNavigator gestures and orientation', () => {
  it('cannot swipe the pairing result away', async () => {
    await render(<RootNavigator />);
    expect(options('PairDone')).toMatchObject({
      presentation: 'modal',
      gestureEnabled: false,
    });
  });

  it('lets the terminal rotate on a phone', async () => {
    mockIos({ pad: false });
    await render(<RootNavigator />);
    expect(options('MainTabs').orientation).toBe('portrait');
    expect(options('Terminal').orientation).toBe('default');
  });

  it('locks an iPhone to portrait and sessions to landscape', async () => {
    mockIos({ pad: false });
    await render(<RootNavigator />);
    expect(options('MainTabs').orientation).toBe('portrait');
    expect(options('Session').orientation).toBe('landscape');
    expect(options('AdapterSession').orientation).toBe('landscape');
  });

  it('does not lock an iPad', async () => {
    mockIos({ pad: true });
    await render(<RootNavigator />);
    expect(options('MainTabs').orientation).toBe('default');
    expect(options('Session').orientation).toBe('default');
  });

  it('locks an Android phone by its shortest side', async () => {
    jest.replaceProperty(Platform, 'OS', 'android');
    mockScreen({ width: 891, height: 411 });
    await render(<RootNavigator />);
    expect(options('MainTabs').orientation).toBe('portrait');
    expect(options('Session').orientation).toBe('landscape');
    expect(options('Terminal').orientation).toBe('default');
  });

  it('does not lock an Android tablet', async () => {
    jest.replaceProperty(Platform, 'OS', 'android');
    mockScreen({ width: 1280, height: 800 });
    await render(<RootNavigator />);
    expect(options('MainTabs').orientation).toBe('default');
    expect(options('Session').orientation).toBe('default');
    expect(options('AdapterSession').orientation).toBe('default');
  });

  it('unlocks when a foldable opens to tablet size', async () => {
    jest.replaceProperty(Platform, 'OS', 'android');
    mockScreen({ width: 400, height: 880 });
    let change: DimensionsChange | undefined;
    const remove = jest.fn();
    jest
      .spyOn(Dimensions, 'addEventListener')
      .mockImplementation((_, handler) => {
        change = handler as DimensionsChange;
        return { remove };
      });
    const view = await render(<RootNavigator />);
    expect(options('MainTabs').orientation).toBe('portrait');

    const unfolded = { width: 880, height: 840, scale: 2.6, fontScale: 1 };
    await act(() => change?.({ window: unfolded, screen: unfolded }));
    expect(options('MainTabs').orientation).toBe('default');
    expect(options('Session').orientation).toBe('default');

    await view.unmount();
    expect(remove).toHaveBeenCalledTimes(1);
  });
});
