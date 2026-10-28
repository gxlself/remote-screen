import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

import { describe, expect, it } from 'vitest';

import { colors } from './tokens';

describe('CSS custom properties', () => {
  const css = readFileSync(resolve(process.cwd(), 'src/theme/global.css'), 'utf8');

  it.each(Object.entries(colors))('defines --rs-%s', (name, value) => {
    expect(css).toContain(`--rs-${name}: ${value};`);
  });
});
