import type {
  RootScreenProps,
  RootStackParamList,
} from '../app/navigation/routes';

export function mockNavigation<
  R extends keyof RootStackParamList,
>(): RootScreenProps<R>['navigation'] & { [k: string]: jest.Mock } {
  // Create parents on demand so nested navigation mocks do not recurse forever.
  let parent: ReturnType<typeof mockNavigation<R>> | undefined;
  return {
    navigate: jest.fn(),
    navigateDeprecated: jest.fn(),
    goBack: jest.fn(),
    replace: jest.fn(),
    push: jest.fn(),
    pop: jest.fn(),
    popToTop: jest.fn(),
    reset: jest.fn(),
    setOptions: jest.fn(),
    dispatch: jest.fn(),
    emit: jest.fn(),
    addListener: jest.fn(() => jest.fn()),
    removeListener: jest.fn(),
    canGoBack: jest.fn(() => true),
    getParent: jest.fn(() => {
      parent ??= mockNavigation<R>();
      return parent;
    }),
    getState: jest.fn(),
    getId: jest.fn(),
    isFocused: jest.fn(() => true),
    setParams: jest.fn(),
    replaceParams: jest.fn(),
    preload: jest.fn(),
    popTo: jest.fn(),
  } as RootScreenProps<R>['navigation'] & { [k: string]: jest.Mock };
}

export function mockRoute<R extends keyof RootStackParamList>(
  name: R,
  params?: RootStackParamList[R],
): RootScreenProps<R>['route'] {
  return {
    key: `${name}-test`,
    name,
    params,
  } as unknown as RootScreenProps<R>['route'];
}
