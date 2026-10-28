import mockSafeAreaContext from 'react-native-safe-area-context/jest/mock';
import { Share } from 'react-native';

jest.mock('react-native-safe-area-context', () => mockSafeAreaContext);
jest.mock('react-native-rs-core', () => ({}));

// RN 0.88's preset forwards RendererProxy to the real Fabric renderer. Merely
// loading ScrollView/Animated then compiles ReactFabric-dev in every worker.
// Our test renderer owns the tree; native renderer operations have no target.
jest.mock('react-native/Libraries/ReactNative/RendererProxy', () =>
  require('./mocks/RendererProxyMock'),
);
// The preset's ScrollView mock calls requireActual to copy component statics,
// which also imports the whole Animated implementation on its first render.
jest.mock('react-native/Libraries/Components/ScrollView/ScrollView', () =>
  require('./mocks/ScrollViewMock'),
);

// MMKV 4 selects its own in-memory createMockMMKV when JEST_WORKER_ID is set.
// Keep its real hooks and public API, without initializing the Nitro runtime.
jest.mock('react-native-nitro-modules', () => ({}));
jest.mock('react-native-mmkv', () => {
  const mmkv =
    jest.requireActual<typeof import('react-native-mmkv')>('react-native-mmkv');
  return { ...mmkv, createMMKV: jest.fn(mmkv.createMMKV) };
});

// The RN preset already mocks AccessibilityInfo, AppState and Linking.
// StatusBar's methods remain spyable and use the preset's native module mock.
// Share is not mocked by the preset; keep tests out of the native share sheet.
Share.share = jest.fn().mockResolvedValue({ action: Share.sharedAction });
