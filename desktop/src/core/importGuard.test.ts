import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

const SRC = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const APP_SRC = path.resolve(SRC, '../../app/src');
const RSCORE = path.join(APP_SRC, 'native/rscore');
/** The App modules desktop reuses read-only; they must stay free of React / RN / react-query. */
const REUSED = ['bindings.ts', 'createRsCore.ts', 'errors.ts', 'RsCoreApi.ts', 'types.ts'].map(
  (name) => path.join(RSCORE, name),
);
const SPECIFIER = /\bfrom\s*['"]([^'"]+)['"]|\bimport\s*\(?\s*['"]([^'"]+)['"]/g;

function specifiers(file: string): string[] {
  const source = readFileSync(file, 'utf8');
  return [...source.matchAll(SPECIFIER)].map((m) => m[1] ?? m[2] ?? '');
}

function resolveFile(base: string): string {
  const candidates = [base, `${base}.ts`, `${base}.tsx`, `${base}/index.ts`, `${base}/index.tsx`];
  const found = candidates.find((c) => existsSync(c) && statSync(c).isFile());
  if (found === undefined) {
    throw new Error(`cannot resolve ${base}`);
  }
  return found;
}

function sourceFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      return sourceFiles(full);
    }
    return /\.tsx?$/.test(entry.name) ? [full] : [];
  });
}

describe('import_guard', () => {
  it('desktop imports only the reused App modules from @app', () => {
    const fromApp = new Set(
      sourceFiles(SRC).flatMap((file) =>
        specifiers(file)
          .filter((spec) => spec.startsWith('@app/'))
          .map((spec) => resolveFile(path.join(APP_SRC, spec.slice('@app/'.length)))),
      ),
    );

    expect(fromApp.size).toBeGreaterThan(0);
    for (const file of fromApp) {
      expect(REUSED).toContain(file);
    }
  });

  it('reused App modules and their relative imports avoid react, react-native and @tanstack', () => {
    const seen = new Set<string>();
    const queue = [...REUSED];
    const offending: string[] = [];
    while (queue.length > 0) {
      const file = queue.pop() as string;
      if (seen.has(file)) {
        continue;
      }
      seen.add(file);
      for (const spec of specifiers(file)) {
        if (/^(?:react|react-native)(?:\/|$)/.test(spec) || spec.startsWith('@tanstack/')) {
          offending.push(`${path.relative(APP_SRC, file)}: ${spec}`);
        }
        if (spec.startsWith('.')) {
          queue.push(resolveFile(path.resolve(path.dirname(file), spec)));
        }
      }
    }

    expect(offending).toEqual([]);
    for (const file of REUSED) {
      expect([...seen]).toContain(file);
    }
  });

  it('only src/core/tauri.ts imports @tauri-apps', () => {
    const importers = sourceFiles(SRC)
      .filter((file) => specifiers(file).some((spec) => spec.startsWith('@tauri-apps/')))
      .map((file) => path.relative(SRC, file).split(path.sep).join('/'));

    expect(importers).toEqual(['core/tauri.ts']);
  });
});
