import path from 'node:path';

import react from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  envPrefix: ['VITE_', 'TAURI_ENV_'],
  resolve: {
    // host-ui components and App's pure TS modules are reused read-only; they must
    // resolve React from desktop/node_modules, not from their own packages.
    alias: {
      '@host-ui': path.resolve(__dirname, '../host-ui/src'),
      '@app': path.resolve(__dirname, '../app/src'),
    },
    dedupe: ['react', 'react-dom'],
  },
  server: {
    port: 1430,
    strictPort: true,
    fs: {
      allow: ['..'],
    },
  },
  build: {
    target: 'safari15',
  },
  esbuild: {
    // A string here stops esbuild reading each file's nearest tsconfig.json: app/'s extends
    // @react-native/typescript-config, which needs app/node_modules. Same options as tsconfig.json.
    tsconfigRaw: JSON.stringify({
      compilerOptions: { verbatimModuleSyntax: true, useDefineForClassFields: true },
    }),
  },
  test: {
    environment: 'jsdom',
    setupFiles: ['src/test/setup.ts'],
    include: ['src/**/*.test.{ts,tsx}'],
    restoreMocks: true,
  },
});
