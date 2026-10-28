// Tests format wall-clock times; pin the zone so results do not depend on the
// machine running them (CI workers are UTC, developer Macs often are not).
// Set here, before Jest starts its workers, so every worker inherits it.
process.env.TZ = 'UTC';

module.exports = {
  preset: '@react-native/jest-preset',
  watchman: false,
  maxWorkers: 4,
  setupFiles: ['<rootDir>/src/test/setup.ts'],
  // SVG, safe-area, TanStack Query and Zustand ship runnable CommonJS entry
  // points. Only transform packages that still publish Flow, JSX or ESM.
  transformIgnorePatterns: [
    'node_modules/(?!((jest-)?react-native|@react-native(-community)?|@react-navigation|react-native-screens|react-native-mmkv)/)',
  ],
  testPathIgnorePatterns: ['/node_modules/', '/artifacts/', '/ios/'],
  // Rust binding generation stages another package.json under artifacts/.
  modulePathIgnorePatterns: ['<rootDir>/artifacts/', '<rootDir>/ios/'],
};
